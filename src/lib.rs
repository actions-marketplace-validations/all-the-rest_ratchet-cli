//! `ratchet` — quality gates that compare a **count** against a **baseline**
//! that may only move in one declared direction.
//!
//! The engine owns the baseline, the direction, the monotonicity and the
//! reporting; the project owns the counting.  See `SPEC.md` for the
//! normative description of the behaviour.

pub mod baseline;
pub mod check;
pub mod config;
pub mod counter;
pub mod generate;
pub mod git;
pub mod path;
pub mod report;

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::baseline::{Baseline, BaselineRecord};
use crate::check::{ActionKind, BaselineAction, Report};
use crate::config::{CONFIG_FILE_NAMES, Config, DEFAULT_BASELINE_FILE, Direction};
use crate::git::Git;

/// The starter config `ratchet init` writes.
const STARTER_CONFIG: &str = r#"// ratchet — starter config.
//
// One rule per defect class.  The example counts TODO markers in the
// repository: the baseline may only decrease, never grow.
//
// The incident of a rule names the instance that motivated it.  A rule for a
// class that has not occurred yet is a prevention rule, and its incident says
// so — a gate must not claim a history it does not have.
[
  {
    "name": "starter",
    "rules": {
      "todo-markers": {
        "class": "unfinished work",
        "incident": "prevention: no instance in this project yet — replace this with the incident that motivated the rule",
        "unit": "markers",
        "direction": "ceiling",
        "tier": "fast",
        "count": {
          "command": [
            "sh",
            "-c",
            "grep -rI --include='*.ts' --include='*.tsx' -o 'TODO' . 2>/dev/null | wc -l | tr -d '[:space:]'"
          ]
        }
      }
    }
  }
]
"#;

/// An operational error; every one of them is exit code 2.
#[derive(Debug)]
pub struct Error {
    message: String,
    /// The raw output of a subprocess, if one was involved; the diagnostic
    /// channel carries it (SPEC 6.2).
    output: Option<String>,
}

impl Error {
    /// Creates an error with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            output: None,
        }
    }

    /// The message of the error.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The raw output of a subprocess, if one was involved.
    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

impl From<config::ConfigError> for Error {
    fn from(error: config::ConfigError) -> Self {
        Self {
            message: error.to_string(),
            output: error.output,
        }
    }
}

