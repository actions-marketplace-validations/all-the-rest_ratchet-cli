//! The release step and the changelog (SPEC §16).
//!
//! The tests run the real binary in a temporary git repository: the version
//! comes from the history, and the history is what the tests build.

use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

/// A config with the release section and no rule.
fn config(version: &str) -> String {
    format!(r#"[{{ "release": {{ "changelog": "CHANGELOG.md", "version": {version} }} }}]"#)
}

/// The version file of a Rust project.
const TOML_VERSION: &str =
    r#"{ "path": "Cargo.toml", "format": "toml", "section": "package", "key": "version" }"#;

/// The version file of a Node project.
const JSON_VERSION: &str = r#"{ "path": "package.json", "format": "json", "key": "version" }"#;

/// The version file of a project that keeps the version in a line.
const LINE_VERSION: &str =
    r#"{ "path": "Makefile", "format": "line", "prefix": "VERSION := ", "suffix": "" }"#;

/// A temporary git repository.
struct Repo {
    dir: TempDir,
}

impl Repo {
    /// Creates a repository with a config, committed.
    fn new(config: &str) -> Self {
        let dir = TempDir::new().expect("a temporary directory");
        let repo = Self { dir };
        repo.write("ratchet.config.jsonc", config);
        repo.git(&["init", "-q", "."]);
        repo.git(&["add", "-A"]);
        repo.commit("init");
        repo
    }

    /// Creates a directory with a config, but no repository.
    fn plain(config: &str) -> Self {
        let dir = TempDir::new().expect("a temporary directory");
        let repo = Self { dir };
        repo.write("ratchet.config.jsonc", config);
        repo
    }

    /// Writes a file into the repository.
    fn write(&self, name: &str, content: &str) {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the parent directory is created");
        }
        std::fs::write(path, content).expect("the file is written");
    }

    /// Reads a file from the repository.
    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(name)).expect("the file is readable")
    }

    /// The tags of the repository, sorted.
    fn tags(&self) -> Vec<String> {
        let output = Command::new("git")
            .args(["tag", "--list"])
            .current_dir(self.path())
            .output()
            .expect("git runs");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The state of the working tree.
    fn status(&self) -> String {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(self.path())
            .output()
            .expect("git runs");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Runs a git command in the repository.
    fn git(&self, arguments: &[&str]) {
        let status = Command::new("git")
            .args(arguments)
            .current_dir(self.path())
            .env("GIT_AUTHOR_NAME", "ratchet test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "ratchet test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {arguments:?} failed");
    }

    /// Commits everything that is in the working tree.
    ///
    /// The commit may be empty: the history is what the release reads, and a
    /// test needs a commit without a change in the tree.
    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
    }

    /// Commits with a fixed date.
    ///
    /// The date of the commit is a fact of the history: the changelog block
    /// carries it, and it must not be the date of the day the test runs.
    fn commit_at(&self, message: &str, date: &str) {
        self.git(&["add", "-A"]);
        Command::new("git")
            .args(["commit", "-q", "--allow-empty", "-m", message])
            .current_dir(self.path())
            .env("GIT_AUTHOR_NAME", "ratchet test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "ratchet test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .status()
            .expect("git runs");
    }

    /// The root of the repository.
    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Runs the binary in the repository.
    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ratchet"))
            .args(arguments)
            .current_dir(self.path())
            .env("RATCHET_NO_UPDATE_CHECK", "1")
            .output()
            .expect("the binary runs")
    }
}

/// The exit code of a run.
fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

/// The stdout of a run.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The stderr of a run.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A repository with a released version and a manifest that carries it.
fn released_repo() -> Repo {
    let repo = Repo::new(&config(TOML_VERSION));
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    repo.commit("the manifest");
    repo.git(&["tag", "v0.1.0"]);
    repo
}

#[test]
fn the_release_computes_the_next_version_from_the_commits() {
    let repo = released_repo();
    repo.commit("feat(verify): the pre-release check");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    // The version of the manifest moved — and nothing else of the file did.
    let manifest = repo.read("Cargo.toml");
    assert!(manifest.contains("version = \"0.2.0\""), "{manifest}");
    assert!(manifest.contains("name = \"ratchet\""), "{manifest}");
    assert!(manifest.contains("edition = \"2024\""), "{manifest}");
    // The changelog holds the block of the release.
    let changelog = repo.read("CHANGELOG.md");
    assert!(changelog.contains("## [0.2.0] - "), "{changelog}");
    assert!(changelog.contains("### Added"), "{changelog}");
    assert!(
        changelog.contains("- the pre-release check (verify)"),
        "{changelog}"
    );
    // The tag is not written: a tag is a promise, and a promise is not made
    // by a tool.
    assert_eq!(repo.tags(), vec!["v0.1.0".to_string()]);
}

#[test]
fn the_release_names_the_tag_of_the_release() {
    let repo = released_repo();
    repo.commit("fix: the restore of a snapshot");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("v0.1.1"), "{text}");
    assert!(text.contains("patch"), "{text}");
}

