//! End to end tests of the findings registry: a temporary git repository, the
//! real binary.
//!
//! The registry is a **counter source** (SPEC 13): the count comes from the
//! file, the baseline from the baseline file, the direction from the rule —
//! the mechanic of every other counter, unchanged.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

/// The command that runs the binary.
///
/// The update check needs the network and the tests do not: the variable
/// keeps the tests hermetic.
fn binary() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ratchet"));
    command.env("RATCHET_NO_UPDATE_CHECK", "1");
    command
}

/// A temporary git repository with a config and a baseline.
struct Repo {
    dir: TempDir,
}

impl Repo {
    /// Creates a repository with a config, both committed.
    fn new(config: &str) -> Self {
        let repo = Self::plain(config);
        repo.git(&["init", "-q", "."]);
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-q", "-m", "init"]);
        repo
    }

    /// Creates a directory with a config, but no repository.
    fn plain(config: &str) -> Self {
        let dir = TempDir::new().expect("a temporary directory");
        let repo = Self { dir };
        repo.write("ratchet.config.jsonc", config);
        repo
    }

    /// The root of the repository.
    fn path(&self) -> &Path {
        self.dir.path()
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

    /// Commits everything that is in the tree.
    fn commit_all(&self) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", "the files"]);
    }

    /// Runs the binary in the repository.
    fn run(&self, arguments: &[&str]) -> Output {
        binary()
            .args(arguments)
            .current_dir(self.path())
            .output()
            .expect("the binary runs")
    }
}

/// The stdout of a run.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The stderr of a run.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Everything a run said.
fn said(output: &Output) -> String {
    format!("{}{}", stdout(output), stderr(output))
}

/// The code of a run.
fn code(output: &Output) -> i32 {
    output.status.code().expect("the process exited normally")
}

/// A directory outside the repository root.
///
/// The directory is removed again when it goes out of scope: a temporary is
/// removed, and a test that fails must not leave anything behind.  The name
/// carries the process id: a run that lets the engine write into it — a
/// defect, a mutation — must not make the next run fail.
#[cfg(unix)]
struct Outside {
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl Outside {
    /// Creates the directory.
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("ratchet-outside-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the directory is created");
        Self { path }
    }

    /// The path of the directory.
    fn path(&self) -> &Path {
        &self.path
    }

    /// The path of a file in the directory.
    fn file(&self, name: &str) -> std::path::PathBuf {
        self.path.join(name)
    }
}

#[cfg(unix)]
impl Drop for Outside {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A chain of two links: `name` → `middle` → `target`.
///
/// The chain is what carries a path out of the root: `canonicalize` follows a
/// chain whose final target **exists**, but a chain that ends at a missing
/// file has to be followed by hand.
#[cfg(unix)]
fn link_chain(name: &Path, middle: &Path, target: &Path) {
    let directory = middle.parent().expect("a parent directory");
    std::fs::create_dir_all(directory).expect("the directory is created");
    std::os::unix::fs::symlink(target, middle).expect("the second link is made");
    std::os::unix::fs::symlink(middle, name).expect("the first link is made");
}

/// A registry with one finding of every level and every status.
///
/// The counting findings are `SEC-01` (critical, open) and `SEC-02` (high,
/// in-progress); the rest are decisions or resolved.
const MIXED: &str = r#"{
  "version": 1,
  "findings": [
    { "id": "SEC-01", "title": "org admins reassign foreign users", "severity": "critical",
      "status": "open", "owner": "alice", "ticket": "D-12" },
    { "id": "SEC-02", "title": "a high one", "severity": "high", "status": "in-progress",
      "owner": "alice", "ticket": "D-13" },
    { "id": "SEC-03", "title": "a medium one", "severity": "medium", "status": "open",
      "owner": "alice", "ticket": "D-14" },
    { "id": "SEC-04", "title": "an accepted risk", "severity": "low", "status": "accepted",
      "owner": "alice", "ticket": "D-15" },
    { "id": "SEC-05", "title": "by design", "severity": "info", "status": "wontfix",
      "owner": "alice", "ticket": "D-16" },
    { "id": "SEC-06", "title": "a closed one", "severity": "critical", "status": "fixed",
      "status_since": "2026-10-19", "regression_test": "BoundaryTest" }
  ]
}
"#;

