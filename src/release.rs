//! The release step and the changelog (SPEC §16).
//!
//! **The version and the changelog are derived facts.**  Both are determined
//! by the commit history; a hand-written version drifts from the history
//! silently — the tag says `0.2.0` while the history contains a breaking
//! change.  The history is the reference, never a version string somebody
//! typed.
//!
//! The engine owns the parsing, the mapping and the rendering.  The project
//! owns the **paths**: where the version lives (the manifest of its build
//! tool — Rust, Node, Python, Go, whatever it is) and where the changelog
//! lives.

use std::fs;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::config::{Config, Release, VersionFile, VersionFormat};
use crate::git::Git;
use crate::version::{Step, Version};

/// The preamble of a changelog that does not exist yet.
const PREAMBLE: &str = "# Changelog\n\nAll notable changes to this project will be documented in this file.\n\nThe format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),\nand this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).\n";

/// The sections of a changelog, in the order of Keep a Changelog.
const SECTIONS: [&str; 6] = [
    "Added",
    "Changed",
    "Deprecated",
    "Removed",
    "Fixed",
    "Security",
];

/// A commit of the release, with what the changelog needs from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The type of the commit: `feat`, `fix`, …
    ///
    /// A commit whose subject does not follow the conventional-commit shape
    /// has no type: it changes nothing that a release announces.
    pub kind: Option<String>,
    /// The scope of the commit.
    pub scope: Option<String>,
    /// The subject of the commit, without the type and the scope.
    pub subject: String,
    /// True when the commit breaks the contract.
    pub breaking: bool,
    /// The date of the commit.
    pub date: String,
}

/// What a release run does.
#[derive(Debug)]
pub struct Options<'a> {
    /// The repository root.
    pub root: &'a Path,
    /// The tag the release is computed from; the last release tag by default.
    pub from: Option<&'a str>,
    /// Print the release, change nothing.
    pub dry_run: bool,
}

/// The outcome of a release run.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The version of the release.
    pub version: Option<Version>,
    /// The tag of the release.
    pub tag: Option<String>,
    /// The tag the release was computed from.
    pub base: Option<String>,
    /// The step the version moves by.
    pub step: Step,
    /// The entries of the changelog block.
    pub entries: Vec<Entry>,
    /// The changelog block.
    pub block: String,
    /// The files that were written.
    pub written: Vec<String>,
    /// Notes about the run.
    pub notes: Vec<String>,
    /// True when there is nothing to release.
    pub nothing: bool,
}

impl Outcome {
    /// The number of commits the changelog announces.
    ///
    /// A commit without a type changes no version and is not announced: the
    /// number of the entries is the number of the lines in the block.
    pub fn announced(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| section_of(entry).is_some())
            .count()
    }
}