#[test]
fn a_dry_run_changes_nothing() {
    let repo = released_repo();
    repo.commit("feat: a feature");

    let output = repo.run(&["release", "--dry-run"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        repo.read("Cargo.toml").contains("version = \"0.1.0\""),
        "the dry run wrote the version"
    );
    assert!(
        !repo.path().join("CHANGELOG.md").exists(),
        "the dry run wrote the changelog"
    );
    assert_eq!(repo.status(), "", "the dry run changed the tree");
}

#[test]
fn a_release_without_a_change_of_the_version_is_nothing_to_release() {
    let repo = released_repo();
    repo.commit("docs: the readme");

    let output = repo.run(&["release"]);

    // Nothing to release is not a failure of the tool, but the release that
    // was asked for does not exist.
    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("nothing to release"),
        "{}",
        stderr(&output)
    );
    assert!(
        repo.read("Cargo.toml").contains("version = \"0.1.0\""),
        "the run wrote the version"
    );
}

#[test]
fn a_version_that_is_not_the_one_of_the_tag_is_refused() {
    let repo = released_repo();
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.3.0\"\n",
    );
    repo.commit("the version was changed by hand");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let message = stderr(&output);
    assert!(message.contains("0.3.0"), "{message}");
    assert!(message.contains("v0.1.0"), "{message}");
    assert!(
        repo.read("Cargo.toml").contains("version = \"0.3.0\""),
        "the run wrote the version"
    );
}

#[test]
fn the_changelog_below_the_managed_version_is_not_written() {
    let repo = Repo::new(
        r#"[{ "release": { "changelog": "CHANGELOG.md", "changelogFrom": "0.3.0",
             "version": { "path": "Cargo.toml", "format": "toml", "section": "package", "key": "version" } } }]"#,
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\n",
    );
    repo.commit("the manifest");
    repo.git(&["tag", "v0.1.0"]);
    repo.commit("feat: a feature");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("written by hand"),
        "{}",
        stderr(&output)
    );
    assert!(
        !repo.path().join("CHANGELOG.md").exists(),
        "the run wrote the changelog"
    );
    assert!(
        repo.read("Cargo.toml").contains("version = \"0.1.0\""),
        "the run wrote the version"
    );
}

#[test]
fn the_release_writes_the_version_into_a_json_manifest() {
    let repo = Repo::new(&config(JSON_VERSION));
    repo.write(
        "package.json",
        "{\n  \"name\": \"ratchet\",\n  \"version\": \"0.1.0\",\n  \"scripts\": {}\n}\n",
    );
    repo.commit("the manifest");
    repo.git(&["tag", "v0.1.0"]);
    repo.commit("feat: a feature");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let manifest = repo.read("package.json");
    assert!(manifest.contains("\"version\": \"0.2.0\""), "{manifest}");
    // The rest of the file keeps its bytes: the write is a targeted edit.
    assert!(manifest.contains("\"name\": \"ratchet\""), "{manifest}");
    assert!(manifest.contains("\"scripts\": {}"), "{manifest}");
}