/// A config with one registry rule; the count object is the parameter.
fn config(count: &str) -> String {
    format!(
        r#"[
      {{
        "name": "test",
        "rules": {{
          "open-findings": {{
            "class": "unresolved findings",
            "incident": "prevention: the findings of the project are counted in one registry",
            "unit": "open findings",
            "direction": "ceiling",
            "count": {count}
          }}
        }}
      }}
    ]"#
    )
}

/// A config with the registry rule of the specification.
fn mixed_config() -> String {
    config(
        r#"{ "registry": "ratchet.findings.json", "severities": ["critical", "high"],
             "statuses": ["open", "in-progress"] }"#,
    )
}

/// A baseline with one record.
fn baseline(rule: &str, value: i64) -> String {
    format!(
        "{{\n  \"version\": 1,\n  \"rules\": {{\n    \"{rule}\": {{ \"value\": {value} }}\n  }}\n}}\n"
    )
}

/// A repository with the mixed registry and a rule that counts it.
fn mixed_repo(baseline_value: i64) -> Repo {
    let repo = Repo::new(&mixed_config());
    repo.write("ratchet.findings.json", MIXED);
    repo.write(
        "ratchet.baseline.json",
        &baseline("open-findings", baseline_value),
    );
    repo.commit_all();
    repo
}

/// The count of a rule in the JSON report.
fn count_of(output: &Output) -> i64 {
    let report: Value = serde_json::from_str(&stdout(output)).expect("the stdout is JSON");
    report["rules"][0]["count"]
        .as_i64()
        .expect("the rule has a count")
}

/// The detail of a rule in the JSON report.
fn detail_of(output: &Output) -> String {
    let report: Value = serde_json::from_str(&stdout(output)).expect("the stdout is JSON");
    report["rules"][0]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn the_count_is_the_number_of_the_findings_that_count() {
    // Two findings count: SEC-01 (critical, open) and SEC-02 (high,
    // in-progress).
    let repo = mixed_repo(2);

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 2);
}

#[test]
fn the_severities_of_the_rule_narrow_the_count() {
    // Only the critical ones count: SEC-01 counts, SEC-02 (high) does not.
    let repo = Repo::new(&config(
        r#"{ "registry": "ratchet.findings.json", "severities": ["critical"] }"#,
    ));
    repo.write("ratchet.findings.json", MIXED);
    repo.write("ratchet.baseline.json", &baseline("open-findings", 1));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 1);
}

#[test]
fn without_the_lists_every_level_and_every_counting_status_counts() {
    // SEC-01, SEC-02 and SEC-03 are open or in progress.
    let repo = Repo::new(&config(r#"{ "registry": "ratchet.findings.json" }"#));
    repo.write("ratchet.findings.json", MIXED);
    repo.write("ratchet.baseline.json", &baseline("open-findings", 3));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 3);
}

#[test]
fn a_decision_does_not_count() {
    // Only the open findings count: SEC-01 and SEC-03.
    let repo = Repo::new(&config(
        r#"{ "registry": "ratchet.findings.json", "statuses": ["open"] }"#,
    ));
    repo.write("ratchet.findings.json", MIXED);
    repo.write("ratchet.baseline.json", &baseline("open-findings", 2));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 2);
}

#[test]
fn a_fixed_finding_does_not_count() {
    let repo = Repo::new(&config(r#"{ "registry": "ratchet.findings.json" }"#));
    repo.write(
        "ratchet.findings.json",
        r#"{ "version": 1, "findings": [
             { "id": "SEC-01", "title": "a closed one", "severity": "critical",
               "status": "fixed" } ] }"#,
    );
    repo.write("ratchet.baseline.json", &baseline("open-findings", 0));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 0);
}