impl From<baseline::BaselineError> for Error {
    fn from(error: baseline::BaselineError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

/// Everything a run needs: the config, the repository root and the baseline.
pub struct Context {
    /// The merged config.
    pub config: Config,
    /// The config file that was loaded.
    pub config_path: PathBuf,
    /// The repository root (the git root, or the config directory).
    pub root: PathBuf,
    /// The absolute path of the baseline file.
    pub baseline_path: PathBuf,
    /// The baseline path relative to the repository root, for `git show`.
    pub baseline_rel: Option<String>,
    /// The git repository, if there is one.
    pub git: Git,
}

/// Runs the CLI and returns the exit code.
pub fn run(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args: Vec<OsString> = args.into_iter().collect();
    let cli = Cli::parse_from(normalize_args(args));
    let debug = cli.debug;
    match run_inner(cli) {
        Ok(code) => code,
        Err(error) => {
            // The error comes first; the transcript explains it afterwards.
            eprintln!("ratchet: error: {error}");
            // The raw output of a subprocess belongs to the diagnostic channel
            // (SPEC 6.2): it is printed only with --debug.
            if debug {
                if let Some(output) = error.output() {
                    eprintln!("ratchet: debug: {output}");
                }
            }
            2
        }
    }
}

fn run_inner(cli: Cli) -> Result<i32, Error> {
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(format!("cannot read the working directory: {e}")))?;

    match &cli.command {
        Command::Check(args) => cmd_check(&cli, args, &cwd),
        Command::Count(_) => cmd_count(&cli, &cwd),
        Command::Baseline(args) => match &args.command {
            BaselineCommand::Init(args) => cmd_baseline(&cli, &cwd, false, args.reason.as_deref()),
            BaselineCommand::Update(args) => cmd_baseline(&cli, &cwd, true, args.reason.as_deref()),
        },
        Command::Init(_) => cmd_init(&cli, &cwd),
    }
}

fn cmd_check(cli: &Cli, args: &CheckArgs, cwd: &Path) -> Result<i32, Error> {
    let context = load_context(cli, cwd)?;
    ensure_rule_exists(&context.config, cli.rule.as_deref())?;
    ensure_paths_exist(&args.paths, cwd, &context.root)?;
    let baseline = Baseline::load_if_exists(Some(&context.root), &context.baseline_path)?;

    let mut notes = Vec::new();
    // A committed baseline that exists but cannot be parsed is an operational
    // error: a broken reference must not disable the gate (SPEC 5.2, 8).
    let committed = read_committed_baseline(&context)?;
    if let Some(note) = check::monotonicity_note(
        &context.git,
        context
            .baseline_rel
            .as_deref()
            .unwrap_or(DEFAULT_BASELINE_FILE),
        committed.as_ref(),
    ) {
        notes.push(note);
    }

    let paths = check::selected_paths(&args.paths, cwd, &context.root);
    let mut report = check::check(
        &context.config,
        &baseline,
        committed.as_ref(),
        &context.root,
        cli.rule.as_deref(),
        &paths,
    );
    report.notes.extend(notes);

    if report.executed == 0 {
        report
            .notes
            .push("no rule was executed — an empty selection is never a pass".to_string());
    }
    if report
        .rules
        .iter()
        .any(|rule| rule.status == check::RuleStatus::MissingBaseline)
    {
        report
            .notes
            .push("hint: run `ratchet baseline init` to write the missing baseline".to_string());
    }
    if report
        .rules
        .iter()
        .any(|rule| rule.status == check::RuleStatus::CounterError)
    {
        report
            .notes
            .push("hint: run with --debug to see the output of the counter".to_string());
    }

    if cli.debug {
        report.debug.insert(0, resolved_config(&context.config));
    }

    match cli.format {
        Format::Json => println!("{}", report::render_json(&report)),
        Format::Stylish => print!("{}", report::render_stylish(&report, cli.quiet)),
    }
    print_notes(&report, cli);

    Ok(exit_code(&report))
}

/// The resolved config: the first line of the diagnostic channel.
fn resolved_config(config: &Config) -> String {
    format!(
        "resolved config: {}",
        serde_json::to_string_pretty(config).unwrap_or_default()
    )
}

fn cmd_count(cli: &Cli, cwd: &Path) -> Result<i32, Error> {
    let context = load_context(cli, cwd)?;
    ensure_rule_exists(&context.config, cli.rule.as_deref())?;
    let mut report = check::counts(&context.config, &context.root, cli.rule.as_deref());
    if cli.debug {
        report.debug.insert(0, resolved_config(&context.config));
    }
    if report
        .rules
        .iter()
        .any(|rule| rule.status == check::RuleStatus::CounterError)
    {
        report
            .notes
            .push("hint: run with --debug to see the output of the counter".to_string());
    }

    match cli.format {
        Format::Json => println!("{}", report::render_json(&report)),
        Format::Stylish => print!("{}", report::render_counts(&report)),
    }
    print_notes(&report, cli);

    if report.counter_errors > 0 {
        Ok(2)
    } else {
        Ok(0)
    }
}

fn cmd_baseline(cli: &Cli, cwd: &Path, update: bool, reason: Option<&str>) -> Result<i32, Error> {
    let context = load_context(cli, cwd)?;
    let mut baseline = Baseline::load_if_exists(Some(&context.root), &context.baseline_path)?;

    ensure_rule_exists(&context.config, cli.rule.as_deref())?;
    let mut actions: Vec<BaselineAction> = Vec::new();
    let mut failed = 0usize;
    let mut counter_errors = 0usize;
    let mut changed = false;
    let mut debug: Vec<String> = Vec::new();

    for (id, rule) in &context.config.rules {
        if let Some(filter) = &cli.rule {
            if filter != id {
                continue;
            }
        }

        let existing = baseline.get(id).map(|record| record.value);

        // `baseline init` only writes the rules that have no baseline: the
        // counters of the other rules are not needed and a broken counter
        // must not fail the command (SPEC 5.4).
        if !update {
            if let Some(value) = existing {
                actions.push(BaselineAction {
                    id: id.clone(),
                    kind: ActionKind::Kept,
                    value: Some(value),
                    message: format!("kept baseline {value}"),
                });
                continue;
            }
        }

        let run = match counter::run_counter(&rule.count, &context.root) {
            Ok(run) => run,
            Err(error) => {
                counter_errors += 1;
                failed += 1;
                debug.push(format!(
                    "counter {id}: {}\n{}",
                    rule.count.describe(),
                    error.output.clone().unwrap_or_default()
                ));
                actions.push(BaselineAction {
                    id: id.clone(),
                    kind: ActionKind::Refused,
                    value: None,
                    message: error.message.clone(),
                });
                continue;
            }
        };
        debug.push(format!(
            "counter {id}: {}\n{}\n--- count ---",
            rule.count.describe(),
            run.output
        ));
        let count = match counter::finalize(
            run.value,
            &rule.exclude_globs,
            &format!("rules.{id}.exclude"),
        ) {
            Ok(count) => count,
            Err(error) => {
                counter_errors += 1;
                failed += 1;
                actions.push(BaselineAction {
                    id: id.clone(),
                    kind: ActionKind::Refused,
                    value: None,
                    message: error.message.clone(),
                });
                continue;
            }
        };
        debug.push(format!("counter {id}: count {count}"));
        match existing {
            None if update => {
                failed += 1;
                actions.push(BaselineAction {
                    id: id.clone(),
                    kind: ActionKind::Refused,
                    value: None,
                    message: format!("refusing: no baseline for {id} — run ratchet baseline init"),
                });
            }
            None => {
                baseline.rules.insert(
                    id.clone(),
                    BaselineRecord {
                        value: count,
                        reason: reason.map(str::to_string),
                    },
                );
                changed = true;
                actions.push(BaselineAction {
                    id: id.clone(),
                    kind: ActionKind::Added,
                    value: Some(count),
                    message: format!("added baseline {count}"),
                });
            }
            Some(value) => {
                let allowed = match rule.direction {
                    Direction::Ceiling => count <= value,
                    Direction::Floor => count >= value,
                };
                if allowed {
                    let reason = reason
                        .map(str::to_string)
                        .or_else(|| baseline.get(id).and_then(|record| record.reason.clone()));
                    baseline.rules.insert(
                        id.clone(),
                        BaselineRecord {
                            value: count,
                            reason,
                        },
                    );
                    changed = true;
                    actions.push(BaselineAction {
                        id: id.clone(),
                        kind: ActionKind::Updated,
                        value: Some(count),
                        message: format!("{value} → {count}"),
                    });
                } else {
                    failed += 1;
                    let message = match rule.direction {
                        Direction::Ceiling => format!(
                            "refusing: count {count} > baseline {value} — a ceiling may only decrease"
                        ),
                        Direction::Floor => format!(
                            "refusing: count {count} < baseline {value} — a floor may only increase"
                        ),
                    };
                    actions.push(BaselineAction {
                        id: id.clone(),
                        kind: ActionKind::Refused,
                        value: Some(value),
                        message,
                    });
                }
            }
        }
    }

    if changed {
        baseline.save(Some(&context.root), &context.baseline_path)?;
    }

    if cli.debug {
        debug.insert(0, resolved_config(&context.config));
        for line in &debug {
            eprintln!("ratchet: debug: {line}");
        }
    }

    let command = if update { "update" } else { "init" };
    // The path that was written — relative to the repository root.
    let path = path::display(Some(&context.root), &context.baseline_path);
    match cli.format {
        Format::Json => println!(
            "{}",
            report::render_baseline_json(command, &actions, failed)
        ),
        Format::Stylish => print!("{}", report::render_baseline(command, &path, &actions)),
    }

    if counter_errors > 0 {
        Ok(2)
    } else if failed > 0 {
        Ok(1)
    } else {
        Ok(0)
    }
}

fn cmd_init(cli: &Cli, cwd: &Path) -> Result<i32, Error> {
    // The same discovery as `check`: a config in a parent directory of the
    // repository must not be shadowed by a second one.
    let git = Git::discover(cwd);
    if let Some(existing) = discover_config(cli, cwd, git.root())? {
        let name = path::display(git.root(), &existing);
        if cli.debug {
            // The resolved config of the config that exists.
            match load_config_file(&existing) {
                Ok(config) => eprintln!("ratchet: debug: {}", resolved_config(&config)),
                Err(error) => eprintln!("ratchet: debug: {error}"),
            }
        }
        match cli.format {
            Format::Json => println!("{}", report::render_init_json(&format!("{name} (exists)"))),
            Format::Stylish => println!("ratchet: {name} already exists — nothing to do"),
        }
        return Ok(0);
    }

    if cli.debug {
        eprintln!("ratchet: debug: no config found — nothing to resolve");
    }

    // The config is written where the discovery finds it: the root of the
    // repository, or the working directory when there is no repository.
    let directory = git
        .root()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.to_path_buf());
    let path = directory.join("ratchet.config.jsonc");
    fs::write(&path, STARTER_CONFIG).map_err(|e| {
        Error::new(format!(
            "cannot write {}: {e}",
            path::display(git.root(), &path)
        ))
    })?;
    if cli.debug {
        eprintln!("ratchet: debug: wrote {}", path::display(git.root(), &path));
    }
    match cli.format {
        Format::Json => println!("{}", report::render_init_json("ratchet.config.jsonc")),
        Format::Stylish => println!("ratchet: wrote ratchet.config.jsonc"),
    }
    Ok(0)
}