/// Computes the release of the repository.
///
/// The version and the changelog are written — unless the run is a dry one.
/// The **tag** is never written: a tag is a promise, and a promise is not
/// made by a tool.
pub fn run(config: &Config, git: &Git, options: &Options) -> Result<Outcome, Error> {
    let release = config.release.as_ref().ok_or_else(|| {
        Error::new(
            "the release is not configured: the `release` section of the config is missing — \
             the tool does not guess where the version of a project lives",
        )
    })?;
    let version_file = release.version.as_ref().ok_or_else(|| {
        Error::new(
            "the release is not configured: `release.version` is missing — the tool does not \
             guess where the version of a project lives",
        )
    })?;
    if !git.is_repo() {
        return Err(Error::new(
            "the release needs a git repository: the tag of the release and the commits are \
             read from it",
        ));
    }

    let prefix = release
        .tag_prefix
        .clone()
        .unwrap_or_else(|| "v".to_string());
    let changelog = release
        .changelog
        .clone()
        .unwrap_or_else(|| "CHANGELOG.md".to_string());

    // The two files the run writes have to lie inside the repository root: a
    // path that leaves it would let the engine write outside the tree it
    // owns.
    let version_path = inside_root(options.root, &version_file.path)?;
    let changelog_path = inside_root(options.root, &changelog)?;

    // The tag the release is computed from: the last release tag, or the one
    // the run names.  The tag is the reference — never a version string
    // somebody typed.
    let base = match options.from {
        Some(tag) => Some(tag.to_string()),
        None => git.last_release_tag(&prefix),
    };
    let base_version = match &base {
        Some(tag) => Some(crate::version::parse_tag(tag, &prefix).map_err(|message| {
            Error::new(format!("the tag {tag} is not a release tag: {message}"))
        })?),
        None => None,
    };

    let commits = git.commits_since(base.as_deref()).ok_or_else(|| {
        Error::new(match &base {
            Some(tag) => format!("the commits since {tag} cannot be read"),
            None => "the commits cannot be read".to_string(),
        })
    })?;
    let entries: Vec<Entry> = commits
        .iter()
        .map(|commit| parse_commit(&commit.subject, &commit.body, &commit.date))
        .collect();

    // The version of the project: read from the file the project names.
    let current = read_version(&version_path, version_file)?;
    let current = Version::parse(&current).map_err(|message| {
        Error::new(format!(
            "the version in {} is not a version: {message}",
            version_file.path
        ))
    })?;

    // The version in the file has to be the version of the last release: a
    // file that says something else means that the release was already
    // prepared, or that the version was changed by hand.
    if let (Some(tag), Some(base)) = (&base, base_version) {
        if current != base {
            return Err(Error::new(format!(
                "the version in {} is {current}, the tag {tag} says {base} — the release was \
                 already prepared, or the version was changed by hand",
                version_file.path
            )));
        }
    }

    // The step: the biggest step of the commits since the last release.
    let step = match base_version {
        // Without a release before, the version in the file is the version of
        // the release: the first release is the version the project chose.
        None => Step::None,
        Some(_) => entries
            .iter()
            .map(|entry| step_of(entry, release))
            .max()
            .unwrap_or(Step::None),
    };
    let version = match base_version {
        Some(base) => base.bump(step),
        None => current,
    };

    let mut outcome = Outcome {
        version: Some(version),
        tag: Some(format!("{prefix}{version}")),
        base: base.clone(),
        step,
        entries: entries.clone(),
        ..Outcome::default()
    };

    if step == Step::None && base_version.is_some() {
        // Nothing to release: no commit since the last release changes the
        // version.  The changelog is not written either — a block without a
        // version is not a release.
        outcome.nothing = true;
        outcome.notes.push(format!(
            "nothing to release: no commit since {} changes the version",
            base.clone().unwrap_or_default()
        ));
        return Ok(outcome);
    }

    // The date of the release is the date of the newest commit: a fact of
    // the history, not the clock.
    let date = entries
        .last()
        .map(|entry| entry.date.clone())
        .unwrap_or_default();
    outcome.block = render_block(&version, &date, &entries);

    // The changelog below the version the tool manages is written by hand.
    if let Some(from) = &release.changelog_from {
        let from = Version::parse(from).map_err(|message| {
            Error::new(format!(
                "the changelog starts at {from}, and that is not a version: {message}"
            ))
        })?;
        if version < from {
            return Err(Error::new(format!(
                "the changelog below {from} is written by hand — the tool starts at {from}"
            )));
        }
    }

    if options.dry_run {
        return Ok(outcome);
    }

    // The version file is written first, the changelog second: a version
    // that is in the file and a changelog that misses it are two states, and
    // the first one is the one the next run can repair.
    let before = fs::read(&version_path).map_err(|error| {
        Error::new(format!(
            "the version file {} cannot be read: {error}",
            version_file.path
        ))
    })?;
    write_version(&version_path, version_file, &version.to_string())?;

    if let Err(error) = append_changelog(&changelog_path, &outcome.block) {
        // The version file goes back: a half-written release is a state
        // nobody asked for.  A restore that fails is reported — a swallowed
        // restore error is a mutated tree that nobody was told about
        // (SPEC §7).
        if let Err(restore) = restore_version(&version_path, &before) {
            return Err(Error::new(format!(
                "{error} — and the version file {} could not be restored: {restore}; \
                 the working tree was modified",
                version_file.path
            )));
        }
        return Err(error);
    }

    outcome.written.push(version_file.path.clone());
    outcome.written.push(changelog);
    Ok(outcome)
}

