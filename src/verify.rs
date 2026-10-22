//! The pre-release check: a command whose output is compared with a committed
//! snapshot (SPEC §15).
//!
//! A **benchmark is a measurement, not a defect class**.  It has no meaningful
//! "baseline that may only sink" — the value moves in both directions and
//! only matters at the moment of a release.  So the pre-release check is **not**
//! a ratchet: it has **no direction**.  It runs in the release pipeline, not
//! in the commit gate: it needs the infrastructure the release needs, and a
//! permanent gate over a measurement would be noise.
//!
//! The mechanic is the drift gate (SPEC §7) with a different comparison: the
//! drift gate compares a *generated file* with its committed counterpart, the
//! pre-release check compares a *command output* with a committed snapshot.
//!
//! The engine never mutates the tree: the command writes to a temporary path
//! and the snapshot is only written by `--update`.  The snapshot itself — the
//! one file the engine owns — is restored after the command, byte-, mode- and
//! time-identical.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::Error;
use crate::config::{Config, Verify};
use crate::diff;
use crate::fsutil;
use crate::git::{Git, HeadEntry};
use crate::path;

/// The placeholder in the argv that names the output path.
///
/// The command tells the engine where to write by carrying the placeholder;
/// the engine replaces it with the temporary output path — **wherever it
/// appears in an argument**, so `--output=<out>` names the output as well.
pub const PLACEHOLDER: &str = "<out>";

/// The status of one pre-release check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyStatus {
    /// The committed snapshot is what the command produces.
    Ok,
    /// The committed snapshot differs from what the command produces.
    Drift,
    /// The snapshot is not committed yet.
    New,
}

impl VerifyStatus {
    /// The name used in the JSON report.
    pub fn as_str(self) -> &'static str {
        match self {
            VerifyStatus::Ok => "ok",
            VerifyStatus::Drift => "drift",
            VerifyStatus::New => "new",
        }
    }

    /// True when the check failed.
    pub fn is_failure(self) -> bool {
        matches!(self, VerifyStatus::Drift)
    }
}

/// The outcome of one pre-release check.
#[derive(Debug, Clone)]
pub struct VerifyEntry {
    /// The name of the check.
    pub name: String,
    /// The status of the check.
    pub status: VerifyStatus,
    /// The unified diff (committed → produced), if the snapshot drifted.
    pub diff: Option<String>,
    /// A human readable explanation, if there is something to say.
    pub detail: Option<String>,
}

/// What a verification run does.
#[derive(Debug)]
pub struct Options<'a> {
    /// The repository root.
    pub root: &'a Path,
    /// Write the produced output into the snapshots.
    pub update: bool,
    /// The one check to run, by name.
    pub name: Option<&'a str>,
}

/// The result of a verification run.
#[derive(Debug, Default)]
pub struct Outcome {
    /// One entry per check, sorted by name.
    pub entries: Vec<VerifyEntry>,
    /// Notes about what was skipped and why.
    pub notes: Vec<String>,
    /// The debug log (`--debug`).
    pub debug: Vec<String>,
}

impl Outcome {
    /// The number of checks whose snapshot differs from the output.
    pub fn drifted(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.status.is_failure())
            .count()
    }
}

/// Runs the pre-release checks of the `verify` entries of the config.
pub fn run(config: &Config, git: &Git, options: &Options) -> Result<Outcome, Error> {
    let mut outcome = Outcome::default();

    if !git.is_repo() {
        // The committed comparisons are skipped; the checks still run (SPEC 8).
        outcome
            .notes
            .push("the verify comparison is skipped: not a git repository".to_string());
    }

    // The entries are sorted by name: the report is a stable diff.
    let mut entries: Vec<&Verify> = config.verify.iter().collect();
    entries.sort_by(|left, right| left.name.cmp(&right.name));

    let mut selected = 0;
    for entry in entries {
        // The snapshot lies inside the repository root: a path that points
        // outside it would let the engine write outside the tree it owns.
        let subject = format!("the check {}: the snapshot {}", entry.name, entry.snapshot);
        let resolved =
            path::inside_repository(options.root, &entry.snapshot).map_err(|refusal| {
                Error::new(refusal.message(&subject, "must lie inside the repository root"))
            })?;
        if let Some(filter) = options.name {
            if filter != entry.name {
                continue;
            }
        }
        selected += 1;
        let result = run_one(entry, &resolved, git, options, &mut outcome.debug)?;
        outcome.entries.push(result);
    }

    if let Some(filter) = options.name {
        if selected == 0 {
            return Err(Error::new(format!("no check for {filter}")));
        }
    }

    Ok(outcome)
}