/// Fails when `--rule` names a rule that does not exist.
fn ensure_rule_exists(config: &Config, rule: Option<&str>) -> Result<(), Error> {
    if let Some(rule) = rule {
        if !config.rules.contains_key(rule) {
            return Err(Error::new(format!("unknown rule: {rule}")));
        }
    }
    Ok(())
}

/// Fails when a selected path does not exist.
///
/// A typo must not silently narrow the run into a green one (SPEC 5.2).
fn ensure_paths_exist(paths: &[String], cwd: &Path, root: &Path) -> Result<(), Error> {
    for path in paths {
        let candidate = Path::new(path);
        let absolute = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            cwd.join(candidate)
        };
        if !absolute.exists() {
            return Err(Error::new(format!(
                "unknown command or path: {} — it is neither a ratchet command nor an existing path",
                path::display(Some(root), &absolute)
            )));
        }
    }
    Ok(())
}

/// Loads the config, the repository root and the baseline path.
fn load_context(cli: &Cli, cwd: &Path) -> Result<Context, Error> {
    let git = Git::discover(cwd);
    let config_path = find_config(cli, cwd, git.root())?;
    let config = load_config_file(&config_path)?;
    let config_dir = parent_dir(&config_path);
    let root = git
        .root()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config_dir.clone());

    let baseline_rel = config
        .baseline
        .clone()
        .unwrap_or_else(|| DEFAULT_BASELINE_FILE.to_string());
    let baseline_path = path::normalize(&if Path::new(&baseline_rel).is_absolute() {
        PathBuf::from(&baseline_rel)
    } else {
        config_dir.join(&baseline_rel)
    });

    // The baseline has to be comparable with its committed version, and that
    // only works inside the repository root (SPEC 2).
    if !baseline_path.starts_with(path::normalize(&root)) {
        return Err(Error::new(
            "config error at baseline: the baseline must be inside the repository root",
        ));
    }

    let baseline_rel = path::relative_to(&root, &baseline_path);

    Ok(Context {
        config,
        config_path,
        root,
        baseline_path,
        baseline_rel,
        git,
    })
}