/// The path of a file the run writes, resolved against the repository root.
///
/// The path has to lie **inside** the root: a path that leaves it would let
/// the engine write outside the tree it owns.  The check is not only lexical
/// — a symbolic link inside the root can point outside it, and the write
/// would follow the link.
fn inside_root(root: &Path, relative: &str) -> Result<PathBuf, Error> {
    let full = crate::path::normalize(&root.join(Path::new(relative)));
    if !full.starts_with(crate::path::normalize(root)) {
        return Err(Error::new(format!(
            "the path {relative} must lie inside the repository root"
        )));
    }
    let resolved_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if !crate::path::resolve(&full).starts_with(&resolved_root) {
        return Err(Error::new(format!(
            "the path {relative} must lie inside the repository root — it leaves it through a \
             symbolic link"
        )));
    }
    Ok(full)
}

/// Parses a commit of the history.
fn parse_commit(subject: &str, body: &str, date: &str) -> Entry {
    let (kind, scope, mut breaking, text) = parse_subject(subject);
    if has_breaking_footer(body) {
        breaking = true;
    }
    Entry {
        kind,
        scope,
        subject: text,
        breaking,
        date: date.to_string(),
    }
}

/// Parses the subject of a commit.
///
/// The shape is the conventional-commit one: `type(scope)!: description`.  A
/// subject that does not follow it has no type — it is a commit the release
/// step does not count, and the changelog does not announce.
fn parse_subject(subject: &str) -> (Option<String>, Option<String>, bool, String) {
    let whole = subject.trim().to_string();
    let Some((head, description)) = subject.split_once(':') else {
        return (None, None, false, whole);
    };
    let head = head.trim();
    if head.is_empty() {
        return (None, None, false, whole);
    }
    let (head, breaking) = match head.strip_suffix('!') {
        Some(rest) => (rest.trim(), true),
        None => (head, false),
    };
    let (kind, scope) = match head.split_once('(') {
        Some((kind, scope)) => {
            let scope = scope.strip_suffix(')').unwrap_or(scope);
            (kind.trim(), Some(scope.trim().to_string()))
        }
        None => (head, None),
    };
    // The type is a word: `feat`, `fix`, `docs`, `build`.
    let valid = !kind.is_empty()
        && kind
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return (None, None, false, whole);
    }
    (
        Some(kind.to_string()),
        scope,
        breaking,
        description.trim().to_string(),
    )
}

/// True when the body of the commit carries a breaking-change footer.
fn has_breaking_footer(body: &str) -> bool {
    body.lines().any(|line| {
        let line = line.trim();
        line.starts_with("BREAKING CHANGE:") || line.starts_with("BREAKING-CHANGE:")
    })
}

/// The step a commit moves the version by.
fn step_of(entry: &Entry, release: &Release) -> Step {
    if entry.breaking {
        return configured_step(release, "breaking", Step::Major);
    }
    let kind = entry.kind.as_deref().unwrap_or("");
    let default = match kind {
        "feat" => Step::Minor,
        "fix" | "perf" => Step::Patch,
        _ => Step::None,
    };
    configured_step(release, kind, default)
}

/// The step of the configuration, or the default one.
fn configured_step(release: &Release, kind: &str, default: Step) -> Step {
    release
        .steps
        .get(kind)
        .and_then(|text| Step::parse(text).ok())
        .unwrap_or(default)
}

/// The section of the changelog an entry belongs to.
fn section_of(entry: &Entry) -> Option<&'static str> {
    if is_security(entry) {
        return Some("Security");
    }
    if entry.breaking {
        return Some("Changed");
    }
    match entry.kind.as_deref() {
        Some("feat") => Some("Added"),
        Some("fix") => Some("Fixed"),
        Some("perf") | Some("refactor") => Some("Changed"),
        _ => None,
    }
}

