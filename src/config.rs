//! Config discovery, parsing and validation.
//!
//! The config is a *flat* config in the ESLint shape: an array of config
//! objects that are merged.  Parsing is hand written on purpose: every error
//! has to name the offending path (`rules.my-rule.direction`), which a
//! derive-based deserializer cannot express.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use globset::{GlobBuilder, GlobMatcher};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::registry::{SEVERITIES, STATUSES, Severity, Status};

/// Config file names, in discovery order.
pub const CONFIG_FILE_NAMES: [&str; 4] = [
    "ratchet.config.jsonc",
    "ratchet.config.json",
    "ratchet.config.mjs",
    "ratchet.config.cjs",
];

/// Default baseline file name; resolved next to the config.
pub const DEFAULT_BASELINE_FILE: &str = "ratchet.baseline.json";

/// A configuration error. `path` points at the offending field.
#[derive(Debug, Clone)]
pub struct ConfigError {
    /// Path of the offending field, e.g. `rules.my-rule.direction`.
    pub path: String,
    /// What is wrong with it.
    pub message: String,
    /// The raw output of a subprocess, if one was involved; the diagnostic
    /// channel carries it (SPEC 6.2).
    pub output: Option<String>,
}

impl ConfigError {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
            output: None,
        }
    }

    /// A configuration error with the raw output of a subprocess.
    fn with_output(path: impl Into<String>, message: impl Into<String>, output: String) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
            output: Some(output),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "config error at {}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for ConfigError {}

/// The direction a baseline is allowed to move in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// The count must not exceed the baseline.
    Ceiling,
    /// The count must not fall below the baseline.
    Floor,
}

impl Direction {
    /// The name used in configs and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Ceiling => "ceiling",
            Direction::Floor => "floor",
        }
    }
}

/// The tier a rule belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// Seconds, no infrastructure.
    Fast,
    /// Real infrastructure.
    Slow,
}

impl Tier {
    /// The name used in configs and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Fast => "fast",
            Tier::Slow => "slow",
        }
    }
}

/// How the count of a rule is produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Counter {
    /// An argv array; no shell is involved.
    Command {
        /// The program and its arguments.
        command: Vec<String>,
    },
    /// A JSON file, optionally with a dotted path into it.
    File {
        /// The file, relative to the repository root.
        file: String,
        /// A dotted path into the JSON document.
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// The generator that refreshes the artifact before it is counted.
        #[serde(skip_serializing_if = "Option::is_none")]
        generate: Option<Vec<String>>,
        /// The argument the generator writes its output to.
        #[serde(skip_serializing_if = "Option::is_none")]
        output_arg: Option<String>,
    },
    /// A findings registry; the count is the number of the findings that
    /// count (SPEC 13).
    Registry {
        /// The registry file, relative to the repository root.
        registry: String,
        /// The levels that count; every level counts without the list.
        #[serde(skip_serializing_if = "Option::is_none")]
        severities: Option<Vec<Severity>>,
        /// The statuses that count; the counting statuses count without the
        /// list.
        #[serde(skip_serializing_if = "Option::is_none")]
        statuses: Option<Vec<Status>>,
    },
}

impl Counter {
    /// A short human readable description, used by `--debug`.
    pub fn describe(&self) -> String {
        match self {
            Counter::Command { command } => command.join(" "),
            Counter::File {
                file,
                path,
                generate,
                ..
            } => {
                let mut text = file.clone();
                if let Some(path) = path {
                    text.push('#');
                    text.push_str(path);
                }
                if let Some(generate) = generate {
                    text.push_str(" (generated by ");
                    text.push_str(&generate.join(" "));
                    text.push(')');
                }
                text
            }
            Counter::Registry {
                registry,
                severities,
                statuses,
            } => {
                let mut text = registry.clone();
                let mut filters: Vec<String> = Vec::new();
                if let Some(severities) = severities {
                    filters.push(format!(
                        "severities: {}",
                        severities
                            .iter()
                            .map(|severity| severity.as_str())
                            .collect::<Vec<&str>>()
                            .join(", ")
                    ));
                }
                if let Some(statuses) = statuses {
                    filters.push(format!(
                        "statuses: {}",
                        statuses
                            .iter()
                            .map(|status| status.as_str())
                            .collect::<Vec<&str>>()
                            .join(", ")
                    ));
                }
                if !filters.is_empty() {
                    text.push_str(" (");
                    text.push_str(&filters.join("; "));
                    text.push(')');
                }
                text
            }
        }
    }
}

