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
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::Error;
use crate::config::{Config, Generated, Tier};
use crate::diff;
use crate::fsutil;
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
        let subject = format!("the generated file {}", entry.path);
        // The resolved path is the place the operating system puts the file:
        // the links of the named path have been followed.  It is the path the
        // engine touches — and the place the directory of the file is created.
        let resolved = match path::inside_repository(options.root, &entry.path) {
            Ok(resolved) => resolved,
            Err(refusal) => {
                return Err(Error::new(
                    refusal.message(&subject, "must lie inside the repository root"),
                ));
            }
        };
        if resolved == path::canonical_root(options.root) {
            return Err(Error::new(format!(
                "{subject} resolves to the repository root, not a file"
            )));
        }
        // A **dangling** link at the file position is refused.  The reason
        // depends on the mode: the in-place generator would write through the
        // link and create the file behind it — and the restore cannot take
        // that file back — while the temporary-output mode never touches
        // the tree, but the comparison would follow the link to a file that
        // is not there.
        if is_dangling_link(options.root, &entry.path) {
            let reason = if entry.output_arg.is_some() {
                "the comparison would follow it to a file that is not there"
            } else {
                "the generator would write through it, and the restore could not take the file \
                 back"
            };
            return Err(Error::new(format!(
                "{subject} is a symbolic link that points at nothing — {reason}"
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
        let result = run_one(entry, git, options, &mut outcome.debug, &resolved)?;
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
    resolved: &Path,
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
        None => produce_in_place(entry, root, options, debug, resolved)?,
    };

    // The comparison is against the committed file (SPEC §7).  The lookup
    // uses the file the system resolves the named path to — the same file the
    // generator wrote; the report and the messages keep the named path.
    let lookup = committed_path(root, &entry.path, resolved);
    let (status, committed) = compare(git, &lookup, &entry.path, &produced)?;

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
                    write_fixed(root, &target, &entry.path, resolved, &produced)?;
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
                        write_fixed(root, &target, &entry.path, resolved, &produced)?;
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
    })
}

/// True when the path is a symbolic link that points at nothing.
///
/// The generator would write through such a link and create the file behind
/// it — and the restore cannot take that file back: the tree would change,
/// and nobody was told about it.
fn is_dangling_link(root: &Path, relative: &str) -> bool {
    let target = root.join(relative);
    let Ok(metadata) = fs::symlink_metadata(&target) else {
        return false;
    };
    if !metadata.file_type().is_symlink() {
        return false;
    }
    // The link is followed: a link that points at nothing fails here.
    fs::metadata(&target).is_err()
}

/// The path the comparison is made against.
///
/// The comparison looks at the file the operating system **resolves** the
/// named path to: git stores a linked directory as a blob, so the committed
/// tree holds the target of the link and not the link.  The path is relative
/// to the repository root; the report and the messages keep the **named**
/// path — the path the project named and the path the user knows.
fn committed_path(root: &Path, relative: &str, resolved: &Path) -> String {
    path::relative_to(&path::canonical_root(root), resolved).unwrap_or_else(|| relative.to_string())
}