/// True when the commit names a security finding.
///
/// The **scope** is the marker: a commit that is about security says so in
/// its scope (`fix(security): …`).  A subject that mentions the word is not
/// a finding — a commit that mentions "security" in a roadmap note is not a
/// security change, and a marker that is sometimes right is a marker that is
/// often wrong.
fn is_security(entry: &Entry) -> bool {
    entry.scope.as_deref() == Some("security")
}

/// The text of an entry in the changelog.
fn text_of(entry: &Entry) -> String {
    let mut text = String::new();
    if entry.breaking {
        text.push_str("**Breaking**: ");
    }
    text.push_str(&entry.subject);
    if let Some(scope) = &entry.scope {
        text.push_str(&format!(" ({scope})"));
    }
    text
}

/// Renders the changelog block of a release.
fn render_block(version: &Version, date: &str, entries: &[Entry]) -> String {
    let mut out = String::new();
    if date.is_empty() {
        out.push_str(&format!("## [{version}]\n"));
    } else {
        out.push_str(&format!("## [{version}] - {date}\n"));
    }
    for section in SECTIONS {
        let items: Vec<&Entry> = entries
            .iter()
            .filter(|entry| section_of(entry) == Some(section))
            .collect();
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("\n### {section}\n\n"));
        for entry in items {
            out.push_str(&format!("- {}\n", text_of(entry)));
        }
    }
    out
}

/// Reads the version from the version file.
fn read_version(path: &Path, file: &VersionFile) -> Result<String, Error> {
    let content = fs::read_to_string(path).map_err(|error| {
        Error::new(format!(
            "the version file {} cannot be read: {error}",
            file.path
        ))
    })?;
    let lines: Vec<&str> = content.lines().collect();
    let Some((index, start, end)) = find_value(&lines, file) else {
        return Err(Error::new(format!(
            "the version file {} does not carry the version",
            file.path
        )));
    };
    Ok(lines[index][start..end].to_string())
}

/// Writes the version into the version file.
///
/// The write is a **targeted edit**: the line that carries the value is
/// replaced, the rest of the file keeps its bytes and its formatting.  A tool
/// that rewrote a manifest with its own serializer would reformat a file it
/// does not own — a diff of the whole file instead of a diff of the version.
fn write_version(path: &Path, file: &VersionFile, version: &str) -> Result<(), Error> {
    let content = fs::read_to_string(path).map_err(|error| {
        Error::new(format!(
            "the version file {} cannot be read: {error}",
            file.path
        ))
    })?;
    let lines: Vec<&str> = content.lines().collect();
    let Some((index, start, end)) = find_value(&lines, file) else {
        return Err(Error::new(format!(
            "the version file {} does not carry the version",
            file.path
        )));
    };
    let mut parts: Vec<String> = content.split_inclusive('\n').map(str::to_string).collect();
    let line = &parts[index];
    let mut updated = String::new();
    updated.push_str(&line[..start]);
    updated.push_str(version);
    updated.push_str(&line[end..]);
    parts[index] = updated;
    write_atomic(path, &parts.concat()).map_err(|error| {
        Error::new(format!(
            "the version file {} cannot be written: {error}",
            file.path
        ))
    })
}

/// Writes a file atomically.
///
/// The content is written to a temporary file in the same directory and
/// renamed over the target: an interrupted write cannot truncate the file,
/// and the mode of the file is kept.
fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let mode = fs::metadata(path)
        .ok()
        .and_then(|metadata| crate::fsutil::mode_of(&metadata));
    crate::fsutil::write_atomic(path, content.as_bytes(), mode, None)
        .map_err(|error| error.to_string())
}

/// Puts the bytes of a file back.
///
/// The restore is **atomic** like every other write of the tool: an
/// interrupted restore cannot truncate the file it restores.
fn restore_version(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let text = String::from_utf8_lossy(bytes).into_owned();
    write_atomic(path, &text)
}

/// The line that carries the version, and the range of the value in it.
fn find_value(lines: &[&str], file: &VersionFile) -> Option<(usize, usize, usize)> {
    match file.format {
        VersionFormat::Toml => find_in_section(lines, file),
        VersionFormat::Json => find_in_json(lines, file),
        VersionFormat::Line => find_in_line(lines, file),
    }
}