/// Runs one pre-release check.
fn run_one(
    entry: &Verify,
    resolved: &Path,
    git: &Git,
    options: &Options,
    debug: &mut Vec<String>,
) -> Result<VerifyEntry, Error> {
    let root = options.root;
    let snapshot = root.join(&entry.snapshot);
    let display = path::display(Some(root), &snapshot);
    let name = entry.name.as_str();

    // The snapshot has to be a regular file: the committed content of a
    // symbolic link is its target, and comparing the *output* with the
    // target's content would say nothing about the output.  The drift gate
    // resolves a link instead — there the *file* is the subject, here the
    // *output* is (SPEC §15).
    if let Ok(metadata) = fs::symlink_metadata(&snapshot) {
        if metadata.file_type().is_symlink() {
            return Err(Error::new(format!(
                "the check {name}: the snapshot {display} is a symbolic link — \
                 the snapshot has to be a regular file"
            )));
        }
        if metadata.is_dir() {
            return Err(Error::new(format!(
                "the check {name}: the snapshot {display} is a directory, not a file"
            )));
        }
    } else if let Ok(metadata) = fs::symlink_metadata(resolved) {
        // The named path can be unwalkable even though its resolved target
        // exists (for example `a/..` when `a` is missing). A directory at the
        // resolved file position is still not a snapshot file.
        if metadata.is_dir() {
            return Err(Error::new(format!(
                "the check {name}: the snapshot {display} is a directory, not a file"
            )));
        }
    }

    // The snapshot is the one file the engine owns: its bytes, its mode and
    // its time are restored after the command, whatever the command did to
    // it.  A command that writes elsewhere in the tree is the command's
    // doing — the engine cannot undo that.
    let created = command_directories(root, &entry.snapshot, resolved);
    let mut guard = Guard::take(resolved, root, created)
        .map_err(|error| Error::new(format!("the check {name}: cannot read {display}: {error}")))?;

    // The marker: the file is touched with an old time, so that the engine
    // can tell whether the command wrote it — even when it wrote the same
    // bytes over it.
    guard.mark();

    let produced = produce(entry, root, debug);

    // The restore happens before the error is reported: a command that wrote
    // into the tree must not leave the snapshot behind in a changed state.
    if let Err(error) = guard.restore() {
        return Err(Error::new(format!(
            "the check {name}: the restore of {display} failed — the working tree was modified: \
             {error}"
        )));
    }

    let produced = produced?;

    // The comparison is against the committed snapshot (SPEC §15).
    let (status, committed) = compare(git, name, &entry.snapshot, &produced)?;

    let mut detail = None;
    let mut diff = None;

    match status {
        VerifyStatus::Ok => {}
        VerifyStatus::New => {
            detail = Some("not committed yet".to_string());
        }
        VerifyStatus::Drift => {
            let committed = committed.unwrap_or_default();
            diff = Some(diff::unified(&entry.snapshot, &committed, &produced));
            detail = Some("differs from the committed snapshot".to_string());
        }
    }

    // `--update` writes the produced output into the snapshot: the snapshot
    // is machine-written, never hand-edited (SPEC §15).  The check stays
    // **failed**: the committed snapshot is still stale until the file is
    // committed — the same rule the drift gate follows (SPEC §7).
    if options.update && write_snapshot(name, root, &display, resolved, &produced)? {
        detail = Some(match status {
            VerifyStatus::Drift => "the snapshot was written to the working tree; commit it — \
                 the committed snapshot is still stale"
                .to_string(),
            VerifyStatus::New => "the snapshot was written to the working tree; it is not \
                 committed yet"
                .to_string(),
            VerifyStatus::Ok => "the snapshot was written to the working tree".to_string(),
        });
    }

    Ok(VerifyEntry {
        name: entry.name.clone(),
        status,
        diff,
        detail,
    })
}