/// One gate, one defect class.
#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    /// The rule id (the key in the `rules` object).
    #[serde(skip)]
    pub id: String,
    /// The defect class.
    pub class: String,
    /// The incident that motivated the rule.
    ///
    /// For a class that has already occurred, the incident names that
    /// concrete instance.  For a prevention rule — a class whose count is zero
    /// today — it says so plainly (SPEC §3.2).
    pub incident: String,
    /// What one count unit is.
    pub unit: String,
    /// The allowed direction of the baseline.
    pub direction: Direction,
    /// The tier of the rule.
    pub tier: Tier,
    /// How the count is produced.
    pub count: Counter,
    /// Glob patterns removed from the counted items.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Glob scope of the rule; without it the rule always runs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Free text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Compiled `exclude` patterns.
    #[serde(skip)]
    pub exclude_globs: Vec<GlobMatcher>,
    /// Compiled `files` patterns.
    #[serde(skip)]
    pub file_globs: Vec<GlobMatcher>,
}

impl Rule {
    /// The compiled scope of the rule: the paths it covers.
    ///
    /// A rule without a `files` scope covers every path.
    pub fn in_scope(&self, path: &str) -> bool {
        self.file_globs.is_empty() || self.file_globs.iter().any(|glob| glob.is_match(path))
    }
}

/// Per tier settings.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TierSettings {
    /// The minimum number of checks that must execute in the tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub floor: Option<i64>,
}

/// A drift gate entry: a generated file and the generator that produces it.
#[derive(Debug, Clone, Serialize)]
pub struct Generated {
    /// The generated file, relative to the repository root.
    pub path: String,
    /// The generator command.
    pub command: Vec<String>,
    /// The argument the generator writes its output to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_arg: Option<String>,
    /// The tier of the drift gate; `fast` by default.
    ///
    /// A generator that needs a database, a browser or the network belongs to
    /// the slow tier: a fast tier that waits for it is a fast tier that is
    /// not fast (SPEC §7).
    pub tier: Tier,
}

/// A pre-release check: a command whose output is compared with a committed
/// snapshot (SPEC §15).
///
/// A benchmark is a measurement, not a defect class: it has no meaningful
/// "baseline that may only sink".  The value moves in both directions and
/// only matters at the moment of a release, so the check has **no direction**.
#[derive(Debug, Clone, Serialize)]
pub struct Verify {
    /// The name of the check; it identifies the entry in the report.
    pub name: String,
    /// The command that produces the output.
    pub command: Vec<String>,
    /// The committed snapshot, relative to the repository root.
    pub snapshot: String,
    /// The argument the command writes its output to.
    ///
    /// The argv gets the temporary output path appended — the flag, then the
    /// path — so the command writes where the tool tells it and the tree
    /// stays untouched (SPEC §15).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_arg: Option<String>,
}

/// A merged config.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Config {
    /// Optional label of the config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Per tier settings.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tiers: BTreeMap<Tier, TierSettings>,
    /// Drift gate entries.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub generated: Vec<Generated>,
    /// The pre-release checks.
    ///
    /// They run in the release pipeline, not in the commit gate: a measurement
    /// is not a defect class (SPEC §15).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub verify: Vec<Verify>,
    /// The rules, sorted by id.
    pub rules: BTreeMap<String, Rule>,
    /// The baseline path, relative to the config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<String>,
}

/// Parses a JSONC (or plain JSON) config.
pub fn parse_config_str(text: &str) -> Result<Config, ConfigError> {
    let value: Value =
        json5::from_str(text).map_err(|e| ConfigError::new("", format!("invalid JSON: {e}")))?;
    parse_config_value(&value)
}

/// Validates and merges a config document.
pub fn parse_config_value(value: &Value) -> Result<Config, ConfigError> {
    let objects: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![value],
        other => {
            return Err(ConfigError::new(
                "",
                format!(
                    "expected an array of config objects, found {}",
                    type_name(other)
                ),
            ));
        }
    };

    let mut config = Config::default();
    for object in objects {
        merge_object(&mut config, object)?;
    }
    Ok(config)
}

