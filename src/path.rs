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
                let target = if is_absolute(&target) {
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
pub fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

/// The root, resolved: the directory the paths of a project are compared with.
///
/// The paths are resolved against the **canonical** root: a link in the path
/// of the root itself would carry the comparison out of the tree otherwise.
pub fn canonical_root(root: &Path) -> PathBuf {
    fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// True when the path lies inside the repository root.
///
/// The root itself counts: a file at the root of the tree is inside it.
pub fn inside_root_path(root: &Path, path: &Path) -> bool {
    path.starts_with(canonical_root(root))
}

/// True when the directory lies **strictly** inside the repository root.
///
/// The root itself does not: it exists, and there is nothing to create there.
/// A directory outside the root is not the engine's to touch.
pub fn inside_root_directory(root: &Path, directory: &Path) -> bool {
    inside_root_path(root, directory) && directory != canonical_root(root)
}

/// Why a path was refused: it does not lie inside the repository root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
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

impl Refusal {
    /// The sentence of the refusal.
    ///
    /// `subject` names the path in the caller's words — "the generated file
    /// docs/x.md", "docs/x.json" — and `outside` is the sentence for a path
    /// that leaves the root: it differs per subject ("must lie inside the
    /// repository root", "is outside the repository root").
    pub fn message(self, subject: &str, outside: &str) -> String {
        match self {
            // The paths of a project are relative: an absolute path names a
            // place on one machine, and a committed config has to work on
            // every machine.
            Refusal::Absolute => {
                format!("{subject} has to be a relative path — the paths of a project are relative")
            }
            Refusal::Outside => format!("{subject} {outside}"),
            // A path that cannot be resolved is not inside the root: the
            // engine cannot tell where it points, so it refuses it.
            Refusal::Unresolvable => format!(
                "{subject} could not be resolved — the links of the path could not be followed \
                 to the end"
            ),
            // The two readings differ: the tool does not pick one.
            Refusal::Ambiguous => format!(
                "{subject} is ambiguous — a \"..\" after a symbolic link has two readings, and \
                 the tool does not choose one"
            ),
        }
    }
}

/// The path a project names, resolved — when it lies inside the repository
/// root.
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
///
/// The `Ok` carries the path the named path **resolves to**: the place the
/// operating system puts the file, after the links of the path have been
/// followed.  It is the path the engine may touch — and the place a
/// directory of the file has to be created: a caller that creates a
/// directory needs the resolved path, not the named one (the drift gate,
/// SPEC §7).  The `Err` carries the reason the path was refused.
pub fn inside_root(root: &Path, base: &Path, relative: &str) -> Result<PathBuf, Refusal> {
    let path = Path::new(relative);
    if is_absolute(path) {
        return Err(Refusal::Absolute);
    }
    let start = fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
    // The walk is the resolution: the result is compared with the resolved
    // root, not the lexically normalized path.
    match walk(&start, path, LINK_DEPTH) {
        Ok(resolved) if inside_root_path(root, &resolved) => Ok(resolved),
        Ok(_) => Err(Refusal::Outside),
        // The path could not be resolved: the engine cannot tell where it
        // points, and a path it cannot resolve is not one it may touch.  Fail
        // closed.
        Err(Failure::Unresolvable) => Err(Refusal::Unresolvable),
        Err(Failure::Ambiguous) => Err(Refusal::Ambiguous),
    }
}

/// True when a path has a root or platform prefix. `Path::is_absolute()` is
/// false for rooted-but-unprefixed Windows paths such as `\\etc\\file`, and
/// for drive-relative paths such as `C:file`; neither is portable config data.
pub fn is_absolute(path: &Path) -> bool {
    path.is_absolute()
        || path.has_root()
        || path
            .components()
            .any(|component| matches!(component, Component::Prefix(_)))
}

/// The same check for a path that is resolved against the repository root.
pub fn inside_repository(root: &Path, relative: &str) -> Result<PathBuf, Refusal> {
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

    #[cfg(windows)]
    #[test]
    fn refuses_rooted_and_drive_relative_windows_paths() {
        for value in [r"\outside\file", r"C:\outside\file", r"C:outside\file"] {
            assert!(
                is_absolute(Path::new(value)),
                "{value:?} must not be accepted as portable relative config data"
            );
            assert_eq!(
                inside_root(Path::new("."), Path::new("."), value),
                Err(Refusal::Absolute),
                "{value:?}"
            );
        }
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

    #[cfg(unix)]
    #[test]
    fn a_path_behind_a_link_resolves_to_its_target() {
        // The helper hands the resolved path to the caller: the place the
        // operating system puts the file — the place a directory of the file
        // has to be created.
        let root = crate::test_tempdir::TempDir::new().expect("a temporary directory");
        std::os::unix::fs::symlink("sub", root.path().join("docs")).expect("the link is made");

        let resolved = inside_repository(root.path(), "docs/generated.md")
            .expect("the path lies inside the root");

        let root = fs::canonicalize(root.path()).expect("the root is readable");
        assert_eq!(resolved, root.join("sub").join("generated.md"));
    }
}