/// Finds the config file: the explicit one, or the first one in the search path.
fn find_config(cli: &Cli, cwd: &Path, git_root: Option<&Path>) -> Result<PathBuf, Error> {
    discover_config(cli, cwd, git_root)?.ok_or_else(|| {
        Error::new("no config found — expected ratchet.config.jsonc (run `ratchet init`)")
    })
}

/// The discovery of the config file; `None` when there is none.
///
/// The lookup walks up from the working directory to the git root.
fn discover_config(
    cli: &Cli,
    cwd: &Path,
    git_root: Option<&Path>,
) -> Result<Option<PathBuf>, Error> {
    if let Some(path) = &cli.config {
        if !path.is_file() {
            return Err(Error::new(format!(
                "config file not found: {}",
                path::display(git_root, path)
            )));
        }
        return Ok(Some(absolute(path, cwd)));
    }
    if cli.no_config {
        return Err(Error::new("--no-config requires --config <path>"));
    }

    let mut directory = Some(cwd.to_path_buf());
    while let Some(current) = directory {
        for name in CONFIG_FILE_NAMES {
            let candidate = current.join(name);
            if candidate.is_file() {
                return Ok(Some(candidate));
            }
        }
        if let Some(root) = git_root {
            if current == root {
                break;
            }
        }
        directory = current.parent().map(Path::to_path_buf);
    }

    Ok(None)
}