/// Runs the command of a check and returns the bytes it produced.
///
/// Three ways to say where the output goes:
///
/// * with `output_arg` — the argv gets the temporary output path appended:
///   the flag, then the path;
/// * with the `<out>` placeholder in the argv — the placeholder is replaced
///   by the temporary output path, **wherever it appears in an argument**:
///   `--output=<out>` and `<out>.txt` name the output as well;
/// * without either — the **stdout** of the command is the output.
///
/// The command runs with the working directory set to the repository root, and
/// it writes into a temporary directory: the tree is not the engine's to
/// write.
fn produce(entry: &Verify, root: &Path, debug: &mut Vec<String>) -> Result<Vec<u8>, Error> {
    let temporary = fsutil::Temporary::new("verify").map_err(|error| {
        Error::new(format!(
            "the check {} could not be started: {error}",
            entry.name
        ))
    })?;
    let name = path::file_name(Path::new(&entry.snapshot));
    let output = temporary.path().join(name);
    let output_path = output.to_string_lossy().into_owned();

    let mut argv = entry.command.clone();
    // The placeholder is replaced wherever it appears: a command that writes
    // `--output=<out>` has to write to the temporary file, or the engine
    // would compare the command's stdout with the snapshot and report a
    // green that means nothing.
    let placeholder = argv.iter().any(|argument| argument.contains(PLACEHOLDER));
    if placeholder {
        argv = argv
            .iter()
            .map(|argument| argument.replace(PLACEHOLDER, &output_path))
            .collect();
    }
    if let Some(output_arg) = &entry.output_arg {
        argv.push(output_arg.clone());
        argv.push(output_path.clone());
    }

    let (program, arguments) = argv.split_first().ok_or_else(|| {
        Error::new(format!(
            "the check {} could not be started: the command is empty",
            entry.name
        ))
    })?;

    let result = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|error| {
            Error::with_output(
                format!("the check {} could not be started: {error}", entry.name),
                format!("{}: could not be started\n{error}", argv.join(" ")),
            )
        })?;

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    let status = match result.status.code() {
        Some(code) => format!("status {code}"),
        None => "an unknown status".to_string(),
    };
    // The transcript shows the argv that was really executed — it belongs to
    // the diagnostic channel, never to the report (SPEC 6.2).
    let transcript = format!(
        "{}: {status}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        argv.join(" ")
    );
    debug.push(format!("check {}: {transcript}", entry.name));

    if !result.status.success() {
        return Err(Error::with_output(
            format!("the check {} exited with {status}", entry.name),
            transcript,
        ));
    }

    // The output goes to a file: the command was told where to write it.
    if placeholder || entry.output_arg.is_some() {
        return read_output(entry, temporary.path(), &transcript);
    }

    // Without an output path the stdout of the command is the output.
    Ok(result.stdout)
}

/// Reads the file the command wrote into the temporary directory.
///
/// The command was told where to write: the temporary directory holds the
/// output and nothing else.  A command that names the output with a suffix
/// (`<out>.txt`) writes a file under another name — the directory is the
/// contract, not the file name.
///
/// * nothing in the directory → the command wrote no output;
/// * more than one file → the output is ambiguous, and a guess would be a
///   comparison that means nothing.
fn read_output(entry: &Verify, temporary: &Path, transcript: &str) -> Result<Vec<u8>, Error> {
    let mut files: Vec<PathBuf> = match fs::read_dir(temporary) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect(),
        Err(error) => {
            return Err(Error::with_output(
                format!("the check {} wrote no output", entry.name),
                format!("{transcript}\ncannot read the temporary directory: {error}"),
            ));
        }
    };
    // The files are sorted: the same tree produces the same bytes.
    files.sort();

    match files.len() {
        0 => Err(Error::with_output(
            format!("the check {} wrote no output", entry.name),
            format!("{transcript}\nthe command wrote nothing into the temporary directory"),
        )),
        1 => fs::read(&files[0]).map_err(|error| {
            Error::with_output(
                format!("the check {} wrote no output", entry.name),
                format!("{transcript}\ncannot read the output of the check: {error}"),
            )
        }),
        count => Err(Error::with_output(
            format!(
                "the check {} wrote {count} files — the output is ambiguous",
                entry.name
            ),
            format!(
                "{transcript}\nthe command wrote more than one file into the temporary directory"
            ),
        )),
    }
}

/// Compares the produced output with the committed snapshot.
///
/// * no repository → the comparison is skipped, the snapshot counts as `new`;
/// * the snapshot is not in the committed tree → `new`, informational and not
///   a failure;
/// * the committed path is a directory → an operational error;
/// * the committed snapshot cannot be read → an operational error.  A broken
///   reference must not disable a check (SPEC 8).
///
/// The message names the check **and** the path: whoever reads the error has
/// to know which check failed and which file it is about.
fn compare(
    git: &Git,
    name: &str,
    relative: &str,
    produced: &[u8],
) -> Result<(VerifyStatus, Option<Vec<u8>>), Error> {
    if !git.is_repo() {
        return Ok((VerifyStatus::New, None));
    }
    match git.head_entry(relative) {
        HeadEntry::Missing => Ok((VerifyStatus::New, None)),
        HeadEntry::Directory => Err(Error::new(format!(
            "the check {name}: the committed snapshot {relative} is a directory, not a file"
        ))),
        HeadEntry::Unreadable => Err(Error::new(format!(
            "the check {name}: the committed snapshot {relative} cannot be read"
        ))),
        HeadEntry::Blob(object) => {
            let committed = git.show_object(&object).ok_or_else(|| {
                Error::new(format!(
                    "the check {name}: the committed snapshot {relative} cannot be read"
                ))
            })?;
            if committed == produced {
                Ok((VerifyStatus::Ok, Some(committed)))
            } else {
                Ok((VerifyStatus::Drift, Some(committed)))
            }
        }
    }
}