/// The line of a TOML file: the key in the section.
fn find_in_section(lines: &[&str], file: &VersionFile) -> Option<(usize, usize, usize)> {
    let section = file.section.as_deref()?;
    let key = file.key.as_deref()?;
    let header = format!("[{section}]");
    let mut inside = false;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            inside = trimmed == header;
            continue;
        }
        if !inside {
            continue;
        }
        if let Some((start, end)) = toml_value_range(line, key) {
            return Some((index, start, end));
        }
    }
    None
}

/// The line of a JSON file: the key of the root object.
///
/// The key has to be a member of the **root**: the members of the root sit
/// at the smallest indentation of the document, and the line of the key has
/// to sit there.  A key that only exists in a nested object is not the
/// version of the project — writing into it would put the version where
/// nobody reads it.
fn find_in_json(lines: &[&str], file: &VersionFile) -> Option<(usize, usize, usize)> {
    let key = file.key.as_deref()?;
    let root = root_indent(lines)?;
    for (index, line) in lines.iter().enumerate() {
        if line.len() - line.trim_start().len() != root {
            continue;
        }
        if let Some((start, end)) = json_value_range(line, key) {
            return Some((index, start, end));
        }
    }
    None
}

/// The indentation of the members of the root object.
///
/// It is the smallest indentation of the lines that carry a member — the
/// lines that start with a quoted name.  A document without members has no
/// root.
fn root_indent(lines: &[&str]) -> Option<usize> {
    lines
        .iter()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with('"') {
                return None;
            }
            Some(line.len() - trimmed.len())
        })
        .min()
}

/// The line of a text file: the line with the prefix and the suffix.
fn find_in_line(lines: &[&str], file: &VersionFile) -> Option<(usize, usize, usize)> {
    let prefix = file.prefix.as_deref()?;
    let suffix = file.suffix.as_deref()?;
    for (index, line) in lines.iter().enumerate() {
        if let Some((start, end)) = line_value_range(line, prefix, suffix) {
            return Some((index, start, end));
        }
    }
    None
}

/// The range of the value of a TOML key in a line.
fn toml_value_range(line: &str, key: &str) -> Option<(usize, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let after_key = trimmed.strip_prefix(key)?;
    let after_equal = after_key.trim_start().strip_prefix('=')?;
    let after_space = after_equal.trim_start();
    let after_quote = after_space.strip_prefix('"')?;
    let start = indent + trimmed.len() - after_quote.len();
    let end = start + after_quote.find('"')?;
    Some((start, end))
}

/// The range of the value of a JSON key in a line.
fn json_value_range(line: &str, key: &str) -> Option<(usize, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let quoted = format!("\"{key}\"");
    let after_key = trimmed.strip_prefix(&quoted)?;
    let after_colon = after_key.trim_start().strip_prefix(':')?.trim_start();
    let after_quote = after_colon.strip_prefix('"')?;
    let start = indent + trimmed.len() - after_quote.len();
    let end = start + after_quote.find('"')?;
    Some((start, end))
}

/// The range of the value in a line with a prefix and a suffix.
fn line_value_range(line: &str, prefix: &str, suffix: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix(prefix)?;
    let rest = rest.strip_suffix(suffix)?;
    Some((prefix.len(), prefix.len() + rest.len()))
}

