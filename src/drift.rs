//! The drift gates: a generated file that differs from what its generator
//! produces (SPEC §7).
//!
//! Two generator modes exist:
//!
//! * with `output_arg` — the generator writes to a **temporary path**; the
//!   tree is never touched;
//! * without `output_arg` — the generator writes the file **in place**.  The
//!   engine then owns a **verified restore**: the original bytes and the file
//!   mode are snapshotted, the generator runs, the produced bytes are read and
//!   the file is restored — and the restore is verified afterwards.  A restore
//!   that fails ends the run with an operational error that names the file and
//!   says that the working tree was modified: a swallowed restore error is a
//!   mutated tree that nobody was told about.
//!
//! The comparison is against the **committed** file (`git show HEAD:<path>`),
//! not against the working-tree copy: the gate asks whether the repository
//! holds what the generator produces.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use crate::Error;
use crate::config::{Config, Generated, Tier};
use crate::diff;
use crate::generate::Generator;
use crate::git::{Git, HeadEntry};
use crate::path;

/// The status of one generated file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftStatus {
    /// The committed file is what the generator produces.
    Ok,
    /// The committed file differs from what the generator produces.
    Drift,
    /// The file is not committed yet.
    New,
}

impl DriftStatus {
    /// The name used in the JSON report.
    pub fn as_str(self) -> &'static str {
        match self {
            DriftStatus::Ok => "ok",
            DriftStatus::Drift => "drift",
            DriftStatus::New => "new",
        }
    }

    /// True when the generated file is a failed gate.
    pub fn is_failure(self) -> bool {
        matches!(self, DriftStatus::Drift)
    }
}

/// The outcome of one drift gate.
#[derive(Debug, Clone)]
pub struct DriftEntry {
    /// The generated file, relative to the repository root.
    pub path: String,
    /// The status of the gate.
    pub status: DriftStatus,
    /// The unified diff (committed → produced), if the gate drifted.
    pub diff: Option<String>,
    /// A human readable explanation, if there is something to say.
    pub detail: Option<String>,
    /// The tier of the drift gate.
    ///
    /// A drift gate is a **check** of its tier: it counts towards the floor
    /// of that tier (SPEC 3.3).
    pub tier: Tier,
}

/// What a drift run does.
#[derive(Debug)]
pub struct Options<'a> {
    /// The repository root.
    pub root: &'a Path,
    /// Write the produced content into the generated files.
    pub fix: bool,
    /// The one generated file to run, relative to the repository root.
    pub path: Option<&'a str>,
    /// The one tier to run; `None` runs every tier.
    pub tier: Option<Tier>,
}

/// The result of a drift run.
#[derive(Debug, Default)]
pub struct Outcome {
    /// One entry per generated file, sorted by path.
    pub entries: Vec<DriftEntry>,
    /// Notes about what was skipped and why.
    pub notes: Vec<String>,
    /// The debug log (`--debug`).
    pub debug: Vec<String>,
}

impl Outcome {
    /// The number of generated files that drifted.
    pub fn drifted(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.status.is_failure())
            .count()
    }
}

/// Runs the drift gates of the `generated` entries of the config.
pub fn run(config: &Config, git: &Git, options: &Options) -> Result<Outcome, Error> {
    let mut outcome = Outcome::default();

    if !git.is_repo() {
        // The committed comparisons are skipped; the gate still runs (SPEC 8).
        outcome
            .notes
            .push("drift comparison skipped: not a git repository".to_string());
    }

    // The entries are sorted by path: the report is a stable diff.
    let mut entries: Vec<&Generated> = config.generated.iter().collect();
    entries.sort_by(|left, right| left.path.cmp(&right.path));

    let mut selected = 0;
    for entry in entries {
        // The generated file lies inside the repository root: a path that
        // points outside it would let the engine write outside the tree it
        // owns.
        if !inside(options.root, &entry.path) {
            return Err(Error::new(format!(
                "the generated file {} must lie inside the repository root",
                entry.path
            )));
        }
        if let Some(filter) = options.path {
            if filter != entry.path {
                continue;
            }
        }
        // A slow generator does not run in the fast tier: a fast tier that
        // waits for a database, a browser or the network is not fast (SPEC 7).
        if let Some(tier) = options.tier {
            if entry.tier != tier {
                outcome.notes.push(format!(
                    "the drift gate {} is skipped: it belongs to the tier `{}`",
                    entry.path,
                    entry.tier.as_str()
                ));
                continue;
            }
        }
        selected += 1;
        let result = run_one(entry, git, options, &mut outcome.debug)?;
        outcome.entries.push(result);
    }

    if let Some(filter) = options.path {
        if selected == 0 {
            return Err(Error::new(format!("no drift gate for {filter}")));
        }
    }

    Ok(outcome)
}

