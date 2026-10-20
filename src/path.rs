//! Path helpers.
//!
//! Every path in the report is relative to the repository root (SPEC §6): an
//! absolute path, a machine name or a timestamp makes the same tree produce
//! different bytes.  These helpers are the only place that decides how a path
//! is shown.

use std::path::{Component, Path, PathBuf};

/// The path of `path` relative to `root`, with `/` as the separator.
pub fn relative_to(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    Some(slash(relative))
}

/// The path with `.` and `..` resolved, without touching the file system.
///
/// A path is compared with the repository root before it exists: the
/// comparison has to be lexical, not a `canonicalize` that fails for a file
/// that is not there yet.
pub fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// A display name for `path` that is never absolute: the path relative to
/// `root` when the file is inside it, the file name otherwise.
pub fn display(root: Option<&Path>, path: &Path) -> String {
    match root {
        Some(root) => relative_to(root, path).unwrap_or_else(|| file_name(path)),
        None => file_name(path),
    }
}

/// The file name of `path`.
pub fn file_name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => slash(path),
    }
}

/// The path with `/` as the separator.
fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_a_path_relative_to_the_root() {
        let root = Path::new("/repo");
        assert_eq!(
            display(Some(root), Path::new("/repo/docs/audit.json")),
            "docs/audit.json"
        );
    }

    #[test]
    fn shows_the_file_name_of_a_path_outside_the_root() {
        let root = Path::new("/repo");
        assert_eq!(
            display(Some(root), Path::new("/elsewhere/audit.json")),
            "audit.json"
        );
    }

    #[test]
    fn a_sibling_directory_is_not_inside_the_root() {
        let root = Path::new("/repo");
        assert_eq!(
            display(Some(root), Path::new("/repository/audit.json")),
            "audit.json"
        );
    }
}
