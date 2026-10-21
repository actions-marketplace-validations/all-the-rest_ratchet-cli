//! Path helpers.
//!
//! Every path in the report is relative to the repository root (SPEC §6): an
//! absolute path, a machine name or a timestamp makes the same tree produce
//! different bytes.  These helpers are the only place that decides how a path
//! is shown.

use std::fs;
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

/// The number of links that are followed before the resolution gives up.
///
/// The system's own limit is smaller on some systems (15 on Darwin) and
/// larger on others (40 on Linux): where the system refuses earlier, the
/// operation fails with `ELOOP`.  A path with more links than this cannot be
/// resolved — and the tool refuses it: fail closed.
const LINK_DEPTH: usize = 32;

/// Why a path could not be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// A `..` follows a component that was resolved through a link: the
    /// operating system reads it as the parent of the link's **target**, a
    /// reader of the config as the parent of the named directory.  The tool
    /// does not choose between the two readings.
    Ambiguous,
    /// The links of the path could not be followed to the end: a cycle, a
    /// chain that is longer than the bound, a link that cannot be read.
    Unresolvable,
}

/// Resolves `path` against `start`, component by component, the way the
/// operating system resolves it.
///
/// * `.` is skipped;
/// * `..` pops the resolved prefix — unless the component before it was
///   resolved through a link: then the path is **ambiguous** and the tool
///   refuses it;
/// * a name that is a symbolic link is followed: its target is resolved the
///   same way, and the walk continues from there.
///
/// The path does not have to exist: a component that is not there is simply
/// not a link.
fn walk(start: &Path, path: &Path, depth: usize) -> Result<PathBuf, Failure> {
    let mut resolved = start.to_path_buf();
    // True when the last component was resolved through a link: a `..` after
    // it is ambiguous.
    let mut through_link = false;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if through_link {
                    return Err(Failure::Ambiguous);
                }
                resolved.pop();
            }
            Component::RootDir => resolved.push(Path::new("/")),
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::Normal(name) => {
                let candidate = resolved.join(name);
                if !is_link(&candidate) {
                    resolved = candidate;
                    through_link = false;
                    continue;
                }
                // The link is followed: its target is resolved the same way.
                if depth == 0 {
                    // Too many links: the path cannot be resolved.
                    return Err(Failure::Unresolvable);
                }
                let target = fs::read_link(&candidate).map_err(|_| Failure::Unresolvable)?;
                let target = if target.is_absolute() {
                    target
                } else {
                    // The target of a link is relative to the directory of
                    // the link.
                    resolved.join(target)
                };
                resolved = walk(Path::new("/"), &target, depth - 1)?;
                through_link = true;
            }
        }
    }
    Ok(resolved)
}

/// True when the path is a symbolic link.
fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

/// The answer of [`inside_root`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inside {
    /// The path lies inside the repository root.
    Yes,
    /// The path is absolute: the paths of a project are relative.
    Absolute,
    /// The path leaves the repository root.
    Outside,
    /// The path cannot be resolved: the links of the path could not be
    /// followed to the end.
    Unresolvable,
    /// The path is ambiguous: a `..` after a link has two readings.
    Ambiguous,
}

impl Inside {
    /// The refusal of the path, or `None` when it lies inside the root.
    ///
    /// `subject` names the path in the caller's words — "the generated file
    /// docs/x.md", "docs/x.json" — and `outside` is the sentence for a path
    /// that leaves the root: it differs per subject ("must lie inside the
    /// repository root", "is outside the repository root").
    pub fn refusal(self, subject: &str, outside: &str) -> Option<String> {
        match self {
            Inside::Yes => None,
            // The paths of a project are relative: an absolute path names a
            // place on one machine, and a committed config has to work on
            // every machine.
            Inside::Absolute => Some(format!(
                "{subject} has to be a relative path — the paths of a project are relative"
            )),
            Inside::Outside => Some(format!("{subject} {outside}")),
            // A path that cannot be resolved is not inside the root: the
            // engine cannot tell where it points, so it refuses it.
            Inside::Unresolvable => Some(format!(
                "{subject} could not be resolved — the links of the path could not be followed \
                 to the end"
            )),
            // The two readings differ: the tool does not pick one.
            Inside::Ambiguous => Some(format!(
                "{subject} is ambiguous — a \"..\" after a symbolic link has two readings, and \
                 the tool does not choose one"
            )),
        }
    }
}

/// Whether the path that a project names lies inside the repository root.
///
/// The path is **relative** — a committed config has to work on every
/// machine — and it is resolved against `base`: the repository root for most
/// paths, the directory of the config for the baseline.
///
/// The path is resolved **component by component**, the way the operating
/// system resolves it, and the **whole** path is compared with the resolved
/// root: a link in a directory of the path may not carry it out of the root,
/// and a link at the file position may not either.  The file may be a link as
/// long as its **target** lies inside the root: there the comparison follows
/// the link, as the drift gate intends (SPEC §7).
pub fn inside_root(root: &Path, base: &Path, relative: &str) -> Inside {
    let path = Path::new(relative);
    if path.is_absolute() {
        return Inside::Absolute;
    }
    let resolved_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let start = fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
    // The walk is the resolution: the result is compared with the resolved
    // root, not the lexically normalized path.
    match walk(&start, path, LINK_DEPTH) {
        Ok(resolved) if resolved.starts_with(&resolved_root) => Inside::Yes,
        Ok(_) => Inside::Outside,
        // The path could not be resolved: the engine cannot tell where it
        // points, and a path it cannot resolve is not one it may touch.  Fail
        // closed.
        Err(Failure::Unresolvable) => Inside::Unresolvable,
        Err(Failure::Ambiguous) => Inside::Ambiguous,
    }
}

/// The same check for a path that is resolved against the repository root.
pub fn inside_repository(root: &Path, relative: &str) -> Inside {
    inside_root(root, root, relative)
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