/// Evaluates a JavaScript config with `node` and parses its default export.
pub fn load_js_config(path: &Path) -> Result<Config, ConfigError> {
    let absolute = absolute_path(path);
    let name = file_name(path);

    // A backslash cannot survive the file URL: node rejects the
    // percent-encoded backslash.  The failure is closed and names the cause.
    if absolute.to_string_lossy().contains('\\') {
        return Err(ConfigError::new(
            "",
            format!(
                "{name} cannot be evaluated: the path contains a backslash, \
                 which the JavaScript bridge cannot load — use ratchet.config.jsonc instead"
            ),
        ));
    }

    // The path is resolved as a file URL: a path that contains `#`, `?` or `%`
    // is not a valid URL specifier.  The path itself is passed as one argv
    // element, it is never interpolated into a shell.
    let script = format!(
        "const {{ pathToFileURL }} = await import('node:url'); \
         const module = await import(pathToFileURL({}).href); \
         process.stdout.write(JSON.stringify(module.default ?? module));",
        serde_json::to_string(&absolute.to_string_lossy()).unwrap_or_else(|_| "\"\"".to_string())
    );

    let output = Command::new("node")
        .arg("--input-type=module")
        .arg("-e")
        .arg(&script)
        .current_dir(absolute.parent().unwrap_or(Path::new(".")))
        .output();

    let output = match output {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ConfigError::new(
                "",
                format!(
                    "node was not found on PATH — {name} cannot be evaluated, use ratchet.config.jsonc instead"
                ),
            ));
        }
        Err(e) => {
            return Err(ConfigError::new("", format!("cannot run node: {e}")));
        }
    };

    // The transcript of node: the diagnostic channel carries the raw output of
    // the subprocess — both streams, in one shape (SPEC 6.2).
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = match output.status.code() {
        Some(code) => format!("status {code}"),
        None => "an unknown status".to_string(),
    };
    let transcript = format!("node: {status}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");

    if !output.status.success() {
        // Node prints the absolute path of the config and the working
        // directory.  That belongs to the diagnostic channel, never to the
        // report (SPEC 6.2).
        return Err(ConfigError::with_output(
            "",
            format!("{name} could not be evaluated (node exited with {status})"),
            transcript,
        ));
    }

    if stdout.trim().is_empty() {
        return Err(ConfigError::with_output(
            "",
            format!("{name} must export a config object"),
            transcript,
        ));
    }

    // The export is not valid JSON: node's output belongs to the diagnostic
    // channel (SPEC 6.2), the report says what happened (SPEC 6.2).
    let value: Value = serde_json::from_str(stdout.trim()).map_err(|e| {
        ConfigError::with_output(
            "",
            format!("{name} did not export valid JSON: {e}"),
            transcript,
        )
    })?;
    parse_config_value(&value)
}

/// The file name of a path; error messages stay free of absolute paths.
fn file_name(path: &Path) -> String {
    crate::path::file_name(path)
}

/// The absolute path of `path`, without resolving symlinks.
fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn merge_object(config: &mut Config, value: &Value) -> Result<(), ConfigError> {
    let map = expect_object(value, "")?;
    check_unknown(
        map,
        &["name", "tiers", "generated", "verify", "rules", "baseline"],
        "",
    )?;

    if let Some(name) = map.get("name") {
        config.name = Some(expect_string(name, "name")?);
    }
    if let Some(baseline) = map.get("baseline") {
        config.baseline = Some(expect_string(baseline, "baseline")?);
    }
    if let Some(tiers) = map.get("tiers") {
        merge_tiers(config, tiers)?;
    }
    if let Some(generated) = map.get("generated") {
        merge_generated(config, generated)?;
    }
    if let Some(verify) = map.get("verify") {
        merge_verify(config, verify)?;
    }
    if let Some(rules) = map.get("rules") {
        merge_rules(config, rules)?;
    }
    Ok(())
}

fn merge_tiers(config: &mut Config, value: &Value) -> Result<(), ConfigError> {
    let map = expect_object(value, "tiers")?;
    for (name, settings) in map {
        let tier = match name.as_str() {
            "fast" => Tier::Fast,
            "slow" => Tier::Slow,
            other => {
                return Err(ConfigError::new(
                    format!("tiers.{other}"),
                    "expected \"fast\" or \"slow\"",
                ));
            }
        };
        let path = format!("tiers.{name}");
        let settings_map = expect_object(settings, &path)?;
        check_unknown(settings_map, &["floor"], &path)?;
        let floor = match settings_map.get("floor") {
            None => None,
            Some(floor) => Some(expect_integer(floor, &format!("{path}.floor"))?),
        };
        config.tiers.insert(tier, TierSettings { floor });
    }
    Ok(())
}

