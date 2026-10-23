use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A temporary directory removed when the test fixture is dropped.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a unique directory under the system temporary directory.
    pub fn new() -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..100 {
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ratchet-test-{}-{nonce}-{serial}",
                std::process::id()
            ));
            match create_temp_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique test directory",
        ))
    }

    /// Returns the path of the directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn create_temp_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)
    }
    #[cfg(not(unix))]
    {
        fs::DirBuilder::new().create(path)
    }
}

#[cfg(unix)]
#[test]
fn tempdir_permissions_exclude_group_and_other_access() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TempDir::new().expect("a temporary directory");
    let mode = fs::metadata(directory.path())
        .expect("the temporary directory metadata is readable")
        .permissions()
        .mode();

    assert_eq!(mode & 0o077, 0, "temporary directory mode was {mode:o}");
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