/// Runs one drift gate.
fn run_one(
    entry: &Generated,
    git: &Git,
    options: &Options,
    debug: &mut Vec<String>,
) -> Result<DriftEntry, Error> {
    let root = options.root;
    let target = root.join(&entry.path);

    let produced = match &entry.output_arg {
        Some(output_arg) => {
            // The generator writes to a temporary path: the tree is untouched.
            let generator = Generator {
                command: entry.command.clone(),
                output_arg: output_arg.clone(),
            };
            let artifact = generator
                .run(Path::new(&entry.path), root)
                .map_err(|error| Error::with_output(error.message, error.output))?;
            debug.push(format!("generator {}: {}", entry.path, artifact.output));
            artifact.bytes
        }
        None => produce_in_place(entry, root, options, debug)?,
    };

    // The comparison is against the committed file (SPEC §7).
    let comparison = comparison_path(root, &entry.path);
    let (status, committed) = compare(git, &comparison, &produced)?;

    let mut detail = None;
    let mut diff = None;

    match status {
        DriftStatus::Ok => {}
        DriftStatus::New => {
            detail = Some("not committed yet".to_string());
        }
        DriftStatus::Drift => {
            let committed = committed.unwrap_or_default();
            diff = Some(diff::unified(&entry.path, &committed, &produced));
            detail = Some("differs from the committed file".to_string());
        }
    }

    // `--fix` writes the produced content into the generated file — and only
    // into the generated files.  The in-place generator has already written
    // it; the temporary-path generator has not.
    //
    // The gate stays **failed**: the gate measures the *committed* artifact,
    // and the committed file is still stale until the fix is committed.  A
    // run with `--fix` in a pipeline is therefore never green against a stale
    // HEAD; that is deliberate, not a bug (SPEC 7).
    if options.fix {
        match status {
            DriftStatus::Drift => {
                if entry.output_arg.is_some() {
                    write_fixed(root, &target, &produced)?;
                }
                detail = Some(
                    "the produced content was written to the working tree; commit it — the committed file is still stale"
                        .to_string(),
                );
            }
            DriftStatus::New => {
                let written = if entry.output_arg.is_some() {
                    let current = fs::read(&target).ok();
                    if current.as_deref() != Some(produced.as_slice()) {
                        write_fixed(root, &target, &produced)?;
                        true
                    } else {
                        false
                    }
                } else {
                    // The in-place generator has written the file.
                    true
                };
                if written {
                    detail = Some(
                        "the produced content was written to the working tree; the file is not committed yet"
                            .to_string(),
                    );
                }
            }
            DriftStatus::Ok => {}
        }
    }

    Ok(DriftEntry {
        path: entry.path.clone(),
        status,
        diff,
        detail,
        tier: entry.tier,
    })
}

/// True when the path lies inside the repository root.
fn inside(root: &Path, relative: &str) -> bool {
    let path = Path::new(relative);
    if path.is_absolute() {
        return false;
    }
    path::normalize(&root.join(path)).starts_with(path::normalize(root))
}

/// The path the comparison is made against.
///
/// A symbolic link is compared **through**: the committed content of a link
/// is its target, and the produced content is the content behind the link.
/// Comparing the link text with the content would report a drift that says
/// nothing about the file.
fn comparison_path(root: &Path, relative: &str) -> String {
    let target = root.join(relative);
    let Ok(metadata) = fs::symlink_metadata(&target) else {
        return relative.to_string();
    };
    if !metadata.file_type().is_symlink() {
        return relative.to_string();
    }
    let Ok(raw) = fs::read_link(&target) else {
        return relative.to_string();
    };
    match path::relative_to(root, &resolve(&target, &raw)) {
        Some(resolved) => resolved,
        None => relative.to_string(),
    }
}