#[test]
fn a_ceiling_with_a_baseline_of_zero_is_the_boolean_gate() {
    let repo = Repo::new(&config(
        r#"{ "registry": "ratchet.findings.json", "severities": ["critical", "high"],
             "statuses": ["open", "in-progress"] }"#,
    ));
    repo.write("ratchet.findings.json", MIXED);
    repo.write("ratchet.baseline.json", &baseline("open-findings", 0));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 1, "{}", said(&output));
    let text = stdout(&output);
    assert!(text.contains("FAIL"), "{text}");
    assert!(text.contains("2 > 0"), "{text}");
}

#[test]
fn the_count_command_prints_the_count_of_the_registry() {
    let repo = mixed_repo(2);

    let output = repo.run(&["count"]);

    assert_eq!(code(&output), 0, "{}", said(&output));
    let text = stdout(&output);
    assert!(text.contains("open-findings"), "{text}");
    assert!(text.contains("2"), "{text}");
    assert!(text.contains("open findings"), "{text}");
}

#[test]
fn the_baseline_is_paid_down_by_the_registry() {
    let repo = mixed_repo(5);

    let output = repo.run(&["baseline", "update"]);

    assert_eq!(code(&output), 0, "{}", said(&output));
    let baseline: Value =
        serde_json::from_str(&repo.read("ratchet.baseline.json")).expect("the baseline is JSON");
    assert_eq!(baseline["rules"]["open-findings"]["value"], 2);
}

#[test]
fn two_runs_produce_the_same_bytes() {
    let repo = mixed_repo(2);

    let first = repo.run(&["check"]);
    let second = repo.run(&["check"]);

    assert_eq!(code(&first), 0, "{}", said(&first));
    assert_eq!(stdout(&first), stdout(&second));
}

#[test]
fn the_registry_is_not_mutated() {
    let repo = mixed_repo(2);
    let before = repo.read("ratchet.findings.json");

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(repo.read("ratchet.findings.json"), before);
    assert_eq!(repo.status(), "", "the tree is clean");
}

/// A repository with a broken registry and the rule that counts it.
fn broken_repo(registry: &str) -> Repo {
    let repo = Repo::new(&config(r#"{ "registry": "ratchet.findings.json" }"#));
    repo.write("ratchet.findings.json", registry);
    repo.write("ratchet.baseline.json", &baseline("open-findings", 0));
    repo
}

/// Runs a run with a broken registry and returns the code and the message.
///
/// The run is the human readable one: the report carries the message of the
/// counter error as it is — without the escaping of the JSON format.
fn broken(registry: &str) -> (i32, String) {
    let repo = broken_repo(registry);
    let output = repo.run(&["check"]);
    (code(&output), said(&output))
}

#[test]
fn a_missing_registry_is_a_configuration_error() {
    let repo = Repo::new(&config(r#"{ "registry": "ratchet.findings.json" }"#));
    repo.write("ratchet.baseline.json", &baseline("open-findings", 0));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("cannot read"), "{message}");
    assert!(message.contains("ratchet.findings.json"), "{message}");
}

#[test]
fn an_invalid_json_registry_is_a_configuration_error() {
    let (code, message) = broken("not json");

    assert_eq!(code, 2, "{message}");
    assert!(message.contains("is not valid JSON"), "{message}");
}

#[test]
fn a_wrong_version_is_a_configuration_error() {
    let (code, message) = broken(r#"{ "version": 2, "findings": [] }"#);

    assert_eq!(code, 2, "{message}");
    assert!(message.contains("the version is 2"), "{message}");
}

#[test]
fn a_missing_field_is_a_configuration_error() {
    let (code, message) = broken(
        r#"{ "version": 1, "findings": [ { "id": "SEC-01", "severity": "critical",
             "status": "open", "owner": "alice", "ticket": "D-1" } ] }"#,
    );

    assert_eq!(code, 2, "{message}");
    assert!(message.contains("SEC-01"), "{message}");
    assert!(message.contains("has no \"title\""), "{message}");
}

#[test]
fn a_missing_id_is_a_configuration_error() {
    let (code, message) = broken(
        r#"{ "version": 1, "findings": [ { "title": "a", "severity": "critical",
             "status": "open", "owner": "alice", "ticket": "D-1" } ] }"#,
    );

    assert_eq!(code, 2, "{message}");
    assert!(message.contains("findings[0]"), "{message}");
    assert!(message.contains("has no \"id\""), "{message}");
}

