//! Git integration.
//!
//! The engine only ever *reads* from git: the repository root and the
//! committed version of a file.  Every git error degrades to "there is no
//! committed reference" — a missing git must never panic.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A git repository (or the absence of one).
#[derive(Debug, Default)]
pub struct Git {
    root: Option<PathBuf>,
}

impl Git {
    /// Looks for the repository that contains `cwd`.
    pub fn discover(cwd: &Path) -> Self {
        let root = run(cwd, &["rev-parse", "--show-toplevel"])
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            .map(PathBuf::from);
        Self { root }
    }

    /// The repository root, if there is one.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// True when `cwd` is inside a git repository.
    pub fn is_repo(&self) -> bool {
        self.root.is_some()
    }

    /// The committed content of `path` (relative to the repository root).
    ///
    /// Returns `None` when there is no repository, when the file is not
    /// committed, or when git fails.
    pub fn show_head(&self, path: &str) -> Option<String> {
        let root = self.root.as_ref()?;
        let spec = format!("HEAD:{path}");
        run(root, &["show", &spec])
    }
}

/// Runs a git command and returns its stdout.
fn run(cwd: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}