fn merge_generated(config: &mut Config, value: &Value) -> Result<(), ConfigError> {
    let items = expect_array(value, "generated")?;
    for (index, item) in items.iter().enumerate() {
        let path = format!("generated[{index}]");
        let map = expect_object(item, &path)?;
        check_unknown(map, &["path", "command", "output_arg", "tier"], &path)?;
        let file = require_string(map, "path", &path)?;
        let command = require_string_array(map, "command", &path)?;
        if command.is_empty() {
            return Err(ConfigError::new(
                format!("{path}.command"),
                "must not be empty",
            ));
        }
        let output_arg = match map.get("output_arg") {
            None => None,
            Some(value) => Some(expect_string(value, &format!("{path}.output_arg"))?),
        };
        let tier = match map.get("tier") {
            None => Tier::Fast,
            Some(value) => match expect_string(value, &format!("{path}.tier"))?.as_str() {
                "fast" => Tier::Fast,
                "slow" => Tier::Slow,
                other => {
                    return Err(ConfigError::new(
                        format!("{path}.tier"),
                        format!("expected \"fast\" or \"slow\", found \"{other}\""),
                    ));
                }
            },
        };
        let entry = Generated {
            path: file,
            command,
            output_arg,
            tier,
        };
        // A later entry for the same path overrides the earlier one.
        match config.generated.iter_mut().find(|g| g.path == entry.path) {
            Some(existing) => *existing = entry,
            None => config.generated.push(entry),
        }
    }
    Ok(())
}

/// Merges the pre-release checks (SPEC §15).
fn merge_verify(config: &mut Config, value: &Value) -> Result<(), ConfigError> {
    let items = expect_array(value, "verify")?;
    // The names of this array: a name twice in one array is a mistake, and a
    // mistake that silently drops a check is a false green.
    let mut names: Vec<String> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let path = format!("verify[{index}]");
        let map = expect_object(item, &path)?;
        check_unknown(map, &["name", "command", "snapshot", "output_arg"], &path)?;

        let name = require_string(map, "name", &path)?;
        if name.trim().is_empty() {
            return Err(ConfigError::new(
                format!("{path}.name"),
                "must not be empty",
            ));
        }
        if names.contains(&name) {
            return Err(ConfigError::new(
                format!("{path}.name"),
                format!("the name \"{name}\" is used twice — every check has its own name"),
            ));
        }
        names.push(name.clone());
        let command = require_string_array(map, "command", &path)?;
        if command.is_empty() {
            return Err(ConfigError::new(
                format!("{path}.command"),
                "must not be empty",
            ));
        }
        let snapshot = require_string(map, "snapshot", &path)?;
        if snapshot.trim().is_empty() {
            return Err(ConfigError::new(
                format!("{path}.snapshot"),
                "must not be empty",
            ));
        }
        let output_arg = match map.get("output_arg") {
            None => None,
            Some(value) => Some(expect_string(value, &format!("{path}.output_arg"))?),
        };

        // The placeholder in the argv and the `output_arg` are two ways to
        // name the output path.  A command that uses both does not say where
        // its output goes, and the tool would tell it twice.
        if output_arg.is_some()
            && command
                .iter()
                .any(|part| part.contains(crate::verify::PLACEHOLDER))
        {
            return Err(ConfigError::new(
                format!("{path}.output_arg"),
                format!(
                    "the command carries the {} placeholder and the entry declares an \
                     `output_arg` — one of them names the output path",
                    crate::verify::PLACEHOLDER
                ),
            ));
        }

        let entry = Verify {
            name,
            command,
            snapshot,
            output_arg,
        };
        // A later config object overrides an earlier entry with the same name.
        match config.verify.iter_mut().find(|v| v.name == entry.name) {
            Some(existing) => *existing = entry,
            None => config.verify.push(entry),
        }
    }
    Ok(())
}

fn merge_rules(config: &mut Config, value: &Value) -> Result<(), ConfigError> {
    let map = expect_object(value, "rules")?;
    for (id, rule) in map {
        // A later config object overrides an earlier one for the same rule id.
        config.rules.insert(id.clone(), parse_rule(id, rule)?);
    }
    Ok(())
}

