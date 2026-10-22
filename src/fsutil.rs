//! The filesystem helpers that the gates share.
//!
//! Three things live here, because two gates need them and a copy of a copy
//! is a copy that drifts:
//!
//! * the **temporary directory** that a command writes into — it is removed
//!   again when it goes out of scope, on every path;
//! * the **atomic write**: the content goes into a temporary file in the same
//!   directory and is then **renamed** over the target — a rename replaces a
//!   symbolic link instead of writing through it, and it does not need write
//!   permission on the target itself;
//! * the **mode** of a file, on the platforms that have one.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A temporary directory that is removed again when it goes out of scope.
///
/// The guard exists so that **every** path removes it: a successful run, a
/// failing command, and every early return in between.  A resource that grows
/// without bound is a defect even outside the repository.
#[derive(Debug)]
pub struct Temporary {
    /// The path of the directory.
    path: PathBuf,
}

impl Temporary {
    /// Creates a temporary directory named after the `prefix`.
    pub fn new(prefix: &str) -> io::Result<Self> {
        let path = temporary_directory(prefix);
        fs::create_dir_all(&path)?;
        Ok(Temporary { path })
    }

    /// The path of the directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A unique temporary directory, named after the `prefix`.
fn temporary_directory(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("ratchet-{prefix}-{}-{nanos}", std::process::id()))
}

/// Writes `content` to `path` through a temporary file in the same directory.
///
/// The temporary file carries the given mode and time and is then **renamed**
/// over the target: a rename replaces a symbolic link instead of writing
/// through it, and it does not need write permission on the target itself.
pub fn write_atomic(
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

/// The name of the temporary file of a write.
///
/// The name carries the process id and a counter: two writes in the same run
/// never collide, and the file is gone before the run ends.
fn temporary_name(path: &Path) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let number = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    format!(".ratchet-restore-{}-{number}-{name}", std::process::id())
}

/// Sets the time the file was written last.
pub fn set_modified(path: &Path, time: SystemTime) -> io::Result<()> {
    let file = fs::File::open(path)?;
    let times = fs::FileTimes::new().set_modified(time);
    file.set_times(times)
}

/// A time far enough in the past that every write lands after it.
///
/// The engine touches a file with this time before a command runs: a command
/// that writes the file sets a current time, and the difference between the
/// two tells the engine that the file was written — on every filesystem,
/// whatever its timestamp granularity is.
pub fn marker_time() -> SystemTime {
    SystemTime::now() - Duration::from_secs(3600)
}

/// The inode of a file, on the platforms that have one.
///
/// The inode survives a preserved time: a command that **replaces** the file
/// (`cp -p` and `mv`, the reproducible-build pattern) leaves a new one behind,
/// and that is the only signal that such a write happened.
#[cfg(unix)]
pub fn inode_of(metadata: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino())
}

/// The inode of a file, on the platforms that have none.
#[cfg(not(unix))]
pub fn inode_of(_metadata: &fs::Metadata) -> Option<u64> {
    None
}

/// The directories that do not exist yet below the target.
///
/// The list is ordered from the deepest directory to the shallowest one: an
/// empty parent can only be removed after its child is gone.
pub fn created_directories(target: &Path) -> Vec<PathBuf> {
    match target.parent() {
        Some(parent) => missing_directories(parent),
        None => Vec::new(),
    }
}

/// The directories that do not exist yet, from `directory` up to the first one
/// that exists.
///
/// The list is ordered from the deepest directory to the shallowest one: it is
/// the order in which the directories have to be **removed** — an empty parent
/// can only be removed after its child is gone.
pub fn missing_directories(directory: &Path) -> Vec<PathBuf> {
    let mut missing = Vec::new();
    let mut current = Some(directory);
    while let Some(candidate) = current {
        if candidate.as_os_str().is_empty() {
            break;
        }
        if !candidate.exists() {
            missing.push(candidate.to_path_buf());
        } else {
            break;
        }
        current = candidate.parent();
    }
    missing
}

/// Removes the directories that are empty.
///
/// A directory that still holds something was not left behind by the command
/// alone — it is not the engine's to remove.
pub fn remove_empty_directories(directories: &[PathBuf]) {
    for directory in directories {
        match fs::read_dir(directory) {
            Ok(mut entries) => {
                if entries.next().is_none() {
                    let _ = fs::remove_dir(directory);
                }
            }
            Err(_) => continue,
        }
    }
}

/// The mode of a file, on the platforms that have one.
#[cfg(unix)]
pub fn mode_of(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode() & 0o7777)
}

/// The mode of a file, on the platforms that have none.
#[cfg(not(unix))]
pub fn mode_of(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

/// Sets the mode of a file, on the platforms that have one.
#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Sets the mode of a file, on the platforms that have none.
#[cfg(not(unix))]
pub fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}