/// Writes the produced output into the snapshot.
///
/// Returns true when the file was written: a snapshot that already holds the
/// produced bytes is left alone, and a check that is in sync does not touch
/// the tree.
fn write_snapshot(
    name: &str,
    root: &Path,
    display: &str,
    target: &Path,
    produced: &[u8],
) -> Result<bool, Error> {
    if let Ok(current) = fs::read(target) {
        if current == produced {
            return Ok(false);
        }
    }
    if let Some(parent) = target.parent() {
        fsutil::create_directories(parent).map_err(|error| {
            let directory = path::display(Some(root), &error.directory);
            let rollback = error.rollback_message(|path| path::display(Some(root), path));
            Error::new(format!(
                "the check {name}: cannot create the snapshot directory {directory}: {}{rollback}",
                error.cause
            ))
        })?;
    }
    fs::write(target, produced).map_err(|error| {
        Error::new(format!("the check {name}: cannot write {display}: {error}"))
    })?;
    Ok(true)
}

/// Directories the command could have created along the snapshot's named
/// traversal or at the resolved target. Paths are ordered deepest-first for
/// removal, and paths behind a link are taken from the resolved target only.
fn command_directories(root: &Path, relative: &str, resolved: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(parent) = resolved.parent() {
        directories.extend(fsutil::missing_directories(parent));
    }

    let components: Vec<_> = Path::new(relative).components().collect();
    let mut prefix = PathBuf::new();
    let mut through_link = false;
    for (index, component) in components.iter().enumerate() {
        match component {
            Component::Normal(name) => {
                prefix.push(name);
                if index + 1 < components.len() {
                    let directory = root.join(&prefix);
                    if path::is_link(&directory) {
                        through_link = true;
                    }
                    if !through_link && !directory.is_dir() {
                        directories.extend(fsutil::missing_directories(&directory));
                    }
                }
            }
            Component::ParentDir => {
                prefix.pop();
            }
            Component::CurDir => {}
            _ => {}
        }
    }

    directories.sort_by(|left, right| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| left.cmp(right))
    });
    directories.dedup();
    directories
}

/// The state of the snapshot file: its bytes, its mode, its time and its
/// inode.
#[derive(Debug, Default)]
struct State {
    /// The bytes of the file, if it existed.
    content: Option<Vec<u8>>,
    /// The mode of the file, on the platforms that have one.
    mode: Option<u32>,
    /// The time the file was written last, if it existed.
    modified: Option<SystemTime>,
    /// The inode of the file, on the platforms that have one.
    inode: Option<u64>,
}

impl State {
    /// Reads the state of `path`.
    ///
    /// A file that is not there has no state: every field is empty.
    fn read(path: &Path) -> Self {
        let Ok(content) = fs::read(path) else {
            return State::default();
        };
        let metadata = fs::metadata(path).ok();
        State {
            content: Some(content),
            mode: metadata.as_ref().and_then(fsutil::mode_of),
            modified: metadata
                .as_ref()
                .and_then(|metadata| metadata.modified().ok()),
            inode: metadata.as_ref().and_then(fsutil::inode_of),
        }
    }
}

/// The snapshot of the snapshot file: its bytes, its mode and its time.
///
/// The command of a check runs with the repository root as its working
/// directory, and a command that writes into the tree cannot be undone.  The
/// one file the engine owns — the snapshot — is protected: it is restored
/// after the command, and the restore is verified afterwards.
struct Guard {
    /// The path of the snapshot.
    target: PathBuf,
    /// The repository root, used to keep cleanup errors repository-relative.
    root: PathBuf,
    /// The state of the file before the command ran.
    before: State,
    /// The time the engine touched the file with, if it touched it.
    marker: Option<SystemTime>,
    /// The directories that do not exist yet below the target.
    created: Vec<PathBuf>,
}