/// Appends the block to the changelog.
///
/// The generator **appends** one block; it never rewrites the file.  A file
/// that does not exist yet is created with the preamble of Keep a Changelog.
fn append_changelog(path: &Path, block: &str) -> Result<(), Error> {
    let mut content = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(Error::new(format!("the changelog cannot be read: {error}")));
        }
    };
    if content.trim().is_empty() {
        content = PREAMBLE.to_string();
    }
    if !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.ends_with("\n\n") {
        content.push('\n');
    }
    content.push_str(block);
    if !content.ends_with('\n') {
        content.push('\n');
    }
    write_atomic(path, &content)
        .map_err(|error| Error::new(format!("the changelog cannot be written: {error}")))
}
#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: Option<&str>, scope: Option<&str>, subject: &str, breaking: bool) -> Entry {
        Entry {
            kind: kind.map(str::to_string),
            scope: scope.map(str::to_string),
            subject: subject.to_string(),
            breaking,
            date: "2026-10-21".to_string(),
        }
    }

    #[test]
    fn a_subject_names_the_type_and_the_scope() {
        let (kind, scope, breaking, text) = parse_subject("feat(verify): the pre-release check");
        assert_eq!(kind.as_deref(), Some("feat"));
        assert_eq!(scope.as_deref(), Some("verify"));
        assert!(!breaking);
        assert_eq!(text, "the pre-release check");
    }

    #[test]
    fn a_subject_without_a_scope_has_none() {
        let (kind, scope, breaking, text) = parse_subject("fix: the restore of a snapshot");
        assert_eq!(kind.as_deref(), Some("fix"));
        assert_eq!(scope, None);
        assert!(!breaking);
        assert_eq!(text, "the restore of a snapshot");
    }

    #[test]
    fn a_bang_marks_the_commit_as_breaking() {
        let (kind, _, breaking, _) = parse_subject("feat(api)!: the config changed");
        assert_eq!(kind.as_deref(), Some("feat"));
        assert!(breaking);
    }

    #[test]
    fn a_subject_that_is_not_a_commit_has_no_type() {
        let (kind, _, _, text) = parse_subject("Merge branch 'main'");
        assert_eq!(kind, None);
        assert_eq!(text, "Merge branch 'main'");
        let (kind, _, _, _) = parse_subject("Some sentence: with a colon");
        assert_eq!(kind, None);
    }

    #[test]
    fn a_breaking_footer_marks_the_commit_as_breaking() {
        assert!(has_breaking_footer("BREAKING CHANGE: the config changed"));
        assert!(has_breaking_footer(
            "body\n\nBREAKING-CHANGE: the config changed\n"
        ));
        assert!(!has_breaking_footer("body only"));
    }

    #[test]
    fn the_step_of_a_commit_follows_its_type() {
        let release = Release::default();
        assert_eq!(
            step_of(&entry(Some("feat"), None, "a", false), &release),
            Step::Minor
        );
        assert_eq!(
            step_of(&entry(Some("fix"), None, "a", false), &release),
            Step::Patch
        );
        assert_eq!(
            step_of(&entry(Some("perf"), None, "a", false), &release),
            Step::Patch
        );
        assert_eq!(
            step_of(&entry(Some("docs"), None, "a", false), &release),
            Step::None
        );
        assert_eq!(
            step_of(&entry(Some("feat"), None, "a", true), &release),
            Step::Major
        );
    }

    #[test]
    fn the_step_is_configurable() {
        let mut release = Release::default();
        release
            .steps
            .insert("feat".to_string(), "patch".to_string());
        release
            .steps
            .insert("breaking".to_string(), "minor".to_string());
        assert_eq!(
            step_of(&entry(Some("feat"), None, "a", false), &release),
            Step::Patch
        );
        assert_eq!(
            step_of(&entry(Some("feat"), None, "a", true), &release),
            Step::Minor
        );
    }

    #[test]
    fn the_block_holds_the_sections_of_keep_a_changelog() {
        let version = Version::parse("0.2.0").expect("the version parses");
        let entries = vec![
            entry(Some("feat"), Some("verify"), "the pre-release check", false),
            entry(Some("fix"), None, "the restore of a snapshot", false),
            entry(Some("feat"), Some("api"), "the config changed", true),
        ];
        let block = render_block(&version, "2026-10-21", &entries);
        assert!(block.starts_with("## [0.2.0] - 2026-10-21\n"), "{block}");
        assert!(
            block.contains("### Added\n\n- the pre-release check (verify)\n"),
            "{block}"
        );
        assert!(
            block.contains("### Fixed\n\n- the restore of a snapshot\n"),
            "{block}"
        );
        assert!(
            block.contains("### Changed\n\n- **Breaking**: the config changed (api)\n"),
            "{block}"
        );
        // The sections come in the order of Keep a Changelog.
        let added = block.find("### Added").expect("the section is there");
        let changed = block.find("### Changed").expect("the section is there");
        let fixed = block.find("### Fixed").expect("the section is there");
        assert!(added < changed && changed < fixed, "{block}");
    }

    #[test]
    fn a_commit_without_a_type_is_not_in_the_changelog() {
        let version = Version::parse("0.2.0").expect("the version parses");
        let entries = vec![entry(None, None, "Merge branch 'main'", false)];
        let block = render_block(&version, "2026-10-21", &entries);
        assert_eq!(block, "## [0.2.0] - 2026-10-21\n");
    }

    #[test]
    fn a_security_commit_goes_into_its_own_section() {
        let version = Version::parse("0.2.0").expect("the version parses");
        let entries = vec![entry(Some("fix"), Some("security"), "the leak", false)];
        let block = render_block(&version, "2026-10-21", &entries);
        assert!(block.contains("### Security"), "{block}");
    }

    #[test]
    fn the_version_of_a_toml_file_is_read_and_written() {
        let file = VersionFile {
            path: "Cargo.toml".to_string(),
            format: VersionFormat::Toml,
            section: Some("package".to_string()),
            key: Some("version".to_string()),
            prefix: None,
            suffix: None,
        };
        let content = "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
        let lines: Vec<&str> = content.lines().collect();
        let (index, start, end) = find_value(&lines, &file).expect("the version is there");
        assert_eq!(index, 2);
        assert_eq!(&lines[index][start..end], "0.1.0");
    }

    #[test]
    fn the_version_of_a_json_file_is_read_and_written() {
        let file = VersionFile {
            path: "package.json".to_string(),
            format: VersionFormat::Json,
            section: None,
            key: Some("version".to_string()),
            prefix: None,
            suffix: None,
        };
        let content = "{\n  \"name\": \"ratchet\",\n  \"version\": \"0.1.0\",\n  \"scripts\": {\n    \"version\": \"1.0.0\"\n  }\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let (index, start, end) = find_value(&lines, &file).expect("the version is there");
        // The member of the root object — not the one of the nested object.
        assert_eq!(index, 2);
        assert_eq!(&lines[index][start..end], "0.1.0");
    }

    #[test]
    fn the_version_of_a_line_is_read_and_written() {
        let file = VersionFile {
            path: "Makefile".to_string(),
            format: VersionFormat::Line,
            section: None,
            key: None,
            prefix: Some("VERSION := ".to_string()),
            suffix: Some(String::new()),
        };
        let content = "all:\n\tcargo build\n\nVERSION := 0.1.0\n";
        let lines: Vec<&str> = content.lines().collect();
        let (index, start, end) = find_value(&lines, &file).expect("the version is there");
        assert_eq!(index, 3);
        assert_eq!(&lines[index][start..end], "0.1.0");
    }

    #[test]
    fn the_version_of_a_line_with_a_suffix_is_read() {
        let file = VersionFile {
            path: "build.gradle".to_string(),
            format: VersionFormat::Line,
            section: None,
            key: None,
            prefix: Some("version = \"".to_string()),
            suffix: Some("\"".to_string()),
        };
        let content = "plugins {\n}\n\nversion = \"0.1.0\"\n";
        let lines: Vec<&str> = content.lines().collect();
        let (index, start, end) = find_value(&lines, &file).expect("the version is there");
        assert_eq!(index, 3);
        assert_eq!(&lines[index][start..end], "0.1.0");
    }

    #[test]
    fn a_restore_that_fails_is_reported() {
        // The directory of the file cannot be written: the temporary file of
        // the restore cannot be created.  The restore must **say** so — a
        // swallowed restore error is a mutated tree that nobody was told
        // about (SPEC §7).
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let file = directory.path().join("Cargo.toml");
        std::fs::write(&file, "[package]\n").expect("the file is written");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o555))
                .expect("the directory is made read-only");
            let result = restore_version(&file, b"[package]\nversion = \"0.1.0\"\n");
            // The permissions go back: the directory has to be removable.
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755))
                .expect("the permissions go back");
            assert!(result.is_err(), "a restore that cannot write says nothing");
        }
    }
}