#[test]
fn a_duplicate_id_is_a_configuration_error() {
    let (code, message) = broken(
        r#"{ "version": 1, "findings": [
             { "id": "SEC-01", "title": "a", "severity": "critical", "status": "fixed" },
             { "id": "SEC-01", "title": "b", "severity": "critical", "status": "fixed" } ] }"#,
    );

    assert_eq!(code, 2, "{message}");
    assert!(message.contains("SEC-01"), "{message}");
    assert!(message.contains("used twice"), "{message}");
}

#[test]
fn an_unknown_severity_is_a_configuration_error() {
    let (code, message) = broken(
        r#"{ "version": 1, "findings": [ { "id": "SEC-01", "title": "a", "severity": "blocker",
             "status": "fixed" } ] }"#,
    );

    assert_eq!(code, 2, "{message}");
    assert!(message.contains("SEC-01"), "{message}");
    assert!(
        message.contains("unknown severity \"blocker\""),
        "{message}"
    );
}

#[test]
fn an_unknown_status_is_a_configuration_error() {
    let (code, message) = broken(
        r#"{ "version": 1, "findings": [ { "id": "SEC-01", "title": "a", "severity": "critical",
             "status": "doing" } ] }"#,
    );

    assert_eq!(code, 2, "{message}");
    assert!(message.contains("SEC-01"), "{message}");
    assert!(message.contains("unknown status \"doing\""), "{message}");
}

#[test]
fn a_missing_owner_is_a_configuration_error() {
    let (code, message) = broken(
        r#"{ "version": 1, "findings": [ { "id": "SEC-01", "title": "a", "severity": "critical",
             "status": "open", "ticket": "D-1" } ] }"#,
    );

    assert_eq!(code, 2, "{message}");
    assert!(message.contains("SEC-01"), "{message}");
    assert!(message.contains("has no \"owner\""), "{message}");
}

#[test]
fn a_missing_ticket_is_a_configuration_error() {
    let (code, message) = broken(
        r#"{ "version": 1, "findings": [ { "id": "SEC-01", "title": "a", "severity": "critical",
             "status": "in-progress", "owner": "alice" } ] }"#,
    );

    assert_eq!(code, 2, "{message}");
    assert!(message.contains("SEC-01"), "{message}");
    assert!(message.contains("has no \"ticket\""), "{message}");
}