/// Loads a config file; the extension decides how it is evaluated.
fn load_config_file(path: &Path) -> Result<Config, Error> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("mjs") | Some("cjs") => Ok(config::load_js_config(path)?),
        _ => {
            let text = fs::read_to_string(path).map_err(|e| {
                Error::new(format!(
                    "cannot read {}: {e}",
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                ))
            })?;
            Ok(config::parse_config_str(&text)?)
        }
    }
}

/// Reads the committed baseline.
///
/// * no repository, or the baseline is not in `HEAD` → `None`: the
///   monotonicity check is skipped with a note.
/// * the committed baseline exists but cannot be parsed → an operational
///   error: a broken reference must not disable the gate (SPEC 5.2, 8).
fn read_committed_baseline(context: &Context) -> Result<Option<Baseline>, Error> {
    let Some(relative) = context.baseline_rel.as_ref() else {
        return Ok(None);
    };
    let Some(text) = context.git.show_head(relative) else {
        return Ok(None);
    };
    match Baseline::parse(&text, relative) {
        Ok(baseline) => Ok(Some(baseline)),
        Err(error) => Err(Error::new(format!(
            "the committed baseline is unreadable: {error}"
        ))),
    }
}

/// Prints the notes and the debug log to stderr.
fn print_notes(report: &Report, cli: &Cli) {
    for note in &report.notes {
        eprintln!("ratchet: note: {note}");
    }
    if cli.debug {
        for line in &report.debug {
            eprintln!("ratchet: debug: {line}");
        }
    }
}

/// The exit code of a check: 2 for a counter error, 1 for a failed gate.
fn exit_code(report: &Report) -> i32 {
    if report.counter_errors > 0 {
        2
    } else if report.ok {
        0
    } else {
        1
    }
}

/// The absolute path of `path`.
fn absolute(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// The directory that contains `path`.
fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
    }
}

/// The subcommands of the CLI.
const SUBCOMMANDS: [&str; 4] = ["check", "count", "baseline", "init"];

/// Inserts the default command when the first argument is not a subcommand:
/// `ratchet src/main.rs` means `ratchet check src/main.rs`.
fn normalize_args(args: Vec<OsString>) -> Vec<OsString> {
    if args.is_empty() {
        return vec![OsString::from("ratchet")];
    }

    let wants_help = args
        .iter()
        .any(|arg| arg == "--help" || arg == "-h" || arg == "--version" || arg == "-V");
    if wants_help {
        return args;
    }

    if let Some(index) = first_positional(&args) {
        let name = args[index].to_string_lossy().to_string();
        if SUBCOMMANDS.contains(&name.as_str()) {
            return args;
        }
    }

    let mut normalized = Vec::with_capacity(args.len() + 1);
    normalized.push(args[0].clone());
    normalized.push(OsString::from("check"));
    normalized.extend(args[1..].iter().cloned());
    normalized
}