/// Compares the produced content with the committed file.
///
/// `lookup` is the path of the committed file, `named` the path the project
/// named: the messages name the named path.
///
/// * no repository → the comparison is skipped, the file counts as `new`;
/// * the file is not in the committed tree → `new`, informational and not a
///   failure;
/// * the committed path is a directory → an operational error;
/// * the committed file cannot be read → an operational error.  A broken
///   reference must not disable a gate (SPEC 8).
fn compare(
    git: &Git,
    lookup: &str,
    named: &str,
    produced: &[u8],
) -> Result<(DriftStatus, Option<Vec<u8>>), Error> {
    if !git.is_repo() {
        return Ok((DriftStatus::New, None));
    }
    match git.head_entry(lookup) {
        HeadEntry::Missing => Ok((DriftStatus::New, None)),
        HeadEntry::Directory => Err(Error::new(format!(
            "the committed path {named} is a directory, not a file"
        ))),
        HeadEntry::Unreadable => Err(Error::new(format!(
            "the committed file {named} cannot be read"
        ))),
        HeadEntry::Blob(object) => {
            let committed = git
                .show_object(&object)
                .ok_or_else(|| Error::new(format!("the committed file {named} cannot be read")))?;
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
    resolved: &Path,
) -> Result<Vec<u8>, Error> {
    let target = root.join(&entry.path);
    let display = path::display(Some(root), &target);

    // 1. the snapshot: the original bytes, the file mode and the time the
    //    file was written last.
    let mut snapshot = Snapshot::take(&target)
        .map_err(|error| Error::new(format!("cannot read {display}: {error}")))?;

    // 1b. the marker: the file is touched with an old time, so that the
    //     engine can tell whether the generator wrote it — whatever the
    //     timestamp granularity of the filesystem is.
    let marker = fsutil::marker_time();
    let _ = fsutil::set_modified(&target, marker);

    // 1c. the directories the file needs are created if they do not exist: a
    //     generator that writes a file into a new directory would otherwise
    //     fail, and a project cannot commit an empty directory. Cleanup after
    //     restore checks identity and containment immediately before removing
    //     each pathname.
    //     A directory that cannot be created is an error of the engine, not
    //     of the generator: the user is not sent to the wrong place.
    //
    //     The engine creates the directories the **operating system** needs
    //     to walk the named path: the missing directories of the traversal —
    //     a `..` can only pop a directory that exists — and the directory
    //     the path resolves to, the place the file lands in when a link
    //     leads there.
    //
    //     The error names the directory that could not be created: it is
    //     named as it was created — the resolved one as the system resolves
    //     it, the named one as the project named it — and it is the place the
    //     user has to look.
    let created = create_directories(root, &entry.path, resolved).map_err(|error| {
        Error::new(format!(
            "cannot create the directory {} of {display}: {}{}",
            directory_name(root, &error.directory),
            error.cause,
            error.rollback_message(root)
        ))
    })?;
    snapshot.created = created;

    // 2. the generator.
    let run = run_generator(&entry.command, root, &display, debug);

    // 3. the produced bytes.
    let produced = fs::read(&target).ok();

    // 3b. did the generator write the file?  A generator that leaves it alone
    //     produced nothing: the comparison below would compare the committed
    //     file with itself and report a green that says nothing.
    let wrote = snapshot.was_written(&target, produced.as_deref(), marker);

    // 4. restore and verify for an ordinary check; keep the produced content
    //    for --fix.
    let restored = if options.fix {
        snapshot.keep(root, resolved)
    } else {
        snapshot.restore(root)
    };

    // A failed restore or keep is the more important error: the working tree
    // was modified and nobody was told about it.
    if let Err(error) = restored {
        let operation = if options.fix { "fix" } else { "restore" };
        return Err(Error::new(format!(
            "the {operation} of {display} failed — the working tree was modified: {error}"
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

/// The identity captured for a directory the engine created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectoryIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows { volume: u64, file: u128 },
}

impl DirectoryIdentity {
    fn capture(path: &Path, handle: &fs::File) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            let _ = path;
            let metadata = handle.metadata()?;
            Ok(Self::Unix {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }

        #[cfg(windows)]
        {
            let _ = handle;
            let (volume, file) = fsutil::high_resolution_identity(path)?;
            Ok(Self::Windows { volume, file })
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = (path, handle);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "filesystem directory identity is unavailable on this platform",
            ))
        }
    }
}

/// A directory the engine created, with its identity and handle retained.
/// The retained handle helps prevent identity reuse while the generator runs.
#[derive(Debug)]
struct CreatedDirectory {
    path: PathBuf,
    identity: DirectoryIdentity,
    _handle: fs::File,
}

/// A directory the engine could not create or safely identify.
#[derive(Debug)]
struct DirectoryError {
    /// The directory the engine tried to create.
    directory: PathBuf,
    /// The cause: what the operating system said.
    cause: io::Error,
    /// Directories created by this attempt that rollback could not remove.
    rollback: Vec<(PathBuf, io::Error)>,
}

impl DirectoryError {
    /// Describes every directory left behind by a failed rollback.
    fn rollback_message(&self, root: &Path) -> String {
        if self.rollback.is_empty() {
            return String::new();
        }
        let failures = self
            .rollback
            .iter()
            .map(|(directory, error)| format!("{}: {error}", directory_name(root, directory)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("; rollback could not remove {failures} — the working tree was modified")
    }
}

/// Captures the identity immediately after creation. Unix reads dev/inode from
/// the retained handle, but directory creation returns no handle: a replacement
/// between creation and opening may be recorded as the candidate. Windows
/// obtains a high-resolution ID by pathname; a replacement during that initial
/// identity capture is also a narrow, accepted race.
fn capture_created_directory(path: &Path) -> io::Result<CreatedDirectory> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(io::Error::other(
            "the created path is no longer a directory",
        ));
    }
    let handle = open_directory(path)?;
    if !handle.metadata()?.is_dir() {
        return Err(io::Error::other(
            "the created path changed while its identity was captured",
        ));
    }
    let identity = DirectoryIdentity::capture(path, &handle)?;
    Ok(CreatedDirectory {
        path: path.to_path_buf(),
        identity,
        _handle: handle,
    })
}

/// Opens a directory handle, including on Windows where backup semantics are needed.
fn open_directory(path: &Path) -> io::Result<fs::File> {
    #[cfg(unix)]
    {
        fs::File::open(path)
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_BACKUP_SEMANTICS permits opening a directory handle.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "directory handles are unavailable on this platform",
        ))
    }
}

/// Validates a directory name through the path resolver before creating it.
fn validate_directory_candidate(root: &Path, candidate: &Path) -> io::Result<()> {
    let relative = path::relative_to(&path::canonical_root(root), candidate)
        .or_else(|| path::relative_to(root, candidate))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the directory candidate is not repository-relative",
            )
        })?;
    let resolved = path::inside_repository(root, &relative).map_err(|refusal| {
        io::Error::other(refusal.message(
            "the directory candidate",
            "must lie inside the repository root",
        ))
    })?;
    if !path::inside_root_directory(root, &resolved) {
        return Err(io::Error::other(
            "the directory candidate resolves to the repository root",
        ));
    }
    Ok(())
}