#[test]
fn a_registry_outside_the_repository_root_is_a_configuration_error() {
    let repo = Repo::new(&config(r#"{ "registry": "../outside.json" }"#));
    repo.write("ratchet.baseline.json", &baseline("open-findings", 0));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("outside the repository root"), "{message}");
}

#[test]
fn an_absolute_registry_is_a_configuration_error() {
    let repo = Repo::new(&config(r#"{ "registry": "/etc/ratchet.findings.json" }"#));
    repo.write("ratchet.baseline.json", &baseline("open-findings", 0));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    // The path of a project is relative: the message says what is wrong.
    assert!(message.contains("has to be a relative path"), "{message}");
}

#[cfg(unix)]
#[test]
fn a_registry_through_a_linked_directory_outside_the_root_is_a_configuration_error() {
    let repo = Repo::new(&config(r#"{ "registry": "docs/ratchet.findings.json" }"#));
    repo.write("ratchet.baseline.json", &baseline("open-findings", 0));
    // The directory inside the root is a link that points out of it: the
    // registry behind the link is a file the repository does not own.
    let outside = Outside::new("registry");
    std::fs::write(outside.file("ratchet.findings.json"), MIXED).expect("the file is written");
    std::os::unix::fs::symlink(outside.path(), repo.path().join("docs")).expect("the link is made");

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("outside the repository root"), "{message}");
}

#[cfg(unix)]
#[test]
fn a_registry_that_is_a_link_to_a_file_outside_the_root_is_refused() {
    let repo = Repo::new(&config(r#"{ "registry": "ratchet.findings.json" }"#));
    repo.write("ratchet.baseline.json", &baseline("open-findings", 0));
    // The registry itself is a link to a file outside the root: the counter
    // would read a file the repository does not own.
    let outside = Outside::new("registry-target");
    std::fs::write(outside.file("ratchet.findings.json"), MIXED).expect("the file is written");
    std::os::unix::fs::symlink(
        outside.file("ratchet.findings.json"),
        repo.path().join("ratchet.findings.json"),
    )
    .expect("the link is made");

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("outside the repository root"), "{message}");
}

#[cfg(unix)]
#[test]
fn a_registry_through_a_link_and_a_parent_is_refused() {
    // "link/../findings.json" is ambiguous: the operating system reads it as
    // the parent of the link's **target**.
    let repo = Repo::new(&config(
        r#"{ "registry": "link/../ratchet.findings.json" }"#,
    ));
    repo.write("ratchet.baseline.json", &baseline("open-findings", 0));
    let outside = Outside::new("parent-registry");
    std::fs::write(outside.file("ratchet.findings.json"), MIXED).expect("the file is written");
    std::os::unix::fs::symlink(outside.path(), repo.path().join("link")).expect("the link is made");

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("is ambiguous"), "{message}");
}

#[cfg(unix)]
#[test]
fn a_registry_through_a_chain_of_links_outside_the_root_is_refused() {
    let repo = Repo::new(&config(r#"{ "registry": "ratchet.findings.json" }"#));
    repo.write("ratchet.baseline.json", &baseline("open-findings", 0));
    // The registry is a chain: `ratchet.findings.json` → `real/findings.json`
    // → a file outside the root that is not there.
    let outside = Outside::new("registry-chain");
    link_chain(
        &repo.path().join("ratchet.findings.json"),
        &repo.path().join("real/findings.json"),
        &outside.file("shared-findings.json"),
    );

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("outside the repository root"), "{message}");
}

#[test]
fn an_unknown_severity_in_the_rule_is_a_configuration_error() {
    let repo = Repo::new(&config(
        r#"{ "registry": "ratchet.findings.json", "severities": ["blocker"] }"#,
    ));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", said(&output));
    let text = stderr(&output);
    assert!(
        text.contains("rules.open-findings.count.severities[0]"),
        "{text}"
    );
    assert!(text.contains("unknown severity \"blocker\""), "{text}");
}

#[test]
fn an_unknown_status_in_the_rule_is_a_configuration_error() {
    let repo = Repo::new(&config(
        r#"{ "registry": "ratchet.findings.json", "statuses": ["doing"] }"#,
    ));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", said(&output));
    let text = stderr(&output);
    assert!(
        text.contains("rules.open-findings.count.statuses[0]"),
        "{text}"
    );
    assert!(text.contains("unknown status \"doing\""), "{text}");
}

#[test]
fn a_counter_with_two_forms_is_a_configuration_error() {
    let repo = Repo::new(&config(
        r#"{ "registry": "ratchet.findings.json", "command": ["echo", "1"] }"#,
    ));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", said(&output));
    let text = stderr(&output);
    assert!(text.contains("expected exactly one of"), "{text}");
}

#[test]
fn a_registry_rule_with_an_exclude_is_a_configuration_error() {
    let repo = Repo::new(&config(r#"{ "registry": "ratchet.findings.json" }"#));
    repo.write("ratchet.findings.json", MIXED);
    repo.write("ratchet.baseline.json", &baseline("open-findings", 3));
    let config = r#"[
      {
        "rules": {
          "open-findings": {
            "class": "unresolved findings",
            "incident": "prevention",
            "unit": "open findings",
            "direction": "ceiling",
            "count": { "registry": "ratchet.findings.json" },
            "exclude": ["SEC-*"]
          }
        }
      }
    ]"#;
    repo.write("ratchet.config.jsonc", config);

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(
        message.contains("exclude requires a counter that yields items"),
        "{message}"
    );
}