/// Compares the produced content with the committed file.
///
/// * no repository → the comparison is skipped, the file counts as `new`;
/// * the file is not in the committed tree → `new`, informational and not a
///   failure;
/// * the committed path is a directory → an operational error;
/// * the committed file cannot be read → an operational error.  A broken
///   reference must not disable a gate (SPEC 8).
fn compare(
    git: &Git,
    relative: &str,
    produced: &[u8],
) -> Result<(DriftStatus, Option<Vec<u8>>), Error> {
    if !git.is_repo() {
        return Ok((DriftStatus::New, None));
    }
    match git.head_entry(relative) {
        HeadEntry::Missing => Ok((DriftStatus::New, None)),
        HeadEntry::Directory => Err(Error::new(format!(
            "the committed path {relative} is a directory, not a file"
        ))),
        HeadEntry::Unreadable => Err(Error::new(format!(
            "the committed file {relative} cannot be read"
        ))),
        HeadEntry::Blob(object) => {
            let committed = git.show_object(&object).ok_or_else(|| {
                Error::new(format!("the committed file {relative} cannot be read"))
            })?;
            if committed == produced {
                Ok((DriftStatus::Ok, Some(committed)))
            } else {
                Ok((DriftStatus::Drift, Some(committed)))
            }
        }
    }
}

/// Runs a generator that writes the file **in place** and returns the produced
/// bytes.
///
/// The engine owns a verified restore: whatever the generator did, the tree is
/// byte- and mode-identical afterwards — unless `--fix` asked for the produced
/// content to stay.
fn produce_in_place(
    entry: &Generated,
    root: &Path,
    options: &Options,
    debug: &mut Vec<String>,
) -> Result<Vec<u8>, Error> {
    let target = root.join(&entry.path);
    let display = path::display(Some(root), &target);

    // 1. the snapshot: the original bytes, the file mode and the time the
    //    file was written last.
    let snapshot = Snapshot::take(&target)
        .map_err(|error| Error::new(format!("cannot read {display}: {error}")))?;

    // 1b. the marker: the file is touched with an old time, so that the
    //     engine can tell whether the generator wrote it — whatever the
    //     timestamp granularity of the filesystem is.
    let marker = marker_time();
    let _ = set_modified(&target, marker);

    // 2. the generator.
    let run = run_generator(&entry.command, root, &display, debug);

    // 3. the produced bytes.
    let produced = fs::read(&target).ok();

    // 3b. did the generator write the file?  A generator that leaves it alone
    //     produced nothing: the comparison below would compare the committed
    //     file with itself and report a green that says nothing.
    let wrote = snapshot.was_written(&target, produced.as_deref(), marker);

    // 4. the restore — and 5. its verification.
    let restored = if options.fix {
        snapshot.keep()
    } else {
        snapshot.restore()
    };

    // A failed restore is the more important error: the working tree was
    // modified and nobody was told about it.
    if let Err(error) = restored {
        return Err(Error::new(format!(
            "the restore of {display} failed — the working tree was modified: {error}"
        )));
    }

    if let Err(failure) = run {
        return Err(Error::with_output(failure.message, failure.transcript));
    }

    let produced = produced.ok_or_else(|| {
        Error::new(format!(
            "the generator for {display} did not write its output"
        ))
    })?;

    if !wrote {
        return Err(Error::new(format!(
            "the generator for {display} produced nothing — it did not write the file"
        )));
    }

    Ok(produced)
}

/// Writes the produced content into the generated file.
///
/// The mode of an existing file is kept: `--fix` changes the content of the
/// generated files, nothing else.
fn write_fixed(root: &Path, target: &Path, produced: &[u8]) -> Result<(), Error> {
    let display = path::display(Some(root), target);
    if target.exists() {
        // Writing in place keeps the mode of the file.
        fs::write(target, produced)
            .map_err(|error| Error::new(format!("cannot write {display}: {error}")))?;
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Error::new(format!("cannot write {display}: {error}")))?;
    }
    fs::write(target, produced)
        .map_err(|error| Error::new(format!("cannot write {display}: {error}")))
}

/// A generator that did not succeed.
struct Failure {
    /// What happened, in ratchet's own words.
    message: String,
    /// The raw output of the generator; the diagnostic channel carries it.
    transcript: String,
}