fn parse_rule(id: &str, value: &Value) -> Result<Rule, ConfigError> {
    let path = format!("rules.{id}");
    let map = expect_object(value, &path)?;
    check_unknown(
        map,
        &[
            "class",
            "incident",
            "unit",
            "direction",
            "tier",
            "count",
            "exclude",
            "files",
            "description",
        ],
        &path,
    )?;

    let class = require_string(map, "class", &path)?;
    let incident = require_string(map, "incident", &path)?;
    let unit = require_string(map, "unit", &path)?;
    // An empty string is a missing value that happens to pass the type check.
    for (field, value) in [("class", &class), ("incident", &incident), ("unit", &unit)] {
        if value.trim().is_empty() {
            return Err(ConfigError::new(
                format!("{path}.{field}"),
                "must not be empty",
            ));
        }
    }
    let direction = match require_string(map, "direction", &path)?.as_str() {
        "ceiling" => Direction::Ceiling,
        "floor" => Direction::Floor,
        other => {
            return Err(ConfigError::new(
                format!("{path}.direction"),
                format!("expected \"ceiling\" or \"floor\", found \"{other}\""),
            ));
        }
    };
    let tier = match map.get("tier") {
        None => Tier::Fast,
        Some(value) => match expect_string(value, &format!("{path}.tier"))?.as_str() {
            "fast" => Tier::Fast,
            "slow" => Tier::Slow,
            other => {
                return Err(ConfigError::new(
                    format!("{path}.tier"),
                    format!("expected \"fast\" or \"slow\", found \"{other}\""),
                ));
            }
        },
    };
    let count = match map.get("count") {
        None => {
            return Err(ConfigError::new(
                format!("{path}.count"),
                "missing required field",
            ));
        }
        Some(value) => parse_counter(value, &format!("{path}.count"))?,
    };
    let exclude = optional_string_array(map, "exclude", &path)?;
    let files = optional_string_array(map, "files", &path)?;
    let description = match map.get("description") {
        None => None,
        Some(value) => Some(expect_string(value, &format!("{path}.description"))?),
    };
    let exclude_globs = compile_globs(&exclude, &format!("{path}.exclude"))?;
    let file_globs = compile_globs(&files, &format!("{path}.files"))?;

    Ok(Rule {
        id: id.to_string(),
        class,
        incident,
        unit,
        direction,
        tier,
        count,
        exclude,
        files,
        description,
        exclude_globs,
        file_globs,
    })
}

fn parse_counter(value: &Value, path: &str) -> Result<Counter, ConfigError> {
    let map = expect_object(value, path)?;

    let has_command = map.contains_key("command");
    let has_file = map.contains_key("file");
    let has_registry = map.contains_key("registry");
    match (has_command, has_file, has_registry) {
        (true, false, false) => {
            check_unknown(map, &["command"], path)?;
            let command = require_string_array(map, "command", path)?;
            if command.is_empty() {
                return Err(ConfigError::new(
                    format!("{path}.command"),
                    "must not be empty",
                ));
            }
            Ok(Counter::Command { command })
        }
        (false, true, false) => {
            check_unknown(map, &["file", "path", "generate", "output_arg"], path)?;
            let file = require_string(map, "file", path)?;
            let json_path = match map.get("path") {
                None => None,
                Some(value) => Some(expect_string(value, &format!("{path}.path"))?),
            };
            let generate = match map.get("generate") {
                None => None,
                Some(value) => {
                    let items = expect_array(value, &format!("{path}.generate"))?;
                    let mut command = Vec::with_capacity(items.len());
                    for (index, item) in items.iter().enumerate() {
                        command.push(expect_string(item, &format!("{path}.generate[{index}]"))?);
                    }
                    if command.is_empty() {
                        return Err(ConfigError::new(
                            format!("{path}.generate"),
                            "must not be empty",
                        ));
                    }
                    Some(command)
                }
            };
            let output_arg = match map.get("output_arg") {
                None => None,
                Some(value) => Some(expect_string(value, &format!("{path}.output_arg"))?),
            };
            if output_arg.is_some() && generate.is_none() {
                return Err(ConfigError::new(
                    format!("{path}.output_arg"),
                    "requires \"generate\"",
                ));
            }
            if generate.is_some() && output_arg.is_none() {
                // A generator that cannot write to a given path would have to
                // write into the tree — and the tree is not the engine's to
                // write (SPEC 3.4).
                return Err(ConfigError::new(
                    format!("{path}.output_arg"),
                    "is required by \"generate\" — the generator writes to a temporary path",
                ));
            }
            Ok(Counter::File {
                file,
                path: json_path,
                generate,
                output_arg,
            })
        }
        (false, false, true) => {
            check_unknown(map, &["registry", "severities", "statuses"], path)?;
            let registry = require_string(map, "registry", path)?;
            if registry.trim().is_empty() {
                return Err(ConfigError::new(
                    format!("{path}.registry"),
                    "must not be empty",
                ));
            }
            let severities = optional_severities(map, "severities", path)?;
            let statuses = optional_statuses(map, "statuses", path)?;
            Ok(Counter::Registry {
                registry,
                severities,
                statuses,
            })
        }
        _ => Err(ConfigError::new(
            path,
            "expected exactly one of \"command\", \"file\" or \"registry\"",
        )),
    }
}

/// The levels of a registry counter; every level counts without the list.
fn optional_severities(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<Vec<Severity>>, ConfigError> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => {
            let items = expect_array(value, &format!("{path}.{key}"))?;
            if items.is_empty() {
                // A list that selects nothing is a gate that counts nothing:
                // it would be green forever.
                return Err(ConfigError::new(
                    format!("{path}.{key}"),
                    "must not be empty",
                ));
            }
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let text = expect_string(item, &format!("{path}.{key}[{index}]"))?;
                let severity = Severity::parse(&text).ok_or_else(|| {
                    ConfigError::new(
                        format!("{path}.{key}[{index}]"),
                        format!(
                            "unknown severity \"{text}\" — expected one of {}",
                            SEVERITIES.join(", ")
                        ),
                    )
                })?;
                out.push(severity);
            }
            Ok(Some(out))
        }
    }
}