#[test]
fn the_release_writes_the_version_into_a_line() {
    let repo = Repo::new(&config(LINE_VERSION));
    repo.write("Makefile", "all:\n\tcargo build\n\nVERSION := 0.1.0\n");
    repo.commit("the makefile");
    repo.git(&["tag", "v0.1.0"]);
    repo.commit("feat: a feature");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        repo.read("Makefile"),
        "all:\n\tcargo build\n\nVERSION := 0.2.0\n"
    );
}

#[test]
fn the_release_needs_a_repository() {
    let repo = Repo::plain(&config(TOML_VERSION));
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\n",
    );

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("git repository"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_config_without_the_version_file_is_refused() {
    let repo = Repo::new(r#"[{ "release": { "changelog": "CHANGELOG.md" } }]"#);
    repo.commit("a commit");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("does not guess"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn two_releases_of_the_same_history_produce_the_same_block() {
    let repo = released_repo();
    repo.commit("feat: a feature");
    repo.commit("fix: another thing");

    let first = repo.run(&["release", "--dry-run"]);
    let second = repo.run(&["release", "--dry-run"]);

    assert_eq!(code(&first), 0, "{}", stderr(&first));
    assert_eq!(stdout(&first), stdout(&second));
}

#[test]
fn the_release_is_json() {
    let repo = released_repo();
    repo.commit("feat: a feature");

    let output = repo.run(&["release", "--dry-run", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("the output is JSON");
    assert_eq!(value["version"], "0.2.0");
    assert_eq!(value["tag"], "v0.2.0");
    assert_eq!(value["step"], "minor");
    assert_eq!(value["nothing"], false);
    assert_eq!(value["entries"][0]["subject"], "a feature");
    assert!(value["block"].as_str().unwrap().contains("## [0.2.0]"));
}

#[test]
fn the_date_of_the_release_is_the_date_of_the_history() {
    let repo = released_repo();
    repo.commit_at("feat: a feature", "2026-03-04T10:00:00+00:00");

    let output = repo.run(&["release", "--dry-run"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    // The date comes from the commit, not from the day the run happens.
    assert!(text.contains("## [0.2.0] - 2026-03-04"), "{text}");
}

#[test]
fn the_changelog_is_appended_and_never_rewritten() {
    let repo = Repo::new(&config(TOML_VERSION));
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\n",
    );
    repo.write(
        "CHANGELOG.md",
        "# Changelog\n\nThe history that was written by hand.\n\n## [0.1.0] - 2026-01-01\n\n- the first release\n",
    );
    repo.commit("the files");
    repo.git(&["tag", "v0.1.0"]);
    repo.commit("feat: a feature");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let changelog = repo.read("CHANGELOG.md");
    // The old content is still there — byte for byte.
    assert!(
        changelog.starts_with("# Changelog\n\nThe history that was written by hand.\n"),
        "{changelog}"
    );
    assert!(changelog.contains("## [0.1.0] - 2026-01-01"), "{changelog}");
    assert!(changelog.contains("- the first release"), "{changelog}");
    // And the new block follows it.
    assert!(changelog.contains("## [0.2.0] - "), "{changelog}");
    let old = changelog
        .find("## [0.1.0]")
        .expect("the old block is there");
    let new = changelog
        .find("## [0.2.0]")
        .expect("the new block is there");
    assert!(old < new, "{changelog}");
}

#[test]
fn the_commits_are_in_the_order_of_the_history() {
    let repo = released_repo();
    repo.commit("feat: the first one");
    repo.commit("feat: the second one");
    repo.commit("feat: the third one");

    let output = repo.run(&["release", "--dry-run"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    let first = text.find("the first one").expect("the entry is there");
    let second = text.find("the second one").expect("the entry is there");
    let third = text.find("the third one").expect("the entry is there");
    assert!(first < second && second < third, "{text}");
}

#[test]
fn a_version_file_that_cannot_be_written_leaves_the_tree_alone() {
    let repo = Repo::new(
        r#"[{ "release": { "changelog": "docs/CHANGELOG.md",
             "version": { "path": "Cargo.toml", "format": "toml", "section": "package", "key": "version" } } }]"#,
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\n",
    );
    repo.write("docs/CHANGELOG.md", "# Changelog\n");
    repo.commit("the files");
    repo.git(&["tag", "v0.1.0"]);
    repo.commit("feat: a feature");

    // The directory of the changelog cannot be written: the temporary file
    // of the write cannot be created.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            repo.path().join("docs"),
            std::fs::Permissions::from_mode(0o555),
        )
        .expect("the directory is made read-only");
    }

    let output = repo.run(&["release"]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The permissions go back: the temporary directory has to be
        // removable.
        std::fs::set_permissions(
            repo.path().join("docs"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("the permissions go back");
    }

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    // The version file goes back: a half-written release is a state nobody
    // asked for.
    assert!(
        repo.read("Cargo.toml").contains("version = \"0.1.0\""),
        "the version file was left changed"
    );
    assert!(
        repo.read("docs/CHANGELOG.md") == "# Changelog\n",
        "the changelog was written"
    );
}

#[test]
fn a_release_from_another_tag() {
    let repo = released_repo();
    repo.commit("feat: a feature");

    let output = repo.run(&["release", "--dry-run", "--from", "v0.1.0"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("v0.2.0"), "{}", stdout(&output));
}

#[test]
fn a_tag_that_does_not_exist_is_an_operational_error() {
    let repo = released_repo();
    repo.commit("feat: a feature");

    let output = repo.run(&["release", "--dry-run", "--from", "v9.9.9"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("the commits since v9.9.9 cannot be read"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_tag_that_is_not_a_release_tag_is_refused() {
    let repo = released_repo();
    repo.commit("feat: a feature");

    // The moving major tag of the action is not a release.
    let output = repo.run(&["release", "--dry-run", "--from", "v1"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("is not a release tag"),
        "{}",
        stderr(&output)
    );

    // A pre-release is not a release either.
    let output = repo.run(&["release", "--dry-run", "--from", "v0.2.0-rc.1"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("is not a release tag"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_first_release_is_the_version_of_the_file() {
    // A repository without a release tag: the version in the file is the
    // version of the first release, and the step does not apply to it.
    let repo = Repo::new(&config(TOML_VERSION));
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\n",
    );
    repo.commit("the manifest");
    repo.commit("feat: a feature");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        repo.read("Cargo.toml").contains("version = \"0.1.0\""),
        "the version moved without a release before it"
    );
    assert!(
        repo.read("CHANGELOG.md").contains("## [0.1.0]"),
        "{}",
        repo.read("CHANGELOG.md")
    );
}

#[test]
fn a_version_file_that_is_not_there_is_an_operational_error() {
    let repo = Repo::new(&config(TOML_VERSION));
    repo.commit("a commit");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("the version file Cargo.toml cannot be read"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_version_file_without_a_version_is_an_operational_error() {
    let repo = Repo::new(&config(TOML_VERSION));
    repo.write("Cargo.toml", "[package]\nname = \"ratchet\"\n");
    repo.commit("the manifest");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("does not carry the version"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_version_that_is_not_a_version_is_an_operational_error() {
    let repo = Repo::new(&config(TOML_VERSION));
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"abc\"\n",
    );
    repo.commit("the manifest");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("is not a version"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_step_is_configurable() {
    let repo = Repo::new(
        r#"[{ "release": { "changelog": "CHANGELOG.md",
             "steps": { "feat": "patch", "breaking": "minor" },
             "version": { "path": "Cargo.toml", "format": "toml", "section": "package", "key": "version" } } }]"#,
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\n",
    );
    repo.commit("the manifest");
    repo.git(&["tag", "v0.1.0"]);
    repo.commit("feat: a feature");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    // The feature moves the patch, not the minor.
    assert!(
        repo.read("Cargo.toml").contains("version = \"0.1.1\""),
        "{}",
        repo.read("Cargo.toml")
    );
}

#[test]
fn a_step_that_is_not_a_step_is_a_configuration_error() {
    let repo = Repo::new(
        r#"[{ "release": { "changelog": "CHANGELOG.md", "steps": { "feat": "huge" },
             "version": { "path": "Cargo.toml", "format": "toml", "section": "package", "key": "version" } } }]"#,
    );
    repo.commit("a commit");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("is not a step"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_changelog_may_start_at_the_version_of_the_release() {
    let repo = Repo::new(
        r#"[{ "release": { "changelog": "CHANGELOG.md", "changelogFrom": "0.2.0",
             "version": { "path": "Cargo.toml", "format": "toml", "section": "package", "key": "version" } } }]"#,
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\n",
    );
    repo.commit("the manifest");
    repo.git(&["tag", "v0.1.0"]);
    repo.commit("feat: a feature");

    let output = repo.run(&["release"]);

    // The version of the release is the version the tool manages.
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        repo.read("CHANGELOG.md").contains("## [0.2.0]"),
        "{}",
        repo.read("CHANGELOG.md")
    );
}

#[test]
fn a_commit_that_is_not_a_conventional_commit_is_not_announced() {
    let repo = released_repo();
    repo.commit("Merge branch 'main'");
    repo.commit("A sentence without a type");

    let output = repo.run(&["release", "--dry-run"]);

    // A commit without a type changes no version: there is nothing to
    // release, and nothing is announced.
    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(!text.contains("### Added"), "{text}");
    assert!(!text.contains("### Fixed"), "{text}");
}

#[test]
fn the_word_security_in_a_subject_is_not_a_security_finding() {
    let repo = released_repo();
    repo.commit("feat(todo): log the release, security, pre-release and CI extensions");

    let output = repo.run(&["release", "--dry-run"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(!text.contains("### Security"), "{text}");
}

#[test]
fn a_commit_with_the_scope_security_is_a_security_finding() {
    let repo = released_repo();
    repo.commit("fix(security): the token was not rotated");

    let output = repo.run(&["release", "--dry-run"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("### Security"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_json_manifest_with_a_nested_key_is_not_the_version_file() {
    let repo = Repo::new(&config(JSON_VERSION));
    repo.write(
        "package.json",
        "{\n  \"name\": \"ratchet\",\n  \"config\": {\n    \"version\": \"0.1.0\"\n  }\n}\n",
    );
    repo.commit("the manifest");
    repo.git(&["tag", "v0.1.0"]);
    repo.commit("feat: a feature");

    let output = repo.run(&["release"]);

    // The version of the project is not in the root: the tool does not
    // write into a nested member.
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("does not carry the version"),
        "{}",
        stderr(&output)
    );
    assert!(
        repo.read("package.json").contains("\"version\": \"0.1.0\""),
        "the nested member was written"
    );
}

#[test]
fn a_version_file_outside_the_repository_root_is_refused() {
    let repo = Repo::new(
        r#"[{ "release": { "changelog": "CHANGELOG.md",
             "version": { "path": "../outside.toml", "format": "toml", "section": "package", "key": "version" } } }]"#,
    );
    repo.commit("a commit");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("inside the repository root"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_dry_run_shows_the_block() {
    let repo = released_repo();
    repo.commit("feat: a feature");

    let output = repo.run(&["release", "--dry-run"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    // The block is the artifact the command produces: it is part of the
    // output, not only of the JSON document.
    assert!(text.contains("## [0.2.0] - "), "{text}");
    assert!(text.contains("### Added"), "{text}");
    assert!(text.contains("- a feature"), "{text}");
    assert!(text.contains("the run was a dry one"), "{text}");
}

#[test]
fn the_command_is_in_the_help() {
    let repo = Repo::new(&config(TOML_VERSION));

    let output = repo.run(&["--help"]);

    let text = stdout(&output);
    assert!(text.contains("release"), "{text}");
    assert!(text.contains("Compute the next version"), "{text}");
}

#[test]
fn a_pre_release_tag_is_not_a_release() {
    let repo = released_repo();
    repo.commit("feat: a feature");
    repo.git(&["tag", "v0.2.0-rc.1"]);

    let output = repo.run(&["release", "--dry-run"]);

    // The pre-release is not a release: the search continues with the tag
    // below it.
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("v0.2.0"), "{text}");
    assert!(text.contains("since"), "{text}");
    assert!(
        text.contains("v0.1.0"),
        "the pre-release was taken for the release: {text}"
    );
}

#[test]
fn the_tag_prefix_is_configurable() {
    let repo = Repo::new(
        r#"[{ "release": { "changelog": "CHANGELOG.md", "tagPrefix": "",
             "version": { "path": "Cargo.toml", "format": "toml", "section": "package", "key": "version" } } }]"#,
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\n",
    );
    repo.commit("the manifest");
    repo.git(&["tag", "0.1.0"]);
    repo.commit("feat: a feature");

    let output = repo.run(&["release", "--dry-run"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    // The tag carries no prefix.
    assert!(text.contains("0.2.0"), "{text}");
    assert!(!text.contains("v0.2.0"), "{text}");
}

#[test]
fn the_from_option_names_the_tag_of_the_release() {
    let repo = released_repo();
    repo.commit("feat: a feature");
    repo.git(&["tag", "v0.2.0"]);
    repo.commit("fix: another thing");

    // The last release tag is v0.2.0; the run is computed from v0.1.0.
    let output = repo.run(&["release", "--dry-run", "--from", "v0.1.0"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    // The base is the named tag — and the version follows from it.
    assert!(text.contains("since"), "{text}");
    assert!(text.contains("v0.1.0"), "{text}");
    assert!(text.contains("v0.2.0"), "{text}");
}

#[test]
fn the_committer_date_is_the_date_of_the_release() {
    let repo = released_repo();
    // The author wrote it earlier; the change landed today.
    Command::new("git")
        .args(["commit", "-q", "--allow-empty", "-m", "feat: a feature"])
        .current_dir(repo.path())
        .env("GIT_AUTHOR_NAME", "ratchet test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "ratchet test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-02T10:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2026-03-04T10:00:00+00:00")
        .status()
        .expect("git runs");

    let output = repo.run(&["release", "--dry-run"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("## [0.2.0] - 2026-03-04"), "{text}");
    assert!(!text.contains("2026-01-02"), "{text}");
}

#[test]
fn a_symlinked_changelog_outside_the_root_is_refused() {
    let repo = Repo::new(
        r#"[{ "release": { "changelog": "CHANGELOG.md",
             "version": { "path": "Cargo.toml", "format": "toml", "section": "package", "key": "version" } } }]"#,
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\n",
    );
    repo.commit("the manifest");
    repo.git(&["tag", "v0.1.0"]);
    repo.commit("feat: a feature");

    // The changelog is a link to a file outside the repository.
    #[cfg(unix)]
    {
        let outside = repo
            .path()
            .parent()
            .expect("a parent directory")
            .join("outside-changelog.md");
        std::fs::write(&outside, "# Changelog\n").expect("the file is written");
        std::os::unix::fs::symlink(&outside, repo.path().join("CHANGELOG.md"))
            .expect("the link is made");
    }

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    // The refusal names the reason: the path leaves the root.  It does not
    // blame a link — the reason is the same for every path that leaves the
    // root.
    assert!(
        stderr(&output).contains("must lie inside the repository root"),
        "{}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("symbolic link"),
        "{}",
        stderr(&output)
    );
}

#[cfg(unix)]
#[test]
fn a_changelog_behind_a_chain_of_links_is_refused() {
    let repo = Repo::new(
        r#"[{ "release": { "changelog": "CHANGELOG.md",
             "version": { "path": "Cargo.toml", "format": "toml", "section": "package", "key": "version" } } }]"#,
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\n",
    );
    repo.commit("the manifest");
    repo.git(&["tag", "v0.1.0"]);
    repo.commit("feat: a feature");

    // The changelog is a chain: `CHANGELOG.md` → `real/CHANGELOG.md` → a file
    // outside the repository that is not there.
    let outside = repo
        .path()
        .parent()
        .expect("a parent directory")
        .join("outside-changelog-chain.md");
    // The name is fixed, so a run that wrote there — a mutation, not the
    // tool — would leave a file behind and the next run would find a link
    // that is not dangling any more.  The file is removed first.
    let _ = std::fs::remove_file(&outside);
    std::fs::create_dir_all(repo.path().join("real")).expect("the directory is created");
    std::os::unix::fs::symlink(&outside, repo.path().join("real/CHANGELOG.md"))
        .expect("the second link is made");
    std::os::unix::fs::symlink(
        repo.path().join("real/CHANGELOG.md"),
        repo.path().join("CHANGELOG.md"),
    )
    .expect("the first link is made");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("must lie inside the repository root"),
        "{}",
        stderr(&output)
    );
    // And nothing was written outside the repository.
    assert!(
        !outside.exists(),
        "the changelog was written outside the repository root"
    );
}

#[test]
fn the_run_writes_exactly_two_files() {
    let repo = released_repo();
    repo.commit("feat: a feature");

    let output = repo.run(&["release"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    // The version file and the changelog — and nothing else.
    let status = repo.status();
    assert!(
        status.contains("Cargo.toml") && status.contains("CHANGELOG.md"),
        "{status}"
    );
    assert_eq!(status.lines().count(), 2, "{status}");
}

#[test]
fn a_breaking_change_moves_the_major_version() {
    let repo = released_repo();
    repo.commit("feat(api)!: the config changed");

    let output = repo.run(&["release", "--dry-run"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("v1.0.0"), "{text}");
    assert!(text.contains("**Breaking**"), "{text}");
}

#[test]
fn a_breaking_footer_moves_the_major_version() {
    let repo = released_repo();
    Command::new("git")
        .args([
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "feat: a new shape\n\nBREAKING CHANGE: the config changed",
        ])
        .current_dir(repo.path())
        .env("GIT_AUTHOR_NAME", "ratchet test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "ratchet test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .status()
        .expect("git runs");

    let output = repo.run(&["release", "--dry-run"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("v1.0.0"), "{}", stdout(&output));
}

#[test]
fn a_refactor_is_announced_but_moves_no_version() {
    let repo = released_repo();
    repo.commit("refactor: the internals");

    let output = repo.run(&["release", "--dry-run"]);

    // A refactor changes no version: there is nothing to release.
    assert_eq!(code(&output), 1, "{}", stderr(&output));
}

#[test]
fn a_compact_json_manifest_carries_no_version() {
    let repo = Repo::new(&config(JSON_VERSION));
    repo.write(
        "package.json",
        "{\"name\": \"ratchet\", \"version\": \"0.1.0\"}\n",
    );
    repo.commit("the manifest");
    repo.git(&["tag", "v0.1.0"]);
    repo.commit("feat: a feature");

    let output = repo.run(&["release"]);

    // The document has no lines to write into.
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("does not carry the version"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_json_document_ends_with_one_newline() {
    let repo = released_repo();
    repo.commit("feat: a feature");

    let output = repo.run(&["release", "--dry-run", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.ends_with("}\n"), "{:?}", text.chars().last());
    assert!(!text.ends_with("}\n\n"), "the document has two newlines");
}