/// The index of the first argument that is not an option or an option value.
fn first_positional(args: &[OsString]) -> Option<usize> {
    let mut index = 1;
    while index < args.len() {
        let argument = args[index].to_string_lossy().to_string();
        if argument == "--" {
            return if index + 1 < args.len() {
                Some(index + 1)
            } else {
                None
            };
        }
        if let Some(name) = argument.strip_prefix("--") {
            if name.contains('=') {
                index += 1;
                continue;
            }
            if matches!(name, "config" | "format" | "rule") {
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        if argument.starts_with('-') && argument.len() > 1 {
            index += 1;
            continue;
        }
        return Some(index);
    }
    None
}

/// The command line interface.
#[derive(Parser, Debug)]
#[command(
    name = "ratchet",
    version,
    about = "Ratchet gates: a count against a baseline that may only move in one direction.",
    long_about = None
)]
struct Cli {
    /// The command to run; `check` is the default.
    #[command(subcommand)]
    command: Command,

    /// Use this config file.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Do not look for a config; only valid together with --config.
    #[arg(long, global = true)]
    no_config: bool,

    /// The output format.
    #[arg(long, global = true, value_enum, default_value_t = Format::Stylish, value_name = "FORMAT")]
    format: Format,

    /// Restrict the run to one rule.
    #[arg(long, global = true, value_name = "ID")]
    rule: Option<String>,

    /// Print the resolved config and every counter invocation.
    #[arg(long, global = true)]
    debug: bool,

    /// Only print failures.
    #[arg(long, global = true, short = 'q')]
    quiet: bool,
}

/// The commands.
#[derive(Subcommand, Debug)]
enum Command {
    /// Run the gates (the default command).
    Check(CheckArgs),
    /// Print the current count per rule.
    Count(CountArgs),
    /// Read and write the baseline file.
    Baseline(BaselineArgs),
    /// Write a starter config.
    Init(InitArgs),
}

/// The arguments of `check`.
#[derive(Args, Debug)]
struct CheckArgs {
    /// Restrict the run to the rules whose scope intersects these paths.
    #[arg(value_name = "PATH")]
    paths: Vec<String>,
}

/// The arguments of `count`.
#[derive(Args, Debug)]
struct CountArgs {}

/// The arguments of `init`.
#[derive(Args, Debug)]
struct InitArgs {}

/// The arguments of `baseline`.
#[derive(Args, Debug)]
struct BaselineArgs {
    /// The baseline command.
    #[command(subcommand)]
    command: BaselineCommand,
}

/// The baseline commands.
#[derive(Subcommand, Debug)]
enum BaselineCommand {
    /// Write the current count as the baseline of every rule without one.
    Init(BaselineReasonArgs),
    /// Move the baseline towards the current count, in the allowed direction only.
    Update(BaselineReasonArgs),
}

/// The arguments of the baseline commands.
#[derive(Args, Debug)]
struct BaselineReasonArgs {
    /// The reason stored in the baseline record.
    #[arg(long, value_name = "TEXT")]
    reason: Option<String>,
}

/// The output format.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    /// Human readable.
    Stylish,
    /// Machine readable.
    Json,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn inserts_the_default_command() {
        let normalized = normalize_args(args(&["ratchet", "src/main.rs"]));
        assert_eq!(normalized[1], OsString::from("check"));
        assert_eq!(normalized[2], OsString::from("src/main.rs"));
    }

    #[test]
    fn keeps_a_known_subcommand() {
        let normalized = normalize_args(args(&["ratchet", "count"]));
        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[1], OsString::from("count"));
    }

    #[test]
    fn inserts_the_default_command_before_an_option() {
        let normalized = normalize_args(args(&["ratchet", "--config", "a.jsonc"]));
        assert_eq!(normalized[1], OsString::from("check"));
    }

    #[test]
    fn leaves_the_help_alone() {
        let normalized = normalize_args(args(&["ratchet", "--help"]));
        assert_eq!(normalized.len(), 2);
    }
}