impl Guard {
    /// Takes the snapshot of `target`.
    fn take(target: &Path, root: &Path, created: Vec<PathBuf>) -> io::Result<Self> {
        let content = match fs::read(target) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let metadata = fs::metadata(target).ok();
        let before = State {
            content,
            mode: metadata.as_ref().and_then(fsutil::mode_of),
            modified: metadata
                .as_ref()
                .and_then(|metadata| metadata.modified().ok()),
            inode: metadata.as_ref().and_then(fsutil::inode_of),
        };
        Ok(Guard {
            target: target.to_path_buf(),
            root: root.to_path_buf(),
            before,
            marker: None,
            created,
        })
    }

    /// Touches the file with an old time, so that the engine can tell whether
    /// the command wrote it.
    ///
    /// The signal is the same as the drift gate's: a command that rewrites
    /// the file with the **same bytes** updates the time, and a command that
    /// leaves the file alone does not.  The time is far enough in the past
    /// that every write lands after it — on every filesystem, whatever its
    /// timestamp granularity is.
    fn mark(&mut self) {
        if self.before.content.is_none() {
            // The file is not there: there is nothing to touch.
            return;
        }
        let marker = fsutil::marker_time();
        if fsutil::set_modified(&self.target, marker).is_ok() {
            self.marker = Some(marker);
        }
    }

    /// Restores the snapshot and verifies the restore.
    fn restore(&self) -> Result<(), String> {
        let after = State::read(&self.target);
        let restored = if !self.was_written(&after) {
            // The command left the file alone: only the marker has to go.
            self.restore_time(&after);
            Ok(())
        } else {
            match &self.before.content {
                Some(content) => (|| {
                    fsutil::write_atomic(
                        &self.target,
                        content,
                        self.before.mode,
                        self.before.modified,
                    )
                    .map_err(|error| error.to_string())?;
                    let read = fs::read(&self.target).map_err(|error| error.to_string())?;
                    if &read != content {
                        return Err("the bytes differ from the snapshot".to_string());
                    }
                    if let Some(mode) = self.before.mode {
                        let metadata =
                            fs::metadata(&self.target).map_err(|error| error.to_string())?;
                        if fsutil::mode_of(&metadata) != Some(mode) {
                            return Err("the mode differs from the snapshot".to_string());
                        }
                    }
                    Ok(())
                })(),
                None => (|| {
                    // The file did not exist before: the command created it.
                    if fs::symlink_metadata(&self.target).is_ok() {
                        fs::remove_file(&self.target).map_err(|error| error.to_string())?;
                    }
                    Ok(())
                })(),
            }
        };

        // The directories the command created are removed on every path: an
        // empty directory left behind is a change of the tree.
        let removed = self.remove_created_directories();
        match (restored, removed) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(restore), Ok(())) => Err(restore),
            (Ok(()), Err(remove)) => Err(remove),
            (Err(restore), Err(remove)) => Err(format!("{restore}; {remove}")),
        }
    }

    /// True when the command wrote the file.
    ///
    /// The bytes, the mode and the inode are compared, and the time is
    /// compared with the **marker**: the engine touched the file with a time
    /// far enough in the past before the command ran, so a current time means
    /// the command wrote it.  A command that **replaces** the file (`cp -p`
    /// and `mv`, the reproducible-build pattern) leaves a new inode behind,
    /// and the inode survives a preserved time.
    fn was_written(&self, after: &State) -> bool {
        if after.content != self.before.content {
            return true;
        }
        if after.mode != self.before.mode {
            return true;
        }
        if after.inode != self.before.inode {
            return true;
        }
        match (self.marker, after.modified) {
            // The time is the marker's: the command left the file alone.
            (Some(marker), Some(modified)) => modified != marker,
            // The file is gone: the command removed it.
            (Some(_), None) => true,
            // There was no marker: there is no time to compare.
            (None, _) => false,
        }
    }

    /// Puts the original time back.
    ///
    /// The time is restored on a best-effort basis: the bytes and the mode
    /// are what the tree owes, and a filesystem that refuses the time must
    /// not fail the restore.
    fn restore_time(&self, after: &State) {
        let (Some(before), Some(current)) = (self.before.modified, after.modified) else {
            return;
        };
        if before == current {
            return;
        }
        let _ = fsutil::set_modified(&self.target, before);
    }

    /// Removes the directories the command created and left empty.
    fn remove_created_directories(&self) -> Result<(), String> {
        let failures = fsutil::remove_empty_directories(&self.created);
        if failures.is_empty() {
            return Ok(());
        }
        let failures = failures
            .iter()
            .map(|(directory, error)| {
                format!("{}: {error}", path::display(Some(&self.root), directory))
            })
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!("could not remove the directory {failures}"))
    }
}