/// The statuses of a registry counter; the counting statuses count without
/// the list.
fn optional_statuses(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<Vec<Status>>, ConfigError> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => {
            let items = expect_array(value, &format!("{path}.{key}"))?;
            if items.is_empty() {
                return Err(ConfigError::new(
                    format!("{path}.{key}"),
                    "must not be empty",
                ));
            }
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let text = expect_string(item, &format!("{path}.{key}[{index}]"))?;
                let status = Status::parse(&text).ok_or_else(|| {
                    ConfigError::new(
                        format!("{path}.{key}[{index}]"),
                        format!(
                            "unknown status \"{text}\" — expected one of {}",
                            STATUSES.join(", ")
                        ),
                    )
                })?;
                out.push(status);
            }
            Ok(Some(out))
        }
    }
}

/// Compiles glob patterns into matchers.
///
/// The patterns are the ones of a config (`exclude`, `files`) and the ones of
/// the command line (`--ignore`); the same rules apply to all of them.
pub fn compile_globs(patterns: &[String], path: &str) -> Result<Vec<GlobMatcher>, ConfigError> {
    let mut compiled = Vec::new();
    for original in patterns {
        for pattern in glob_patterns(original) {
            let glob = GlobBuilder::new(&pattern)
                .literal_separator(true)
                .build()
                .map_err(|e| {
                    ConfigError::new(path, format!("invalid glob pattern \"{original}\": {e}"))
                })?;
            compiled.push(glob.compile_matcher());
        }
    }
    Ok(compiled)
}

/// The patterns a glob stands for.
///
/// `**` matches zero or more path segments (SPEC 3.7).  Globset already
/// handles that in the middle of a pattern (`a/**/b` matches `a/b`); at the end
/// of a pattern it does not: `src/**` has to match the directory `src` as
/// well, so the directory is added as a pattern of its own.
fn glob_patterns(pattern: &str) -> Vec<String> {
    let mut patterns = vec![pattern.to_string()];
    if let Some(prefix) = pattern.strip_suffix("/**") {
        if !prefix.is_empty() {
            patterns.push(prefix.to_string());
        }
    }
    patterns
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn expect_object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, ConfigError> {
    value.as_object().ok_or_else(|| {
        ConfigError::new(
            path,
            format!("expected an object, found {}", type_name(value)),
        )
    })
}

fn expect_array<'a>(value: &'a Value, path: &str) -> Result<&'a Vec<Value>, ConfigError> {
    value.as_array().ok_or_else(|| {
        ConfigError::new(
            path,
            format!("expected an array, found {}", type_name(value)),
        )
    })
}

fn expect_string(value: &Value, path: &str) -> Result<String, ConfigError> {
    value.as_str().map(str::to_string).ok_or_else(|| {
        ConfigError::new(
            path,
            format!("expected a string, found {}", type_name(value)),
        )
    })
}

fn expect_integer(value: &Value, path: &str) -> Result<i64, ConfigError> {
    value.as_i64().ok_or_else(|| {
        ConfigError::new(
            path,
            format!("expected an integer, found {}", type_name(value)),
        )
    })
}

fn require_string_array(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Vec<String>, ConfigError> {
    let value = map
        .get(key)
        .ok_or_else(|| ConfigError::new(format!("{path}.{key}"), "missing required field"))?;
    let items = expect_array(value, &format!("{path}.{key}"))?;
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        out.push(expect_string(item, &format!("{path}.{key}[{index}]"))?);
    }
    Ok(out)
}

fn optional_string_array(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Vec<String>, ConfigError> {
    match map.get(key) {
        None => Ok(Vec::new()),
        Some(value) => {
            let items = expect_array(value, &format!("{path}.{key}"))?;
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                out.push(expect_string(item, &format!("{path}.{key}[{index}]"))?);
            }
            Ok(out)
        }
    }
}

fn require_string(map: &Map<String, Value>, key: &str, path: &str) -> Result<String, ConfigError> {
    let value = map
        .get(key)
        .ok_or_else(|| ConfigError::new(format!("{path}.{key}"), "missing required field"))?;
    expect_string(value, &format!("{path}.{key}"))
}