/// Runs a generator in the repository root.
fn run_generator(
    command: &[String],
    root: &Path,
    display: &str,
    debug: &mut Vec<String>,
) -> Result<(), Failure> {
    let (program, arguments) = command.split_first().ok_or_else(|| Failure {
        message: format!("the generator for {display} could not be started: the command is empty"),
        transcript: "the generator command is empty".to_string(),
    })?;

    let output = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|error| Failure {
            message: format!("the generator for {display} could not be started: {error}"),
            transcript: format!("{}: could not be started\n{error}", command.join(" ")),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = match output.status.code() {
        Some(code) => format!("status {code}"),
        None => "an unknown status".to_string(),
    };
    // The transcript shows the argv that was really executed — it belongs to
    // the diagnostic channel, never to the report (SPEC 6.2).
    let transcript = format!(
        "{}: {status}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        command.join(" ")
    );
    debug.push(format!("generator {display}: {transcript}"));

    if !output.status.success() {
        return Err(Failure {
            message: format!("the generator for {display} exited with {status}"),
            transcript,
        });
    }

    Ok(())
}

/// The snapshot of a generated file: its bytes, its mode and the directories
/// around it.
struct Snapshot {
    /// The path of the generated file.
    target: PathBuf,
    /// The original bytes, if the file existed.
    content: Option<Vec<u8>>,
    /// The time the file was written last, if it existed.
    modified: Option<SystemTime>,
    /// The inode of the file, on the platforms that have one.
    ///
    /// The inode survives a preserved time: a generator that replaces the
    /// file leaves a new one behind.
    inode: Option<u64>,
    /// The original mode, on the platforms that have one.
    mode: Option<u32>,
    /// The symbolic link the target pointed to, if it was one.
    link: Option<Link>,
    /// The directories the generator created.
    created: Vec<PathBuf>,
}

/// The symbolic link a generated file pointed to.
struct Link {
    /// The raw target of the link.
    raw: PathBuf,
    /// The path of the file the link points to.
    resolved: PathBuf,
    /// The bytes of that file, if it existed.
    content: Option<Vec<u8>>,
    /// The time that file was written last, if it existed.
    modified: Option<SystemTime>,
    /// The inode of that file, on the platforms that have one.
    inode: Option<u64>,
    /// The mode of that file, on the platforms that have one.
    mode: Option<u32>,
}

impl Snapshot {
    /// Takes the snapshot of `target`.
    fn take(target: &Path) -> io::Result<Self> {
        let created = created_directories(target);
        match fs::symlink_metadata(target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Snapshot {
                target: target.to_path_buf(),
                content: None,
                modified: None,
                inode: None,
                mode: None,
                link: None,
                created,
            }),
            Err(error) => Err(error),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let raw = fs::read_link(target)?;
                let resolved = resolve(target, &raw);
                let (content, modified, inode, mode) = match fs::metadata(&resolved) {
                    Ok(metadata) => (
                        Some(fs::read(&resolved)?),
                        metadata.modified().ok(),
                        inode_of(&metadata),
                        mode_of(&metadata),
                    ),
                    Err(_) => (None, None, None, None),
                };
                Ok(Snapshot {
                    target: target.to_path_buf(),
                    content: None,
                    modified: None,
                    inode: None,
                    mode: None,
                    link: Some(Link {
                        raw,
                        resolved,
                        content,
                        modified,
                        inode,
                        mode,
                    }),
                    created,
                })
            }
            Ok(metadata) => Ok(Snapshot {
                target: target.to_path_buf(),
                content: Some(fs::read(target)?),
                modified: metadata.modified().ok(),
                inode: inode_of(&metadata),
                mode: mode_of(&metadata),
                link: None,
                created,
            }),
        }
    }

    /// True when the file was written since the snapshot.
    ///
    /// The bytes and the time are both compared: a generator that rewrites
    /// the same bytes updates the time, and a generator that leaves the file
    /// alone updates neither.  Without the time the engine could not tell
    /// "the generator produced this" from "the generator did not run".
    ///
    /// The time is compared with the **marker**: the engine touched the file
    /// with a time far enough in the past before the generator ran, so a
    /// current time means the generator wrote the file — on every
    /// filesystem, whatever its timestamp granularity is.
    fn was_written(&self, target: &Path, produced: Option<&[u8]>, marker: SystemTime) -> bool {
        let Ok(metadata) = fs::metadata(target) else {
            return false;
        };
        let Some(before) = self.modified() else {
            // The file did not exist before: it exists now, so it was written.
            return true;
        };
        let now = metadata.modified().ok();
        if now != Some(before) && now != Some(marker) {
            return true;
        }
        // The time is the same — but a generator that **replaces** the file
        // (`cp -p` and `mv`, the reproducible-build pattern) leaves a new
        // inode behind, and the inode survives a preserved time.
        if inode_of(&metadata) != self.inode() {
            return true;
        }
        produced != self.bytes()
    }

    /// The bytes of the file behind the target, if it existed.
    fn bytes(&self) -> Option<&[u8]> {
        match &self.link {
            Some(link) => link.content.as_deref(),
            None => self.content.as_deref(),
        }
    }

    /// The time the file behind the target was written last, if it existed.
    fn modified(&self) -> Option<SystemTime> {
        match &self.link {
            Some(link) => link.modified,
            None => self.modified,
        }
    }

    /// The inode of the file behind the target, if it existed.
    fn inode(&self) -> Option<u64> {
        match &self.link {
            Some(link) => link.inode,
            None => self.inode,
        }
    }

    /// Restores the snapshot and verifies the restore.
    fn restore(&self) -> Result<(), String> {
        match &self.link {
            Some(link) => {
                // The file the link pointed to: the generator may have written
                // through the link, and the restore undoes that.  The rename
                // replaces that file, it never writes through the link.
                //
                // The link itself is recreated: a rename over the link would
                // replace it with a regular file and change the tree — and the
                // tree must not change (SPEC 7).
                if let Some(content) = &link.content {
                    write_atomic(&link.resolved, content, link.mode, link.modified)
                        .map_err(|error| error.to_string())?;
                }
                remove_path(&self.target).map_err(|error| error.to_string())?;
                create_symlink(&link.raw, &self.target).map_err(|error| error.to_string())?;
            }
            None => match &self.content {
                Some(content) => write_atomic(&self.target, content, self.mode, self.modified)
                    .map_err(|error| error.to_string())?,
                None => {
                    // The file did not exist before: the generator created it.
                    if fs::symlink_metadata(&self.target).is_ok() {
                        fs::remove_file(&self.target).map_err(|error| error.to_string())?;
                    }
                }
            },
        }
        self.remove_created_directories()?;
        self.verify()
    }

    /// Keeps the produced content but restores the mode of the file.
    ///
    /// This is what `--fix` asks for: the content of the generated file is
    /// the produced one, everything else stays as it was.
    fn keep(&self) -> Result<(), String> {
        if let (Some(mode), None) = (self.mode, &self.link) {
            let metadata = fs::metadata(&self.target).map_err(|error| error.to_string())?;
            if mode_of(&metadata) != Some(mode) {
                set_mode(&self.target, mode).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    /// Removes the directories the generator created and left empty.
    fn remove_created_directories(&self) -> Result<(), String> {
        for directory in &self.created {
            // The deepest directory comes first: an empty parent can only be
            // removed after its child is gone.
            match fs::read_dir(directory) {
                Ok(mut entries) => {
                    if entries.next().is_none() {
                        let _ = fs::remove_dir(directory);
                    }
                }
                Err(_) => continue,
            }
        }
        Ok(())
    }

    /// Verifies the restore: the bytes are read back and compared.
    fn verify(&self) -> Result<(), String> {
        match &self.link {
            Some(link) => {
                let metadata =
                    fs::symlink_metadata(&self.target).map_err(|error| error.to_string())?;
                if !metadata.file_type().is_symlink() {
                    return Err("the target is no longer a symbolic link".to_string());
                }
                let raw = fs::read_link(&self.target).map_err(|error| error.to_string())?;
                if raw != link.raw {
                    return Err("the symbolic link points somewhere else".to_string());
                }
                if let Some(content) = &link.content {
                    let read = fs::read(&link.resolved).map_err(|error| error.to_string())?;
                    if &read != content {
                        return Err("the bytes differ from the snapshot".to_string());
                    }
                }
                Ok(())
            }
            None => match &self.content {
                Some(content) => {
                    let read = fs::read(&self.target).map_err(|error| error.to_string())?;
                    if &read != content {
                        return Err("the bytes differ from the snapshot".to_string());
                    }
                    if let Some(mode) = self.mode {
                        let metadata =
                            fs::metadata(&self.target).map_err(|error| error.to_string())?;
                        if mode_of(&metadata) != Some(mode) {
                            return Err("the mode differs from the snapshot".to_string());
                        }
                    }
                    Ok(())
                }
                None => {
                    if fs::symlink_metadata(&self.target).is_ok() {
                        Err("the file was not removed".to_string())
                    } else {
                        Ok(())
                    }
                }
            },
        }
    }
}

/// Writes `content` to `path` through a temporary file in the same directory.
///
/// The temporary file carries the original mode and time and is then
/// **renamed** over the target: a rename replaces a symbolic link instead of
/// writing through it, and it does not need write permission on the target
/// itself.
fn write_atomic(
    path: &Path,
    content: &[u8],
    mode: Option<u32>,
    modified: Option<SystemTime>,
) -> io::Result<()> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let temporary = directory.join(temporary_name(path));

    let result = (|| -> io::Result<()> {
        fs::write(&temporary, content)?;
        if let Some(mode) = mode {
            set_mode(&temporary, mode)?;
        }
        if let Some(modified) = modified {
            // The time is restored on a best-effort basis: the bytes and the
            // mode are what the tree owes, and a filesystem that refuses the
            // time must not fail the restore.
            let _ = set_modified(&temporary, modified);
        }
        fs::rename(&temporary, path)?;
        Ok(())
    })();

    if result.is_err() {
        // The temporary file must not stay behind: it would be a change of
        // the tree that nobody asked for.
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// A time far enough in the past that every write lands after it.
///
/// The engine touches the generated file with this time before the generator
/// runs: a generator that writes the file sets a current time, and the
/// difference between the two tells the engine that the file was written —
/// on every filesystem, whatever its timestamp granularity is.
fn marker_time() -> SystemTime {
    SystemTime::now() - Duration::from_secs(3600)
}

/// Sets the time the file was written last.
fn set_modified(path: &Path, time: SystemTime) -> io::Result<()> {
    let file = fs::File::open(path)?;
    let times = fs::FileTimes::new().set_modified(time);
    file.set_times(times)
}

/// The name of the temporary file of a restore.
///
/// The name carries the process id and a counter: two restores in the same
/// run never collide, and the file is gone before the run ends.
fn temporary_name(path: &Path) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let number = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    format!(".ratchet-restore-{}-{number}-{name}", std::process::id())
}

/// The directories that do not exist yet below the target.
fn created_directories(target: &Path) -> Vec<PathBuf> {
    let mut missing = Vec::new();
    let mut current = target.parent();
    while let Some(directory) = current {
        if directory.as_os_str().is_empty() {
            break;
        }
        if !directory.exists() {
            missing.push(directory.to_path_buf());
        } else {
            break;
        }
        current = directory.parent();
    }
    missing
}

/// The path a symbolic link points to.
fn resolve(target: &Path, raw: &Path) -> PathBuf {
    if raw.is_absolute() {
        path::normalize(raw)
    } else {
        let directory = target.parent().unwrap_or_else(|| Path::new("."));
        path::normalize(&directory.join(raw))
    }
}

/// Removes a file or a symbolic link.
fn remove_path(target: &Path) -> io::Result<()> {
    match fs::symlink_metadata(target) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(metadata) if metadata.is_dir() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the target is a directory",
        )),
        Ok(_) => fs::remove_file(target),
    }
}

/// Creates a symbolic link, on the platforms that have them.
#[cfg(unix)]
fn create_symlink(raw: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(raw, target)
}

/// Creates a symbolic link, on the platforms that have none.
#[cfg(not(unix))]
fn create_symlink(_raw: &Path, _target: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symbolic links are not supported here",
    ))
}

/// The mode of a file, on the platforms that have one.
#[cfg(unix)]
fn mode_of(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode() & 0o7777)
}

/// The mode of a file, on the platforms that have none.
#[cfg(not(unix))]
fn mode_of(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

/// The inode of a file, on the platforms that have one.
///
/// The inode survives a preserved time: a generator that replaces the file
/// (`cp -p` and `mv`, the reproducible-build pattern) leaves a new one behind,
/// and that is the only signal that such a write happened.
#[cfg(unix)]
fn inode_of(metadata: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino())
}

/// The inode of a file, on the platforms that have none.
#[cfg(not(unix))]
fn inode_of(_metadata: &fs::Metadata) -> Option<u64> {
    None
}

/// Sets the mode of a file, on the platforms that have one.
#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Sets the mode of a file, on the platforms that have none.
#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}