/// Confirms that the current path still identifies the directory created by
/// the engine. `false` means it is already gone; errors mean it changed or
/// cannot be identified safely.
fn verify_created_directory_identity(directory: &CreatedDirectory) -> io::Result<bool> {
    #[cfg(any(unix, windows))]
    {
        let metadata = match fs::symlink_metadata(&directory.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_dir() {
            return Err(io::Error::other(
                "the created path is no longer a directory; refusing cleanup",
            ));
        }
        let handle = open_directory(&directory.path)?;
        if !handle.metadata()?.is_dir() {
            return Err(io::Error::other(
                "the created path is no longer a directory; refusing cleanup",
            ));
        }
        let identity = DirectoryIdentity::capture(&directory.path, &handle)?;
        if identity != directory.identity {
            return Err(io::Error::other(
                "the directory identity changed; refusing cleanup",
            ));
        }
        Ok(true)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = directory;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "filesystem directory identity is unavailable on this platform",
        ))
    }
}

/// Confirms identity and that the current path remains inside the repository.
/// `None` means it is already gone.
fn verify_created_directory(
    root: &Path,
    directory: &CreatedDirectory,
) -> io::Result<Option<PathBuf>> {
    if !verify_created_directory_identity(directory)? {
        return Ok(None);
    }
    match fs::canonicalize(&directory.path) {
        Ok(canonical) if path::inside_root_directory(root, &canonical) => Ok(Some(canonical)),
        Ok(_) => Err(io::Error::other(
            "the directory resolves outside the repository root; refusing cleanup",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Removes one tracked directory only if it is still the same filesystem
/// object and remains inside the repository. The final identity and containment
/// check is immediately before pathname removal. Portable pathname APIs cannot
/// make those checks and removal atomic, so a replacement in that narrow
/// interval may still be removed.
fn remove_created_directory(root: &Path, directory: &CreatedDirectory) -> io::Result<()> {
    remove_created_directory_with(root, directory, || {})
}

fn remove_created_directory_with<F>(
    root: &Path,
    directory: &CreatedDirectory,
    before_final_validation: F,
) -> io::Result<()>
where
    F: FnOnce(),
{
    if verify_created_directory(root, directory)?.is_none() {
        return Ok(());
    }
    before_final_validation();
    if verify_created_directory(root, directory)?.is_none() {
        return Ok(());
    }
    fs::remove_dir(&directory.path)
}

/// Removes tracked empty directories, leaving non-empty or replaced paths alone.
fn remove_created_empty_directories<'a>(
    root: &Path,
    directories: impl IntoIterator<Item = &'a CreatedDirectory>,
) -> Vec<(PathBuf, io::Error)> {
    let mut failures = Vec::new();
    for directory in directories {
        match verify_created_directory(root, directory) {
            Ok(None) => continue,
            Err(error) => {
                failures.push((directory.path.clone(), error));
                continue;
            }
            Ok(Some(_)) => {}
        }

        match fs::read_dir(&directory.path) {
            Ok(mut entries) => match entries.next() {
                None => {
                    if let Err(error) = remove_created_directory(root, directory) {
                        if error.kind() != io::ErrorKind::NotFound {
                            failures.push((directory.path.clone(), error));
                        }
                    }
                }
                Some(Ok(_)) => {
                    if let Err(error) = verify_created_directory(root, directory) {
                        failures.push((directory.path.clone(), error));
                    }
                }
                Some(Err(error)) => failures.push((directory.path.clone(), error)),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => failures.push((directory.path.clone(), error)),
        }
    }
    failures
}

/// Creates the directories the generated file needs.
///
/// The **union** of two sets of directories, and both of them are needed:
///
/// * the **missing directories of the named traversal**: the directories the
///   operating system walks to reach the file.  A `..` can only pop a
///   directory that exists, so `sub/../x.md` needs `sub` — even though the
///   file itself lands in the root;
/// * the **directory the path resolves to**: the place the operating system
///   puts the file.  A link in the path leads to its target, and the target
///   is the place the file lands in.
///
/// The missing directories of the traversal are created **as they are named**:
/// the path the project named is the path the engine walks.  The directory
/// the path resolves to is created **where the path resolves to** — the place
/// the system puts the file.
///
/// The list that comes back is in cleanup order: the directories below a
/// directory come before it, so the parent is empty if it can be removed.
fn create_directories(
    root: &Path,
    relative: &str,
    resolved: &Path,
) -> Result<Vec<CreatedDirectory>, DirectoryError> {
    create_directories_with(root, relative, resolved, |directory| {
        fs::create_dir(directory)
    })
}

fn create_directories_with<F>(
    root: &Path,
    relative: &str,
    resolved: &Path,
    create: F,
) -> Result<Vec<CreatedDirectory>, DirectoryError>
where
    F: FnMut(&Path) -> io::Result<()>,
{
    create_directories_with_capture(root, relative, resolved, create, capture_created_directory)
}

fn create_directories_with_capture<F, C>(
    root: &Path,
    relative: &str,
    resolved: &Path,
    mut create: F,
    mut capture: C,
) -> Result<Vec<CreatedDirectory>, DirectoryError>
where
    F: FnMut(&Path) -> io::Result<()>,
    C: FnMut(&Path) -> io::Result<CreatedDirectory>,
{
    let mut created = Vec::new();
    let mut directories = Vec::new();

    // 1. the directory the path resolves to. It has to lie strictly inside
    //    the repository root. The root itself exists — there is nothing to
    //    create.
    if let Some(directory) = resolved.parent() {
        if path::inside_root_directory(root, directory) {
            directories.push(directory.to_path_buf());
        }
    }

    // 2. the missing directories of the named traversal.  They come after
    //    the resolved directory: a link in the path is only a directory when
    //    its target exists.
    directories.extend(traversal_directories(root, relative));
    for directory in directories {
        if directory.is_dir() {
            // The system walks into it: there is nothing to create.
            continue;
        }
        if path::is_link(&directory) {
            // A link is not the engine's to create: the system walks into
            // its target, and the resolved directory above has created it.
            continue;
        }
        let missing = fsutil::missing_directories(&directory);
        // `missing_directories` is deepest-first. Create each parent before
        // its child so the successful creations can be rolled back exactly.
        // An empty result means an existing non-directory (normally a file):
        // attempting to create it preserves the operating system's error.
        let candidates = if missing.is_empty() {
            vec![directory.clone()]
        } else {
            missing.into_iter().rev().collect()
        };
        for candidate in candidates {
            // Resolve each name through the same path authority before the
            // syscall. A concurrent link swap can still race this check; the
            // post-create check then reports the new path as left in place.
            if let Err(cause) = validate_directory_candidate(root, &candidate) {
                let rollback = rollback_directories(root, &created);
                return Err(DirectoryError {
                    directory: candidate,
                    cause,
                    rollback,
                });
            }
            match create(&candidate) {
                Ok(()) => {
                    let directory = match capture(&candidate) {
                        Ok(directory) => directory,
                        Err(cause) => {
                            return Err(post_creation_directory_error(
                                root, &candidate, &created, cause,
                            ));
                        }
                    };
                    match verify_created_directory(root, &directory) {
                        Ok(Some(_)) => created.push(directory),
                        Ok(None) => {
                            let rollback = rollback_directories(root, &created);
                            return Err(DirectoryError {
                                directory: candidate,
                                cause: io::Error::other(
                                    "the created directory disappeared before validation",
                                ),
                                rollback,
                            });
                        }
                        Err(cause) => {
                            return Err(post_creation_directory_error(
                                root, &candidate, &created, cause,
                            ));
                        }
                    }
                }
                Err(cause)
                    if cause.kind() == io::ErrorKind::AlreadyExists && candidate.is_dir() =>
                {
                    // Another process made the directory after the missing
                    // path was inspected. It is usable, but not ours to remove.
                    if let Err(cause) = validate_directory_candidate(root, &candidate) {
                        let rollback = rollback_directories(root, &created);
                        return Err(DirectoryError {
                            directory: candidate,
                            cause,
                            rollback,
                        });
                    }
                }
                Err(cause) => {
                    let rollback = rollback_directories(root, &created);
                    return Err(DirectoryError {
                        directory: candidate,
                        cause,
                        rollback,
                    });
                }
            }
        }
    }

    // The creation order has parents before children; removal is its reverse.
    created.reverse();
    Ok(created)
}

/// Attempts rollback of every directory created by a failed attempt, deepest first.
fn rollback_directories(root: &Path, created: &[CreatedDirectory]) -> Vec<(PathBuf, io::Error)> {
    let mut failures = Vec::new();
    for directory in created.iter().rev() {
        if let Err(error) = remove_created_directory(root, directory) {
            if error.kind() != io::ErrorKind::NotFound {
                failures.push((directory.path.clone(), error));
            }
        }
    }
    failures
}

/// Reports a candidate created by the engine but rejected by post-create
/// identity or containment validation. It is never removed by name alone.
fn post_creation_directory_error(
    root: &Path,
    candidate: &Path,
    created: &[CreatedDirectory],
    cause: io::Error,
) -> DirectoryError {
    let mut rollback = rollback_directories(root, created);
    match fs::symlink_metadata(candidate) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => rollback.push((
            candidate.to_path_buf(),
            io::Error::other(format!(
                "post-creation validation failed; candidate left in place: {cause}"
            )),
        )),
        Err(error) => rollback.push((
            candidate.to_path_buf(),
            io::Error::other(format!(
                "post-creation validation failed; candidate may remain and was left untouched: \
                 {cause}; cannot inspect it: {error}"
            )),
        )),
    }
    DirectoryError {
        directory: candidate.to_path_buf(),
        cause: io::Error::other(format!("post-creation validation failed: {cause}")),
        rollback,
    }
}

/// The directories the operating system walks to reach the file.
///
/// Every component of the named path but the last is a directory the system
/// walks into: a `..` pops the prefix, and the directory it pops is the one
/// before it.
///
/// The directories are **as the path names them**: the engine walks the path
/// the project named.
fn traversal_directories(root: &Path, relative: &str) -> Vec<PathBuf> {
    let components: Vec<Component<'_>> = Path::new(relative).components().collect();
    let mut prefix = PathBuf::new();
    let mut directories = Vec::new();
    for (index, component) in components.iter().enumerate() {
        match component {
            Component::Normal(name) => {
                prefix.push(name);
                // The last component is the file itself.
                if index + 1 < components.len() {
                    directories.push(root.join(&prefix));
                }
            }
            // A `..` pops the prefix: the path goes up again.
            Component::ParentDir => {
                prefix.pop();
            }
            Component::CurDir => {}
            _ => {}
        }
    }
    directories
}

/// The name of a directory in an error: the path relative to the repository
/// root.
///
/// The directory is named as it was created: the resolved one as the system
/// resolves it, the named one as the project named it.
fn directory_name(root: &Path, directory: &Path) -> String {
    path::relative_to(&path::canonical_root(root), directory)
        .or_else(|| path::relative_to(root, directory))
        .unwrap_or_else(|| path::file_name(directory))
}

/// Writes the produced content into the generated file.
///
/// The mode of an existing file is kept: `--fix` changes the content of the
/// generated files, nothing else.
fn write_fixed(
    root: &Path,
    target: &Path,
    relative: &str,
    resolved: &Path,
    produced: &[u8],
) -> Result<(), Error> {
    let display = path::display(Some(root), target);
    if target.exists() {
        // Writing in place keeps the mode of the file.
        fs::write(target, produced)
            .map_err(|error| Error::new(format!("cannot write {display}: {error}")))?;
        return Ok(());
    }
    // The directories the file needs are created: the engine's business, not
    // the generator's.  The write follows the link, and the file lands
    // behind it.
    let created = create_directories(root, relative, resolved).map_err(|error| {
        Error::new(format!(
            "cannot write {display}: cannot create the directory {}: {}{}",
            directory_name(root, &error.directory),
            error.cause,
            error.rollback_message(root)
        ))
    })?;
    if let Err(error) = fs::write(target, produced) {
        let cleanup = cleanup_fix_directories(root, &created, resolved);
        let message = match cleanup {
            Ok(()) => format!("cannot write {display}: {error}"),
            Err(cleanup) => format!(
                "cannot write {display}: {error}; {cleanup} — the working tree was modified"
            ),
        };
        return Err(Error::new(message));
    }
    cleanup_fix_directories(root, &created, resolved).map_err(|error| {
        Error::new(format!(
            "cannot finish fixing {display}: {error} — the working tree was modified"
        ))
    })
}

/// Attempts cleanup of empty directories created only to walk the named path.
///
/// The resolved target's parent and its ancestors are required for the file
/// to remain accessible. Canonicalizing each created directory also handles a
/// traversal that reaches one of those parents through an in-root link.
fn cleanup_fix_directories(
    root: &Path,
    created: &[CreatedDirectory],
    resolved: &Path,
) -> Result<(), String> {
    let resolved_parent = resolved.parent().unwrap_or(root);
    let canonical_root = path::canonical_root(root);
    let mut traversal_only: Vec<&CreatedDirectory> = Vec::new();
    let mut failures = Vec::new();
    for directory in created {
        match verify_created_directory(root, directory) {
            Ok(None) => continue,
            Err(error) => {
                failures.push((directory.path.clone(), error));
                continue;
            }
            Ok(Some(canonical)) if canonical != canonical_root => {
                if !resolved_parent.starts_with(&canonical) {
                    traversal_only.push(directory);
                }
            }
            // The repository root and the resolved target's parent chain are
            // required to keep the generated file in place.
            Ok(Some(_)) => {}
        }
    }
    failures.extend(remove_created_empty_directories(
        root,
        traversal_only.iter().copied(),
    ));
    if failures.is_empty() {
        return Ok(());
    }
    let failures = failures
        .iter()
        .map(|(directory, error)| format!("{}: {error}", directory_name(root, directory)))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "could not remove created traversal directories: {failures}"
    ))
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
    /// The directories the engine created for the file.
    ///
    /// The list is in cleanup order: children come before parents.
    created: Vec<CreatedDirectory>,
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
        match fs::symlink_metadata(target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Snapshot {
                target: target.to_path_buf(),
                content: None,
                modified: None,
                inode: None,
                mode: None,
                link: None,
                created: Vec::new(),
            }),
            Err(error) => Err(error),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let raw = fs::read_link(target)?;
                let resolved = resolve(target, &raw);
                let (content, modified, inode, mode) = match fs::metadata(&resolved) {
                    Ok(metadata) => (
                        Some(fs::read(&resolved)?),
                        metadata.modified().ok(),
                        fsutil::inode_of(&metadata),
                        fsutil::mode_of(&metadata),
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
                    created: Vec::new(),
                })
            }
            Ok(metadata) => Ok(Snapshot {
                target: target.to_path_buf(),
                content: Some(fs::read(target)?),
                modified: metadata.modified().ok(),
                inode: fsutil::inode_of(&metadata),
                mode: fsutil::mode_of(&metadata),
                link: None,
                created: Vec::new(),
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
        if fsutil::inode_of(&metadata) != self.inode() {
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
    fn restore(&self, root: &Path) -> Result<(), String> {
        self.restore_with(root, || self.restore_file())?;
        self.verify()
    }

    /// Runs file restoration and always attempts directory cleanup.
    ///
    /// Kept separate so a file-restore failure can be tested deterministically
    /// without relying on filesystem permissions.
    fn restore_with<F>(&self, root: &Path, restore_file: F) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        let restored = restore_file();
        let cleaned = self.remove_created_directories(root);
        match (restored, cleaned) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(restore), Ok(())) => Err(format!("file restoration failed: {restore}")),
            (Ok(()), Err(cleanup)) => Err(cleanup),
            (Err(restore), Err(cleanup)) => {
                Err(format!("file restoration failed: {restore}; {cleanup}"))
            }
        }
    }

    /// Restores the file or symbolic link without cleaning up its directories.
    fn restore_file(&self) -> Result<(), String> {
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
                    fsutil::write_atomic(&link.resolved, content, link.mode, link.modified)
                        .map_err(|error| error.to_string())?;
                }
                remove_path(&self.target).map_err(|error| error.to_string())?;
                create_symlink(&link.raw, &self.target).map_err(|error| error.to_string())?;
            }
            None => match &self.content {
                Some(content) => {
                    fsutil::write_atomic(&self.target, content, self.mode, self.modified)
                        .map_err(|error| error.to_string())?
                }
                None => {
                    // The file did not exist before: the generator created it.
                    if fs::symlink_metadata(&self.target).is_ok() {
                        fs::remove_file(&self.target).map_err(|error| error.to_string())?;
                    }
                }
            },
        }
        Ok(())
    }

    /// Keeps the produced content, restores its mode, and cleans traversal dirs.
    ///
    /// This is what `--fix` asks for: the content of the generated file is
    /// the produced one, required resolved parents remain, and everything else
    /// stays as it was.
    fn keep(&self, root: &Path, resolved: &Path) -> Result<(), String> {
        let kept = self.keep_mode();
        let cleaned = cleanup_fix_directories(root, &self.created, resolved);
        match (kept, cleaned) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(keep), Ok(())) => Err(keep),
            (Ok(()), Err(cleanup)) => Err(cleanup),
            (Err(keep), Err(cleanup)) => Err(format!("{keep}; {cleanup}")),
        }
    }

    /// Restores the file mode while keeping the produced content.
    fn keep_mode(&self) -> Result<(), String> {
        if let (Some(mode), None) = (self.mode, &self.link) {
            let metadata = fs::metadata(&self.target).map_err(|error| error.to_string())?;
            if fsutil::mode_of(&metadata) != Some(mode) {
                fsutil::set_mode(&self.target, mode).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    /// Removes the directories the engine created and left empty.
    fn remove_created_directories(&self, root: &Path) -> Result<(), String> {
        let failures = remove_created_empty_directories(root, &self.created);
        if failures.is_empty() {
            return Ok(());
        }
        let failures = failures
            .iter()
            .map(|(directory, error)| format!("{}: {error}", directory_name(root, directory)))
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!("could not remove created directories: {failures}"))
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
                        if fsutil::mode_of(&metadata) != Some(mode) {
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

/// The path a symbolic link points to.
fn resolve(target: &Path, raw: &Path) -> PathBuf {
    if path::is_absolute(raw) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn restore_failure_still_removes_created_empty_directories() {
        let root = crate::test_tempdir::TempDir::new().expect("a temporary root");
        let parent = root.path().join("new");
        let nested = parent.join("nested");
        fs::create_dir_all(&nested).expect("the directories are created");
        let snapshot = Snapshot {
            target: nested.join("generated.md"),
            content: None,
            modified: None,
            inode: None,
            mode: None,
            link: None,
            created: vec![
                capture_created_directory(&nested).expect("the nested directory identity is read"),
                capture_created_directory(&parent).expect("the parent directory identity is read"),
            ],
        };

        let error = snapshot
            .restore_with(root.path(), || {
                Err("injected file restore failure".to_string())
            })
            .expect_err("the injected restoration fails");

        assert!(error.contains("injected file restore failure"), "{error}");
        assert!(!parent.exists(), "the parent directory was left behind");
        assert!(!nested.exists(), "the nested directory was left behind");
    }

    #[cfg(unix)]
    #[test]
    fn directory_cleanup_failures_name_repository_relative_paths() {
        let root = crate::test_tempdir::TempDir::new().expect("a temporary root");
        let not_a_directory = root.path().join("not-a-directory");
        fs::create_dir(&not_a_directory).expect("the directory is created");
        let created = capture_created_directory(&not_a_directory)
            .expect("the created directory identity is read");
        fs::remove_dir(&not_a_directory).expect("the directory is removed");
        fs::write(&not_a_directory, "content").expect("the replacement file is created");
        let snapshot = Snapshot {
            target: not_a_directory.clone(),
            content: None,
            modified: None,
            inode: None,
            mode: None,
            link: None,
            created: vec![created],
        };

        let error = snapshot
            .remove_created_directories(root.path())
            .expect_err("reading the file as a directory fails");

        assert!(error.contains("not-a-directory"), "{error}");
        assert!(error.contains("no longer a directory"), "{error}");
        assert!(
            !error.contains(root.path().to_str().expect("the root is UTF-8")),
            "the error contains an absolute path: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_creation_refuses_an_outside_traversal_candidate_before_creating_it() {
        let root = crate::test_tempdir::TempDir::new().expect("a temporary root");
        let outside = crate::test_tempdir::TempDir::new().expect("an outside directory");
        let root_path = fs::canonicalize(root.path()).expect("the root is resolved");
        let outside_path = fs::canonicalize(outside.path()).expect("outside is resolved");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&outside_path, &link).expect("the link is made");
        let root_name = root_path
            .file_name()
            .expect("the temp root has a name")
            .to_string_lossy();
        let relative = format!("link/sub/../../{root_name}/generated.md");
        let resolved = root_path.join("generated.md");
        assert_eq!(
            path::inside_repository(root.path(), &relative).expect("the file resolves in root"),
            resolved
        );
        let mut creates = 0;

        let error = create_directories_with(root.path(), &relative, &resolved, |directory| {
            creates += 1;
            fs::create_dir(directory)
        })
        .expect_err("the outside traversal directory is refused");

        assert_eq!(creates, 0, "the rejected candidate was created");
        assert!(
            error
                .cause
                .to_string()
                .contains("must lie inside the repository root"),
            "{}",
            error.cause
        );
        assert!(
            !outside.path().join("sub").exists(),
            "the outside candidate was left behind"
        );
        assert!(error.rollback.is_empty(), "rollback left paths behind");
    }

    #[cfg(unix)]
    #[test]
    fn a_candidate_that_escapes_after_validation_is_reported_left_behind() {
        let root = crate::test_tempdir::TempDir::new().expect("a temporary root");
        let outside = crate::test_tempdir::TempDir::new().expect("an outside directory");
        let root_path = fs::canonicalize(root.path()).expect("the root is resolved");
        let outside_path = fs::canonicalize(outside.path()).expect("outside is resolved");
        let unrelated = outside.path().join("keep.txt");
        fs::write(&unrelated, "keep").expect("the unrelated outside file is written");
        let internal = root_path.join("internal");
        fs::create_dir(&internal).expect("the internal target is made");
        let link = root_path.join("link");
        std::os::unix::fs::symlink(&internal, &link).expect("the link is made");
        let relative = "link/sub/../generated.md";
        let resolved = path::inside_repository(root.path(), relative)
            .expect("the generated target initially resolves inside root");
        let candidate = root.path().join("link/sub");

        let error = create_directories_with(root.path(), relative, &resolved, |directory| {
            assert_eq!(directory, candidate);
            fs::remove_file(&link)?;
            std::os::unix::fs::symlink(&outside_path, &link)?;
            fs::create_dir(directory)
        })
        .expect_err("post-create containment validation detects the swapped link");

        assert!(outside.path().join("sub").is_dir());
        assert_eq!(
            fs::read_to_string(&unrelated).expect("the unrelated outside file remains"),
            "keep"
        );
        let rollback = error.rollback_message(root.path());
        assert!(rollback.contains("link/sub"), "{rollback}");
        assert!(rollback.contains("candidate left in place"), "{rollback}");
        assert!(rollback.contains("working tree was modified"), "{rollback}");
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_rechecks_containment_before_removing_a_traversal_directory() {
        let root = crate::test_tempdir::TempDir::new().expect("a temporary root");
        let outside = crate::test_tempdir::TempDir::new().expect("an outside directory");
        let internal_target = root.path().join("internal");
        let candidate = internal_target.join("sub");
        fs::create_dir_all(&candidate).expect("the candidate directory is created");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&internal_target, &link).expect("the link is made");
        let created = capture_created_directory(&link.join("sub"))
            .expect("the created directory identity is captured");
        let moved_target = outside.path().join("moved");

        let error = remove_created_directory_with(root.path(), &created, || {
            fs::remove_file(&link).expect("the original link is removed");
            fs::rename(&internal_target, &moved_target).expect("the target is moved outside");
            std::os::unix::fs::symlink(&moved_target, &link).expect("the outside link is made");
        })
        .expect_err("the final containment validation refuses the outside path");

        assert!(
            error.to_string().contains("outside the repository root"),
            "{error}"
        );
        assert!(
            verify_created_directory_identity(&created)
                .expect("the same directory identity is still present"),
            "the test did not preserve the tracked directory identity"
        );
        assert!(
            moved_target.join("sub").is_dir(),
            "cleanup removed the directory after its ancestor redirected outside"
        );
    }

    #[test]
    fn an_unavailable_directory_identity_leaves_and_reports_the_candidate() {
        let root = crate::test_tempdir::TempDir::new().expect("a temporary root");
        let resolved = root.path().join("new/generated.md");
        let candidate = root.path().join("new");
        let mut creates = 0;

        let error = create_directories_with_capture(
            root.path(),
            "new/generated.md",
            &resolved,
            |_| {
                creates += 1;
                fs::create_dir(&candidate)
            },
            |_| {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "the filesystem did not provide a high-resolution file identity",
                ))
            },
        )
        .expect_err("the candidate cannot be tracked without an identity");

        assert_eq!(creates, 1, "directory creation was refused pre-emptively");
        assert!(
            candidate.is_dir(),
            "the untracked candidate was removed by name"
        );
        assert!(
            error
                .cause
                .to_string()
                .contains("high-resolution file identity"),
            "{}",
            error.cause
        );
        let rollback = error.rollback_message(root.path());
        assert!(rollback.contains("new"), "{rollback}");
        assert!(rollback.contains("candidate left in place"), "{rollback}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_directory_creation_uses_high_resolution_identity() {
        let root = crate::test_tempdir::TempDir::new().expect("a temporary root");
        let resolved = root.path().join("new/generated.md");
        let candidate = root.path().join("new");

        let created = create_directories(root.path(), "new/generated.md", &resolved)
            .expect("Windows directory creation has a high-resolution identity");

        assert_eq!(created.len(), 1);
        assert!(candidate.is_dir(), "the directory was not created");
        assert!(remove_created_empty_directories(root.path(), &created).is_empty());
        assert!(!candidate.exists(), "the tracked directory was not removed");
    }

    #[cfg(unix)]
    #[test]
    fn failed_directory_rollback_reports_every_directory_left_behind() {
        let root = crate::test_tempdir::TempDir::new().expect("a temporary root");
        let resolved = root.path().join("a/b/c/generated.md");
        let mut calls = 0;

        let error =
            create_directories_with(root.path(), "a/b/c/generated.md", &resolved, |directory| {
                calls += 1;
                if calls == 3 {
                    return Err(io::Error::other("injected creation failure"));
                }
                fs::create_dir(directory)?;
                if calls == 2 {
                    fs::write(directory.join("external.txt"), "keep")?;
                }
                Ok(())
            })
            .expect_err("the third directory creation fails");

        assert_eq!(error.rollback.len(), 2);
        assert_eq!(error.rollback[0].0, root.path().join("a/b"));
        assert_eq!(error.rollback[1].0, root.path().join("a"));
        let message = error.rollback_message(root.path());
        assert!(message.contains("a/b"), "{message}");
        assert!(message.contains("a:"), "{message}");
        assert!(message.contains("working tree was modified"), "{message}");
        assert!(root.path().join("a/b/external.txt").exists());
    }
}