fn check_unknown(
    map: &Map<String, Value>,
    allowed: &[&str],
    path: &str,
) -> Result<(), ConfigError> {
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            let field = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            return Err(ConfigError::new(field, "unknown field"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_jsonc_config_with_comments() {
        let text = r#"
            // the only source of truth
            [
              {
                "name": "noema",
                /* a block comment */
                "rules": {
                  "unwrapped-action-calls": {
                    "class": "layer violation",
                    "incident": "F-09",
                    "unit": "call sites",
                    "direction": "ceiling",
                    "tier": "fast",
                    "count": { "command": ["node", "scripts/audit.mjs", "--print"] },
                  },
                },
              },
            ]
        "#;
        let config = parse_config_str(text).expect("config parses");
        assert_eq!(config.name.as_deref(), Some("noema"));
        let rule = config
            .rules
            .get("unwrapped-action-calls")
            .expect("rule exists");
        assert_eq!(rule.direction, Direction::Ceiling);
        assert_eq!(rule.tier, Tier::Fast);
        assert_eq!(
            rule.count,
            Counter::Command {
                command: vec!["node".into(), "scripts/audit.mjs".into(), "--print".into()]
            }
        );
    }

    #[test]
    fn merges_config_objects_and_overrides_rules_by_id() {
        let text = r#"[
            { "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
                                "direction": "ceiling", "count": { "command": ["echo", "1"] } } } },
            { "rules": { "a": { "class": "c2", "incident": "i", "unit": "u",
                                "direction": "floor", "count": { "command": ["echo", "2"] } } } }
        ]"#;
        let config = parse_config_str(text).expect("config parses");
        assert_eq!(config.rules.len(), 1);
        let rule = config.rules.get("a").expect("rule exists");
        assert_eq!(rule.class, "c2");
        assert_eq!(rule.direction, Direction::Floor);
    }

    #[test]
    fn rejects_unknown_fields_with_a_path() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling", "count": { "command": ["echo", "1"] }, "surprise": 1 } } }]"#;
        let error = parse_config_str(text).expect_err("unknown field is an error");
        assert_eq!(error.path, "rules.a.surprise");
    }

    #[test]
    fn rejects_an_invalid_direction_with_a_path() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "sideways", "count": { "command": ["echo", "1"] } } } }]"#;
        let error = parse_config_str(text).expect_err("invalid direction is an error");
        assert_eq!(error.path, "rules.a.direction");
    }

    #[test]
    fn rejects_a_counter_with_both_command_and_file() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling", "count": { "command": ["echo", "1"], "file": "x.json" } } } }]"#;
        let error = parse_config_str(text).expect_err("both counters is an error");
        assert_eq!(error.path, "rules.a.count");
    }

    #[test]
    fn rejects_a_path_on_a_command_counter() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling", "count": { "command": ["echo", "1"], "path": "x" } } } }]"#;
        let error = parse_config_str(text).expect_err("a path on a command is an error");
        assert_eq!(error.path, "rules.a.count.path");
    }

    #[test]
    fn rejects_a_missing_required_field() {
        let text = r#"[{ "rules": { "a": { "class": "c", "unit": "u",
            "direction": "ceiling", "count": { "command": ["echo", "1"] } } } }]"#;
        let error = parse_config_str(text).expect_err("missing field is an error");
        assert_eq!(error.path, "rules.a.incident");
    }

    #[test]
    fn parses_the_file_counter_with_a_dotted_path() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling",
            "count": { "file": "docs/_audit-raw.json", "path": "coherence.innerHtml" } } } }]"#;
        let config = parse_config_str(text).expect("config parses");
        let rule = config.rules.get("a").expect("rule exists");
        assert_eq!(
            rule.count,
            Counter::File {
                file: "docs/_audit-raw.json".into(),
                path: Some("coherence.innerHtml".into()),
                generate: None,
                output_arg: None
            }
        );
    }

    #[test]
    fn parses_the_registry_counter() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling",
            "count": { "registry": "ratchet.findings.json",
                       "severities": ["critical", "high"],
                       "statuses": ["open", "in-progress"] } } } }]"#;
        let config = parse_config_str(text).expect("config parses");
        let rule = config.rules.get("a").expect("rule exists");
        assert_eq!(
            rule.count,
            Counter::Registry {
                registry: "ratchet.findings.json".into(),
                severities: Some(vec![Severity::Critical, Severity::High]),
                statuses: Some(vec![Status::Open, Status::InProgress]),
            }
        );
    }

    #[test]
    fn a_registry_counter_without_the_lists_counts_everything() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling", "count": { "registry": "ratchet.findings.json" } } } }]"#;
        let config = parse_config_str(text).expect("config parses");
        let rule = config.rules.get("a").expect("rule exists");
        assert_eq!(
            rule.count,
            Counter::Registry {
                registry: "ratchet.findings.json".into(),
                severities: None,
                statuses: None,
            }
        );
    }

    #[test]
    fn rejects_an_unknown_severity_in_the_rule() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling",
            "count": { "registry": "ratchet.findings.json", "severities": ["blocker"] } } } }]"#;
        let error = parse_config_str(text).expect_err("an unknown level is an error");
        assert_eq!(error.path, "rules.a.count.severities[0]");
    }

    #[test]
    fn rejects_an_unknown_status_in_the_rule() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling",
            "count": { "registry": "ratchet.findings.json", "statuses": ["doing"] } } } }]"#;
        let error = parse_config_str(text).expect_err("an unknown status is an error");
        assert_eq!(error.path, "rules.a.count.statuses[0]");
    }

    #[test]
    fn rejects_a_counter_with_a_registry_and_a_file() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling",
            "count": { "registry": "ratchet.findings.json", "file": "x.json" } } } }]"#;
        let error = parse_config_str(text).expect_err("two counters is an error");
        assert_eq!(error.path, "rules.a.count");
    }

    #[test]
    fn a_rule_without_files_always_runs() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling", "count": { "command": ["echo", "1"] } } } }]"#;
        let config = parse_config_str(text).expect("config parses");
        let rule = config.rules.get("a").expect("rule exists");
        assert!(rule.in_scope("src/other.rs"));
    }

    #[test]
    fn a_scoped_rule_runs_only_for_paths_in_its_scope() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling", "count": { "command": ["echo", "1"] },
            "files": ["src/**"] } } }]"#;
        let config = parse_config_str(text).expect("config parses");
        let rule = config.rules.get("a").expect("rule exists");
        assert!(rule.in_scope("src/app/main.rs"));
        assert!(!rule.in_scope("docs/readme.md"));
    }

    #[test]
    fn a_double_star_matches_the_directory_itself() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling", "count": { "command": ["echo", "1"] },
            "files": ["src/**"] } } }]"#;
        let config = parse_config_str(text).expect("config parses");
        let rule = config.rules.get("a").expect("rule exists");
        // `src/**` matches `src` and everything below it (SPEC 3.7).
        assert!(rule.in_scope("src"));
        assert!(rule.in_scope("src/"));
        assert!(rule.in_scope("src/nested/deep/file.rs"));
        assert!(!rule.in_scope("srcs/file.rs"));
        assert!(!rule.in_scope("docs/readme.md"));
    }

    #[test]
    fn rejects_an_invalid_glob_pattern() {
        let text = r#"[{ "rules": { "a": { "class": "c", "incident": "i", "unit": "u",
            "direction": "ceiling", "count": { "command": ["echo", "1"] },
            "exclude": ["src/["] } } }]"#;
        let error = parse_config_str(text).expect_err("invalid glob is an error");
        assert_eq!(error.path, "rules.a.exclude");
    }

    #[test]
    fn loads_a_javascript_config_when_node_is_available() {
        if !node_available() {
            eprintln!(
                "*** SKIPPED: node is not on the PATH — this JavaScript test did NOT run. \
                 Install node (or run the CI, which installs it) to exercise it. ***"
            );
            return;
        }
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ratchet.config.mjs");
        let config = load_js_config(&path).expect("the js config loads");
        assert_eq!(config.name.as_deref(), Some("fixture"));
        // The rule of the fixture proves that the file was really evaluated.
        let rule = config
            .rules
            .get("js-rule")
            .expect("the rule of the fixture is loaded");
        assert_eq!(rule.direction, Direction::Ceiling);
        assert_eq!(rule.class, "layer violation");
    }

    #[test]
    fn a_javascript_config_in_a_path_with_url_characters_loads() {
        if !node_available() {
            eprintln!(
                "*** SKIPPED: node is not on the PATH — this JavaScript test did NOT run. \
                 Install node (or run the CI, which installs it) to exercise it. ***"
            );
            return;
        }
        // `#`, `?` and `%` are not valid in a URL specifier: the bridge has to
        // resolve the path as a file URL.
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("ratchet#config?100%.mjs");
        std::fs::write(
            &path,
            "export default [{ rules: { a: { class: \"c\", incident: \"i\", unit: \"u\", \
             direction: \"ceiling\", count: { command: [\"echo\", \"1\"] } } } }];\n",
        )
        .expect("the file is written");

        let config = load_js_config(&path).expect("the js config loads");

        assert!(config.rules.contains_key("a"));
    }

    fn node_available() -> bool {
        Command::new("node")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}
