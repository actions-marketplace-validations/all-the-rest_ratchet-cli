//! End to end tests: a temporary git repository, the real binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

/// A temporary git repository with a config and a baseline.
struct Repo {
    dir: TempDir,
}

impl Repo {
    /// Creates a repository with a config and a baseline, both committed.
    fn new(config: &str, baseline: Option<&str>) -> Self {
        let repo = Self::plain(config, baseline);
        repo.git(&["init", "-q", "."]);
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-q", "-m", "init"]);
        repo
    }

    /// Creates a directory with a config and a baseline, but no repository.
    fn plain(config: &str, baseline: Option<&str>) -> Self {
        let dir = TempDir::new().expect("a temporary directory");
        let repo = Self { dir };
        repo.write("ratchet.config.jsonc", config);
        if let Some(baseline) = baseline {
            repo.write("ratchet.baseline.json", baseline);
        }
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

    /// Runs the binary in the repository.
    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ratchet"))
            .args(arguments)
            .current_dir(self.path())
            .output()
            .expect("the binary runs")
    }

    /// Runs the binary without a reachable git repository.
    fn run_without_git(&self, arguments: &[&str]) -> Output {
        let ceiling = self.path().parent().expect("a parent directory");
        Command::new(env!("CARGO_BIN_EXE_ratchet"))
            .args(arguments)
            .current_dir(self.path())
            .env("GIT_CEILING_DIRECTORIES", ceiling)
            .output()
            .expect("the binary runs")
    }
}

/// True when `node` is on the PATH.
fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// The stdout of a run.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The stderr of a run.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The code of a run.
fn code(output: &Output) -> i32 {
    output.status.code().expect("the process exited normally")
}

/// A config with one ceiling rule that counts three.
fn ceiling_config() -> &'static str {
    r#"[
      {
        "name": "test",
        "rules": {
          "todo-markers": {
            "class": "unfinished work",
            "incident": "a TODO marker was merged without a tracking issue",
            "unit": "markers",
            "direction": "ceiling",
            "count": { "command": ["echo", "3"] }
          }
        }
      }
    ]"#
}

/// A config with one floor rule that counts three.
fn floor_config() -> &'static str {
    r#"[
      {
        "rules": {
          "tests": {
            "class": "coverage",
            "incident": "a test was deleted",
            "unit": "tests",
            "direction": "floor",
            "count": { "command": ["echo", "3"] }
          }
        }
      }
    ]"#
}

/// A config whose counter always fails.
fn counter_error_config() -> &'static str {
    r#"[
      {
        "rules": {
          "a": {
            "class": "layer violation",
            "incident": "the counter crashed",
            "unit": "call sites",
            "direction": "ceiling",
            "tier": "slow",
            "count": { "command": ["false"] }
          }
        }
      }
    ]"#
}

/// A baseline with one record.
fn baseline(rule: &str, value: i64) -> String {
    format!(
        "{{\n  \"version\": 1,\n  \"rules\": {{\n    \"{rule}\": {{ \"value\": {value} }}\n  }}\n}}\n"
    )
}

#[test]
fn a_green_run_exits_zero() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("PASS"), "{text}");
    assert!(text.contains("COHERENT"), "{text}");
}

#[test]
fn a_count_above_the_baseline_fails() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 2)));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("FAIL"), "{text}");
    assert!(text.contains("3 > 2"), "{text}");
}

#[test]
fn a_floor_rule_fails_below_its_baseline() {
    let repo = Repo::new(floor_config(), Some(&baseline("tests", 5)));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("3 < 5"), "{text}");
}

#[test]
fn a_floor_rule_passes_at_its_baseline() {
    let repo = Repo::new(floor_config(), Some(&baseline("tests", 3)));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
}

#[test]
fn a_rule_without_a_baseline_fails() {
    let repo = Repo::new(ceiling_config(), None);

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("missing-baseline"), "{text}");
    assert!(text.contains("no baseline for todo-markers"), "{text}");
}

#[test]
fn a_raised_baseline_fails_against_the_committed_one() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));
    // The working tree raises the baseline, HEAD still has 5.
    repo.write("ratchet.baseline.json", &baseline("todo-markers", 7));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("baseline 5 raised to 7"), "{text}");
}

#[test]
fn a_lowered_floor_baseline_fails_against_the_committed_one() {
    let repo = Repo::new(floor_config(), Some(&baseline("tests", 5)));
    repo.write("ratchet.baseline.json", &baseline("tests", 3));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("baseline 5 lowered to 3"), "{text}");
}

#[test]
fn baseline_update_refuses_the_forbidden_direction() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 2)));

    let output = repo.run(&["baseline", "update"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("refusing: count 3 > baseline 2 — a ceiling may only decrease"),
        "{text}"
    );
    // The refused rule is not modified.
    assert!(repo.read("ratchet.baseline.json").contains("\"value\": 2"));
}

#[test]
fn baseline_update_accepts_the_allowed_direction() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&["baseline", "update", "--reason", "paid down"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let baseline = repo.read("ratchet.baseline.json");
    assert!(baseline.contains("\"value\": 3"), "{baseline}");
    assert!(baseline.contains("paid down"), "{baseline}");
}

#[test]
fn baseline_update_refuses_a_floor_rule_that_fell() {
    let repo = Repo::new(floor_config(), Some(&baseline("tests", 5)));

    let output = repo.run(&["baseline", "update"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("refusing: count 3 < baseline 5 — a floor may only increase"),
        "{text}"
    );
}

#[test]
fn baseline_update_moves_a_floor_rule_up() {
    let repo = Repo::new(floor_config(), Some(&baseline("tests", 3)));

    let output = repo.run(&["baseline", "update"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(repo.read("ratchet.baseline.json").contains("\"value\": 3"));
}

#[test]
fn baseline_init_writes_the_current_count() {
    let repo = Repo::new(ceiling_config(), None);

    let output = repo.run(&["baseline", "init", "--reason", "first measurement"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let baseline = repo.read("ratchet.baseline.json");
    assert!(baseline.contains("\"value\": 3"), "{baseline}");
    assert!(baseline.contains("first measurement"), "{baseline}");
}

#[test]
fn baseline_init_keeps_an_existing_baseline() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&["baseline", "init"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(repo.read("ratchet.baseline.json").contains("\"value\": 5"));
}

#[test]
fn the_json_format_prints_only_json() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    let report: Value = serde_json::from_str(&text).expect("the stdout is a JSON object");
    assert_eq!(report["schema"], 1);
    assert_eq!(report["ok"], true);
    assert_eq!(report["summary"]["rules"], 1);
    assert_eq!(report["summary"]["failed"], 0);
    assert_eq!(report["summary"]["executed"], 1);
    assert_eq!(report["summary"]["generated"], 0);
    assert_eq!(report["summary"]["drifted"], 0);
    assert_eq!(report["rules"][0]["id"], "todo-markers");
    assert_eq!(report["rules"][0]["status"], "ok");
    assert_eq!(report["rules"][0]["count"], 3);
    assert_eq!(report["rules"][0]["baseline"], 5);
    assert_eq!(report["rules"][0]["direction"], "ceiling");
    assert_eq!(report["rules"][0]["unit"], "markers");
    assert_eq!(report["rules"][0]["class"], "unfinished work");
    assert_eq!(report["rules"][0]["tier"], "fast");
    assert_eq!(report["generated"].as_array().expect("an array").len(), 0);
}

#[test]
fn the_json_format_reports_a_failed_gate() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 2)));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["ok"], false);
    assert_eq!(report["summary"]["failed"], 1);
    assert_eq!(report["rules"][0]["status"], "over-baseline");
}

#[test]
fn an_invalid_config_exits_two_and_names_the_path() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "u",
                "direction": "sideways",
                "count": { "command": ["echo", "1"] }
              }
            }
          }
        ]"#,
        None,
    );

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2);
    let text = stderr(&output);
    assert!(text.contains("rules.a.direction"), "{text}");
}

#[test]
fn a_missing_config_exits_two() {
    let repo = Repo::new(ceiling_config(), None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("no config found"));
}

#[test]
fn a_crashed_counter_exits_two() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "u",
                "direction": "ceiling",
                "count": { "command": ["false"] }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 1)),
    );

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["status"], "counter-error");
}

#[test]
fn the_paths_scope_the_run() {
    let config = r#"[
      {
        "rules": {
          "scoped": {
            "class": "layer violation",
            "incident": "a call site was wrapped",
            "unit": "call sites",
            "direction": "ceiling",
            "count": { "command": ["echo", "1"] },
            "files": ["src/**"]
          }
        }
      }
    ]"#;
    let repo = Repo::new(config, Some(&baseline("scoped", 3)));
    repo.write("docs/readme.md", "hello");
    repo.write("src/main.rs", "fn main() {}");

    let output = repo.run(&["check", "docs/readme.md"]);

    // The rule is out of scope, nothing is executed: an empty selection fails.
    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(!stdout(&output).contains("PASS"), "{}", stdout(&output));

    let output = repo.run(&["check", "src/main.rs"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("PASS"), "{}", stdout(&output));
}

#[test]
fn the_count_command_prints_the_current_count() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&["count"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("3"), "{text}");
    assert!(text.contains("markers"), "{text}");
}

#[test]
fn the_default_command_is_check() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&[]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("PASS"));
}

#[test]
fn the_config_is_found_in_a_parent_directory() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));
    let nested = repo.path().join("nested/deeper");
    std::fs::create_dir_all(&nested).expect("the directory is created");

    let output = Command::new(env!("CARGO_BIN_EXE_ratchet"))
        .arg("check")
        .current_dir(&nested)
        .output()
        .expect("the binary runs");

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

#[test]
fn init_writes_a_starter_config_and_never_overwrites() {
    let repo = Repo::new(ceiling_config(), None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");

    let output = repo.run(&["init"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let config = repo.read("ratchet.config.jsonc");
    assert!(config.contains("\"rules\""), "{config}");
    assert!(config.contains("\"direction\": \"ceiling\""), "{config}");

    // A second run does not overwrite the config.
    repo.write("ratchet.config.jsonc", "// mine\n");
    let output = repo.run(&["init"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(repo.read("ratchet.config.jsonc"), "// mine\n");
}

#[test]
fn the_starter_config_is_valid() {
    let repo = Repo::new(ceiling_config(), None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");
    repo.run(&["init"]);

    let output = repo.run(&["check"]);

    // The starter config counts TODO markers; there are none in the repository.
    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("missing-baseline"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_missing_baseline_file_is_an_empty_baseline() {
    let repo = Repo::new(ceiling_config(), None);

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["status"], "missing-baseline");
    assert_eq!(report["rules"][0]["baseline"], Value::Null);
}

#[test]
fn a_file_counter_reads_a_json_document() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": { "file": "audit.json", "path": "coherence.innerHtml" }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 1)),
    );
    repo.write(
        "audit.json",
        r#"{"coherence": {"innerHtml": ["src/a.ts", "src/b.ts"]}}"#,
    );

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["count"], 2);
}

#[test]
fn the_exclude_patterns_remove_items_from_the_count() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": { "file": "audit.json" },
                "exclude": ["src/**"]
              }
            }
          }
        ]"#,
        Some(&baseline("a", 1)),
    );
    repo.write("audit.json", r#"["src/a.ts", "src/b.ts", "docs/c.ts"]"#);

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["count"], 1);
}

#[test]
fn the_quiet_flag_prints_only_failures() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 2)));

    let output = repo.run(&["check", "--quiet"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("FAIL"), "{text}");
    assert!(!text.contains("PASS"), "{text}");
    assert!(!text.contains("ratchet —"), "{text}");
}

#[test]
fn an_unknown_rule_exits_two() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&["check", "--rule", "nope"]);

    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("unknown rule: nope"));
}

#[test]
fn a_rule_filter_restricts_the_run() {
    let config = r#"[
      {
        "rules": {
          "a": {
            "class": "c",
            "incident": "i",
            "unit": "u",
            "direction": "ceiling",
            "count": { "command": ["echo", "1"] }
          },
          "b": {
            "class": "c",
            "incident": "i",
            "unit": "u",
            "direction": "ceiling",
            "count": { "command": ["echo", "2"] }
          }
        }
      }
    ]"#;
    let repo = Repo::new(
        config,
        Some(
            "{\n  \"version\": 1,\n  \"rules\": {\n    \"a\": { \"value\": 1 },\n    \"b\": { \"value\": 1 }\n  }\n}\n",
        ),
    );

    let output = repo.run(&["check", "--rule", "a", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["summary"]["rules"], 1);
    assert_eq!(report["rules"][0]["id"], "a");
    assert_eq!(report["rules"][0]["status"], "ok");
}

// --- the fixes of the verification round -------------------------------------

#[test]
fn without_git_the_committed_comparison_is_skipped_with_a_note() {
    let repo = Repo::plain(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run_without_git(&["check"]);

    // The gate still runs: the count is compared with the baseline.
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stderr(&output);
    assert!(
        text.contains("monotonicity skipped: not a git repository"),
        "{text}"
    );
    assert!(stdout(&output).contains("PASS"), "{}", stdout(&output));
}

#[test]
fn the_arguments_of_a_counter_arrive_verbatim() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));
    // The script writes the arguments it received, NUL separated, and prints
    // how many it got.
    repo.write(
        "argv.sh",
        "#!/bin/sh\nout=\"$1\"\nshift\n: > \"$out\"\nfor arg in \"$@\"; do printf '%s\\0' \"$arg\" >> \"$out\"; done\nprintf '%s' \"$#\"\n",
    );
    let outside = tempfile::tempdir().expect("a temporary directory");
    let output_file = outside.path().join("argv.bin");

    let arguments = vec![
        "a b".to_string(),
        "c\"d".to_string(),
        "*".to_string(),
        ";".to_string(),
        "|".to_string(),
        "$(echo x)".to_string(),
        "line1\nline2".to_string(),
    ];
    let mut command = vec!["sh".to_string(), "argv.sh".to_string()];
    command.push(output_file.to_string_lossy().into_owned());
    command.extend(arguments.iter().cloned());

    let config = serde_json::json!([
        {
            "rules": {
                "argv": {
                    "class": "c",
                    "incident": "i",
                    "unit": "u",
                    "direction": "ceiling",
                    "count": { "command": command }
                }
            }
        }
    ]);
    repo.write("ratchet.config.jsonc", &config.to_string());
    repo.write(
        "ratchet.baseline.json",
        &baseline("argv", arguments.len() as i64),
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "argv"]);

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    // Every argument arrived: none was split, expanded or substituted.
    assert_eq!(report["rules"][0]["count"], arguments.len());
    let received = std::fs::read(&output_file).expect("the file is readable");
    let mut expected = Vec::new();
    for argument in &arguments {
        expected.extend_from_slice(argument.as_bytes());
        expected.push(0);
    }
    assert_eq!(received, expected);
}

#[test]
fn the_engine_does_not_mutate_the_tree() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&["check"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(repo.status(), "", "check changed the working tree");

    let output = repo.run(&["count"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(repo.status(), "", "count changed the working tree");
}

#[test]
fn the_baseline_commands_only_touch_the_baseline_file() {
    let repo = Repo::new(ceiling_config(), None);

    let output = repo.run(&["baseline", "init"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(repo.status(), "?? ratchet.baseline.json\n");

    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "baseline"]);

    let output = repo.run(&["baseline", "update", "--reason", "paid down"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(repo.status(), " M ratchet.baseline.json\n");
}

#[test]
fn a_counter_error_on_count_exits_two() {
    let repo = Repo::new(counter_error_config(), Some(&baseline("a", 1)));

    let output = repo.run(&["count"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stdout(&output).contains("failed"), "{}", stdout(&output));
}

#[test]
fn a_counter_error_on_baseline_init_exits_two() {
    let repo = Repo::new(counter_error_config(), None);

    let output = repo.run(&["baseline", "init"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("the counter exited with status 1"),
        "{}",
        stdout(&output)
    );
    assert!(!repo.path().join("ratchet.baseline.json").exists());
}

#[test]
fn no_config_without_an_explicit_config_exits_two() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&["check", "--no-config"]);

    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("--no-config requires --config"));
}

#[test]
fn the_javascript_config_is_used_end_to_end() {
    if !node_available() {
        eprintln!(
            "*** SKIPPED: node is not on the PATH — this JavaScript test did NOT run. \
             Install node (or run the CI, which installs it) to exercise it. ***"
        );
        return;
    }
    let repo = Repo::plain("", None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ratchet.config.mjs");
    std::fs::copy(fixture, repo.path().join("ratchet.config.mjs")).expect("the fixture is copied");
    repo.write(
        "ratchet.baseline.json",
        "{\n  \"version\": 1,\n  \"rules\": {\n    \"js-rule\": { \"value\": 1 }\n  }\n}\n",
    );

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    // The rule of the fixture proves that the JavaScript config was really
    // evaluated: there is no other config in the repository.
    assert_eq!(report["rules"][0]["id"], "js-rule");
    assert_eq!(report["rules"][0]["status"], "ok");
    assert_eq!(report["rules"][0]["count"], 1);
}

#[test]
fn two_runs_produce_the_same_bytes() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let first = repo.run(&["check", "--format", "json"]);
    let second = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&first), 0, "{}", stderr(&first));
    assert_eq!(stdout(&first), stdout(&second));
    assert_eq!(stderr(&first), stderr(&second));
}

#[test]
fn a_committed_baseline_that_does_not_parse_exits_two() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));
    // The committed baseline is broken, the working tree is fine.
    repo.write("ratchet.baseline.json", "{ this is not a baseline");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "broken"]);
    repo.write("ratchet.baseline.json", &baseline("todo-markers", 5));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    assert!(
        text.contains("the committed baseline is unreadable"),
        "{text}"
    );
    // The gate is not silently disabled.
    assert!(!text.contains("monotonicity skipped"), "{text}");
}

#[test]
fn an_unknown_command_or_path_exits_two() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    for argument in ["drift", "update", "chek"] {
        let output = repo.run(&[argument]);
        assert_eq!(code(&output), 2, "{argument} must not be green");
        let text = stderr(&output);
        assert!(
            text.contains("unknown command or path"),
            "{argument}: {text}"
        );
    }
}

#[test]
fn a_path_that_does_not_exist_exits_two() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&["check", "src/does-not-exist.rs"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("unknown command or path"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn init_does_not_shadow_a_config_of_a_parent_directory() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));
    let nested = repo.path().join("nested/deeper");
    std::fs::create_dir_all(&nested).expect("the directory is created");

    let output = Command::new(env!("CARGO_BIN_EXE_ratchet"))
        .arg("init")
        .current_dir(&nested)
        .output()
        .expect("the binary runs");

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        !nested.join("ratchet.config.jsonc").exists(),
        "init must not write a second config"
    );
    assert!(
        stdout(&output).contains("already exists"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_counter_error_keeps_the_metadata_of_the_rule() {
    let repo = Repo::new(counter_error_config(), Some(&baseline("a", 1)));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["status"], "counter-error");
    assert_eq!(report["rules"][0]["direction"], "ceiling");
    assert_eq!(report["rules"][0]["unit"], "call sites");
    assert_eq!(report["rules"][0]["class"], "layer violation");
    assert_eq!(report["rules"][0]["tier"], "slow");
}

#[test]
fn a_count_without_rules_is_a_successful_run() {
    let repo = Repo::new("[]", None);

    let output = repo.run(&["count", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["summary"]["rules"], 0);
}

#[test]
fn a_generated_artifact_is_counted_and_the_tree_stays_clean() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["sh", "generate.sh"],
                  "output_arg": "--output"
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );
    repo.write(
        "generate.sh",
        "#!/bin/sh\nprintf '{\"coherence\": {\"items\": [\"src/a.ts\", \"src/b.ts\", \"docs/c.ts\"]}}' > \"$2\"\n",
    );
    // The committed artifact is stale: the count must come from the generator.
    repo.write(
        "docs/_audit-raw.json",
        "{\"coherence\": {\"items\": [\"stale.ts\"]}}",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "artifact"]);

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["count"], 3);
    // The tree is unchanged: the generator wrote to a temporary path.
    assert_eq!(repo.status(), "");
    assert!(
        repo.read("docs/_audit-raw.json").contains("stale.ts"),
        "the artifact in the tree is untouched"
    );
}

#[test]
fn a_generated_artifact_is_counted_from_a_temporary_path() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["sh", "generate.sh"],
                  "output_arg": "--output"
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );
    repo.write(
        "generate.sh",
        "#!/bin/sh\nprintf '{\"coherence\": {\"items\": [\"src/a.ts\", \"src/b.ts\", \"docs/c.ts\"]}}' > \"$2\"\n",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "generator"]);

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["count"], 3);
    assert_eq!(repo.status(), "");
}

#[test]
fn a_failing_generator_leaves_the_tree_unchanged() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["sh", "generate.sh"],
                  "output_arg": "--output"
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );
    repo.write(
        "generate.sh",
        "#!/bin/sh\nprintf 'broken' > \"$2\"\nexit 1\n",
    );
    repo.write("docs/_audit-raw.json", "{\"coherence\": {\"items\": []}}");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "generator"]);

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert_eq!(repo.status(), "");
    assert!(repo.read("docs/_audit-raw.json").contains("items"));
}

#[test]
fn the_monotonicity_check_takes_precedence_over_the_count_comparison() {
    let config = r#"[
      {
        "rules": {
          "tests": {
            "class": "coverage",
            "incident": "a test was deleted",
            "unit": "tests",
            "direction": "floor",
            "count": { "command": ["echo", "1"] }
          }
        }
      }
    ]"#;
    let repo = Repo::new(config, Some(&baseline("tests", 5)));
    // The floor baseline is lowered and the count is below it as well.
    repo.write("ratchet.baseline.json", &baseline("tests", 2));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["status"], "baseline-lowered");
    // The count comparison is reported as an additional note.
    let text = stderr(&output);
    assert!(
        text.contains("the count 1 also fails against the baseline 2 (under-floor)"),
        "{text}"
    );
}

// --- the determinism rule ----------------------------------------------------

/// Asserts that a message contains nothing machine-specific.
///
/// An absolute path, a `file://` URL, a temporary directory, a process id or a
/// nanosecond timestamp make the same tree produce different bytes.
fn assert_deterministic(text: &str) {
    for forbidden in [
        "file://",
        "/var/folders",
        "/private/var",
        "/tmp/",
        // The temporary directory of the generator: it carries the process id
        // and a nanosecond timestamp.
        "ratchet-generate-",
    ] {
        assert!(
            !text.contains(forbidden),
            "the output contains \"{forbidden}\": {text}"
        );
    }

    // The machine name must not survive either.
    if let Some(hostname) = hostname() {
        assert!(
            !text.contains(&hostname),
            "the output contains the machine name: {text}"
        );
    }

    // No timestamp: a nanosecond clock has 19 digits, a millisecond clock 13.
    let digits = text
        .split(|character: char| !character.is_ascii_digit())
        .filter(|run| !run.is_empty());
    for run in digits {
        assert!(
            run.len() < 12,
            "the output contains a timestamp-like number ({run}): {text}"
        );
    }
}

/// The name of the machine the test runs on.
fn hostname() -> Option<String> {
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return Some(name);
        }
    }
    let output = Command::new("hostname").output().ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

#[test]
fn a_failing_generator_does_not_leak_the_temporary_path() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["sh", "generate.sh"],
                  "output_arg": "--output"
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );
    // The generator prints the path it was given — the temporary one.
    repo.write(
        "generate.sh",
        "#!/bin/sh\necho \"cannot write to $2\" >&2\nexit 3\n",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "generator"]);

    let first = repo.run(&["check"]);
    let second = repo.run(&["check"]);

    assert_eq!(code(&first), 2, "{}", stderr(&first));
    let text = format!("{}{}", stdout(&first), stderr(&first));
    assert_deterministic(&text);
    // The generator's own message and the file it was supposed to write stay.
    assert!(text.contains("exited with status 3"), "{text}");
    assert!(text.contains("docs/_audit-raw.json"), "{text}");
    // Two runs of the same tree produce the same bytes.
    assert_eq!(stdout(&first), stdout(&second));
    assert_eq!(stderr(&first), stderr(&second));
}

#[test]
fn a_failing_generator_does_not_relay_its_output() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["sh", "generate.sh"],
                  "output_arg": "--output"
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );
    // The generator prints the path it was given — the temporary one.
    repo.write(
        "generate.sh",
        "#!/bin/sh\necho \"cannot write to $2\" >&2\nexit 3\n",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "generator"]);

    let first = repo.run(&["check"]);
    let second = repo.run(&["check"]);

    assert_eq!(code(&first), 2, "{}", stderr(&first));
    let text = format!("{}{}", stdout(&first), stderr(&first));
    // The report carries ratchet's own words, never the output of the
    // generator (SPEC 6.2).
    assert!(!text.contains("cannot write to"), "{text}");
    assert!(!text.contains("ratchet-generate-"), "{text}");
    assert_deterministic(&text);
    // The generator's own message and the file it was supposed to write stay.
    assert!(
        text.contains("the generator for docs/_audit-raw.json"),
        "{text}"
    );
    assert!(text.contains("exited with status 3"), "{text}");
    // Two runs of the same tree produce the same bytes.
    assert_eq!(stdout(&first), stdout(&second));
    assert_eq!(stderr(&first), stderr(&second));
}

#[test]
fn a_broken_javascript_config_does_not_relay_nodes_output() {
    if !node_available() {
        eprintln!(
            "*** SKIPPED: node is not on the PATH — this JavaScript test did NOT run. \
             Install node (or run the CI, which installs it) to exercise it. ***"
        );
        return;
    }
    let repo = Repo::plain("", None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");
    // A syntax error: node prints the file URL of the config.
    repo.write("ratchet.config.mjs", "export default [\n");

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    // The report says that the config could not be evaluated; node's message
    // belongs to the diagnostic channel.
    assert!(text.contains("could not be evaluated"), "{text}");
    assert!(!text.contains("SyntaxError"), "{text}");
    assert!(!text.contains("file://"), "{text}");
    assert_deterministic(&text);
}

#[test]
fn a_non_serialisable_javascript_config_does_not_relay_nodes_output() {
    if !node_available() {
        eprintln!(
            "*** SKIPPED: node is not on the PATH — this JavaScript test did NOT run. \
             Install node (or run the CI, which installs it) to exercise it. ***"
        );
        return;
    }
    let repo = Repo::plain("", None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");
    // A circular structure: node prints the URL of the evaluated script.
    repo.write(
        "ratchet.config.mjs",
        "const x = {};\nx.self = x;\nexport default x;\n",
    );

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("could not be evaluated"), "{text}");
    assert!(!text.contains("TypeError"), "{text}");
    assert!(!text.contains("file://"), "{text}");
    assert_deterministic(&text);
}

#[test]
fn a_scope_of_src_matches_the_directory_itself() {
    let config = r#"[
      {
        "rules": {
          "a": {
            "class": "c",
            "incident": "i",
            "unit": "u",
            "direction": "ceiling",
            "count": { "command": ["echo", "1"] },
            "files": ["src/**"]
          }
        }
      }
    ]"#;
    let repo = Repo::new(config, Some(&baseline("a", 1)));
    repo.write("src/main.rs", "fn main() {}");

    // The README example: `ratchet check src/` selects the rule.
    let output = repo.run(&["check", "src/"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("PASS"), "{}", stdout(&output));

    let output = repo.run(&["check", "src"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("PASS"), "{}", stdout(&output));
}

#[test]
fn a_scope_that_selects_nothing_still_fails() {
    let config = r#"[
      {
        "rules": {
          "a": {
            "class": "c",
            "incident": "i",
            "unit": "u",
            "direction": "ceiling",
            "count": { "command": ["echo", "1"] },
            "files": ["src/**"]
          }
        }
      }
    ]"#;
    let repo = Repo::new(config, Some(&baseline("a", 1)));
    repo.write("docs/readme.md", "hello");

    let output = repo.run(&["check", "docs/readme.md"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(!stdout(&output).contains("PASS"), "{}", stdout(&output));
}

#[cfg(unix)]
#[test]
fn a_generator_that_replaces_the_file_does_not_touch_the_tree() {
    use std::os::unix::fs::PermissionsExt;

    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["sh", "generate.sh"],
                  "output_arg": "--output"
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );
    // The generator writes the fresh bytes to the temporary path it was
    // given — the tree is not its business.
    repo.write(
        "generate.sh",
        "#!/bin/sh\nprintf '{\"coherence\": {\"items\": [\"a.ts\", \"b.ts\", \"c.ts\"]}}' > \"$2\"\nchmod 600 \"$2\"\n",
    );
    repo.write(
        "docs/_audit-raw.json",
        "{\"coherence\": {\"items\": [\"a.ts\"]}}",
    );
    std::fs::set_permissions(
        repo.path().join("docs/_audit-raw.json"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("the permissions are set");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "artifact"]);

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["count"], 3);
    // The engine never writes into the tree: the artifact is untouched.
    assert_eq!(repo.status(), "");
    let mode = std::fs::metadata(repo.path().join("docs/_audit-raw.json"))
        .expect("the file exists")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "the permissions are unchanged");
}

#[test]
fn the_json_report_carries_the_reason_of_a_failure() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 2)));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["status"], "over-baseline");
    assert_eq!(report["rules"][0]["detail"], "count 3 > baseline 2");
}

#[test]
fn the_json_report_carries_the_reason_of_a_counter_error() {
    let repo = Repo::new(counter_error_config(), Some(&baseline("a", 1)));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["status"], "counter-error");
    let detail = report["rules"][0]["detail"]
        .as_str()
        .expect("the counter error has a detail");
    assert!(detail.contains("exited with status"), "{detail}");
}

#[test]
fn a_green_rule_has_no_detail() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&["check", "--format", "json"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).expect("the stdout is JSON");
    assert_eq!(report["rules"][0]["status"], "ok");
    assert!(report["rules"][0].get("detail").is_none());
}

#[test]
fn the_baseline_command_prints_the_path_it_wrote() {
    let repo = Repo::new(ceiling_config(), None);

    let output = repo.run(&["baseline", "init"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("ratchet.baseline.json"), "{text}");
    assert_deterministic(&text);
}

#[test]
fn baseline_init_does_not_count_the_rules_that_have_a_baseline() {
    // The counter of the rule fails, but the rule has a baseline already: the
    // command must not run the counter.
    let config = r#"[
      {
        "rules": {
          "broken": {
            "class": "c",
            "incident": "i",
            "unit": "u",
            "direction": "ceiling",
            "count": { "command": ["false"] }
          }
        }
      }
    ]"#;
    let repo = Repo::new(config, Some(&baseline("broken", 1)));

    let output = repo.run(&["baseline", "init"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("kept baseline 1"),
        "{}",
        stdout(&output)
    );
    assert!(repo.read("ratchet.baseline.json").contains("\"value\": 1"));
}

#[test]
fn baseline_init_still_fails_when_a_needed_counter_fails() {
    let repo = Repo::new(counter_error_config(), None);

    let output = repo.run(&["baseline", "init"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("the counter exited with status 1"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn init_writes_the_config_where_the_discovery_finds_it() {
    let repo = Repo::new(ceiling_config(), None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");
    let nested = repo.path().join("nested");
    std::fs::create_dir_all(&nested).expect("the directory is created");

    let output = Command::new(env!("CARGO_BIN_EXE_ratchet"))
        .arg("init")
        .current_dir(&nested)
        .output()
        .expect("the binary runs");

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    // The config is written into the root of the repository, not into the
    // subdirectory the command ran in.
    assert!(repo.path().join("ratchet.config.jsonc").exists());
    assert!(!nested.join("ratchet.config.jsonc").exists());
}

#[test]
fn a_counter_that_prints_the_working_directory_does_not_relay_it() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "u",
                "direction": "ceiling",
                "count": { "command": ["sh", "count.sh"] }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 1)),
    );
    // The counter prints the absolute path of the repository.
    repo.write(
        "count.sh",
        "#!/bin/sh\necho \"failed to read $PWD/docs/audit.json\" >&2\nexit 1\n",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "counter"]);

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = format!("{}{}", stdout(&output), stderr(&output));
    // The report carries the exit status, not the output of the counter.
    assert!(!text.contains("failed to read"), "{text}");
    assert!(!text.contains("docs/audit.json"), "{text}");
    assert_deterministic(&text);
    assert!(text.contains("the counter exited with status 1"), "{text}");
}

// --- the report and the diagnostic channel ----------------------------------

#[test]
fn the_debug_channel_contains_the_output_of_the_counter() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "u",
                "direction": "ceiling",
                "count": { "command": ["sh", "count.sh"] }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 1)),
    );
    repo.write(
        "count.sh",
        "#!/bin/sh\necho 'the reason the counter failed' >&2\nexit 1\n",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "counter"]);

    let output = repo.run(&["check", "--debug"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    // The diagnostic channel carries the raw output of the subprocess.
    assert!(text.contains("the reason the counter failed"), "{text}");
    // The report itself stays clean.
    assert!(!stdout(&output).contains("the reason the counter failed"));
}

#[test]
fn the_debug_channel_contains_the_output_of_the_generator() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["sh", "generate.sh"],
                  "output_arg": "--output"
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );
    repo.write(
        "generate.sh",
        "#!/bin/sh\necho 'the reason the generator failed' >&2\nexit 3\n",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "generator"]);

    let output = repo.run(&["check", "--debug"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("the reason the generator failed"), "{text}");
    assert!(!stdout(&output).contains("the reason the generator failed"));
}

#[test]
fn the_debug_channel_contains_the_output_of_node() {
    if !node_available() {
        eprintln!(
            "*** SKIPPED: node is not on the PATH — this JavaScript test did NOT run. \
             Install node (or run the CI, which installs it) to exercise it. ***"
        );
        return;
    }
    let repo = Repo::plain("", None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");
    repo.write("ratchet.config.mjs", "export default [\\n");

    let output = repo.run(&["check", "--debug"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    // Node's message belongs to the diagnostic channel.
    assert!(text.contains("SyntaxError"), "{text}");
    // The report says that the config could not be evaluated.
    assert!(text.contains("could not be evaluated"), "{text}");
}

#[test]
fn the_debug_channel_prints_the_resolved_config_for_every_command() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    for arguments in [
        vec!["check", "--debug"],
        vec!["count", "--debug"],
        vec!["baseline", "init", "--debug"],
        vec!["baseline", "update", "--debug"],
    ] {
        let output = repo.run(&arguments);
        let text = stderr(&output);
        assert!(
            text.contains("resolved config"),
            "{arguments:?} did not print the resolved config: {text}"
        );
        assert!(
            text.contains("todo-markers"),
            "{arguments:?} did not print the resolved config: {text}"
        );
    }
}

#[test]
fn the_debug_channel_prints_the_counter_invocations_for_every_command() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    for arguments in [
        vec!["check", "--debug"],
        vec!["count", "--debug"],
        vec!["baseline", "update", "--debug"],
    ] {
        let output = repo.run(&arguments);
        let text = stderr(&output);
        assert!(
            text.contains("counter todo-markers"),
            "{arguments:?} did not print the counter invocation: {text}"
        );
    }

    // `baseline init` only counts the rules that have no baseline yet.
    let fresh = Repo::new(ceiling_config(), None);
    let output = fresh.run(&["baseline", "init", "--debug"]);
    let text = stderr(&output);
    assert!(
        text.contains("counter todo-markers"),
        "baseline init did not print the counter invocation: {text}"
    );
}

#[test]
fn a_generate_without_an_output_arg_is_a_configuration_error() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["sh", "generate.sh"]
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );
    repo.write("generate.sh", "#!/bin/sh\nprintf 'x' > \"$2\"\n");
    repo.write("docs/_audit-raw.json", "{\"coherence\": {\"items\": []}}");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "generator"]);

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("rules.a.count.output_arg"), "{text}");
    assert!(text.contains("is required by"), "{text}");
    // The tree is untouched.
    assert_eq!(repo.status(), "");
}

#[test]
fn a_baseline_outside_the_repository_root_is_a_configuration_error() {
    let repo = Repo::new(
        r#"[
          {
            "baseline": "../outside.json",
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "u",
                "direction": "ceiling",
                "count": { "command": ["echo", "1"] }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 1)),
    );

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    assert!(
        text.contains("baseline must be inside the repository root"),
        "{text}"
    );
    // The gate is not silently disabled.
    assert!(!text.contains("monotonicity skipped"), "{text}");
}

#[test]
fn an_absolute_baseline_outside_the_repository_root_is_a_configuration_error() {
    let repo = Repo::new(
        r#"[
          {
            "baseline": "/etc/ratchet-baseline.json",
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "u",
                "direction": "ceiling",
                "count": { "command": ["echo", "1"] }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 1)),
    );

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    assert!(
        text.contains("baseline must be inside the repository root"),
        "{text}"
    );
    assert!(!text.contains("monotonicity skipped"), "{text}");
}

#[test]
fn a_counter_that_prints_a_timestamp_and_the_machine_name_does_not_relay_it() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "u",
                "direction": "ceiling",
                "count": { "command": ["sh", "count.sh"] }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 1)),
    );
    repo.write(
        "count.sh",
        "#!/bin/sh\necho \"run on $(hostname) at $(date +%s)\" >&2\nexit 1\n",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "counter"]);

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = format!("{}{}", stdout(&output), stderr(&output));
    // Neither the machine name nor the timestamp reaches the report.
    assert!(!text.contains("run on"), "{text}");
    assert_deterministic(&text);
    assert!(text.contains("the counter exited with status 1"), "{text}");
}

// --- the fourth round of findings -------------------------------------------

/// The entries of a directory.
fn entries(path: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(path)
        .expect("the directory is readable")
        .map(|entry| {
            entry
                .expect("the entry is readable")
                .path()
                .display()
                .to_string()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn a_failing_generator_leaves_no_temporary_directory_behind() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["sh", "generate.sh"],
                  "output_arg": "--output"
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );
    repo.write("generate.sh", "#!/bin/sh\necho 'the reason' >&2\nexit 3\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "generator"]);

    // The engine writes its temporary output into TMPDIR.
    let temporary = tempfile::tempdir().expect("a temporary directory");
    let output = Command::new(env!("CARGO_BIN_EXE_ratchet"))
        .arg("check")
        .current_dir(repo.path())
        .env("TMPDIR", temporary.path())
        .env("TMP", temporary.path())
        .env("TEMP", temporary.path())
        .output()
        .expect("the binary runs");

    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        entries(temporary.path()).is_empty(),
        "the temporary directory was left behind: {:?}",
        entries(temporary.path())
    );
}

#[test]
fn a_generator_that_writes_nothing_leaves_no_temporary_directory_behind() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["sh", "generate.sh"],
                  "output_arg": "--output"
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );
    // The generator succeeds but writes nothing.
    repo.write("generate.sh", "#!/bin/sh\nexit 0\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "generator"]);

    let temporary = tempfile::tempdir().expect("a temporary directory");
    let output = Command::new(env!("CARGO_BIN_EXE_ratchet"))
        .arg("check")
        .current_dir(repo.path())
        .env("TMPDIR", temporary.path())
        .env("TMP", temporary.path())
        .env("TEMP", temporary.path())
        .output()
        .expect("the binary runs");

    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        entries(temporary.path()).is_empty(),
        "the temporary directory was left behind: {:?}",
        entries(temporary.path())
    );
}

#[test]
fn the_report_shows_a_repository_relative_path_for_the_generator() {
    let outside = tempfile::tempdir().expect("a temporary directory");
    let artifact = outside.path().join("ratchet-probe-outside.json");
    let repo = Repo::new(
        &format!(
            r#"[
              {{
                "rules": {{
                  "a": {{
                    "class": "c",
                    "incident": "i",
                    "unit": "files",
                    "direction": "ceiling",
                    "count": {{
                      "file": "{}",
                      "path": "coherence.items",
                      "generate": ["sh", "generate.sh"],
                      "output_arg": "--output"
                    }}
                  }}
                }}
              }}
            ]"#,
            artifact.display()
        ),
        Some(&baseline("a", 3)),
    );
    repo.write("generate.sh", "#!/bin/sh\nexit 2\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "generator"]);

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = format!("{}{}", stdout(&output), stderr(&output));
    // The report shows the file name, never the absolute path of the config.
    assert!(
        text.contains("the generator for ratchet-probe-outside.json"),
        "{text}"
    );
    assert!(
        !text.contains(&artifact.display().to_string()),
        "the report contains the absolute path: {text}"
    );
    assert_deterministic(&text);
}

#[test]
fn the_diagnostic_channel_contains_the_stdout_of_a_javascript_config() {
    if !node_available() {
        eprintln!(
            "*** SKIPPED: node is not on the PATH — this JavaScript test did NOT run. \
             Install node (or run the CI, which installs it) to exercise it. ***"
        );
        return;
    }
    let repo = Repo::plain("", None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");
    // The module writes to stdout: the export is not valid JSON any more.
    repo.write(
        "ratchet.config.mjs",
        "console.log(\"noise from the config module\");\nexport default [];\n",
    );

    let output = repo.run(&["check", "--debug"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    // The report says what happened; node's stdout is in the diagnostic
    // channel.
    assert!(text.contains("did not export valid JSON"), "{text}");
    assert!(
        text.contains("noise from the config module"),
        "the diagnostic channel does not contain node's stdout: {text}"
    );
}

#[test]
fn init_debug_prints_the_resolved_config() {
    let repo = Repo::new(ceiling_config(), Some(&baseline("todo-markers", 5)));

    let output = repo.run(&["init", "--debug"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("resolved config"), "{text}");
    assert!(text.contains("todo-markers"), "{text}");
}

#[test]
fn init_debug_says_that_there_is_nothing_to_resolve() {
    let repo = Repo::new(ceiling_config(), None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");

    let output = repo.run(&["init", "--debug"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stderr(&output);
    assert!(
        text.contains("no config found — nothing to resolve"),
        "{text}"
    );
}

// --- the fifth round of findings --------------------------------------------

/// The documented stylish example, exactly as the binary prints it.
const DOCUMENTED_EXAMPLE: &str = "ratchet — 3 rules, 2 failed\n\n  FAIL  missing-baseline              no baseline for module-test-coverage\n  FAIL  unexpected-inner-html         7 > 5  (files)\n  PASS  unwrapped-action-calls        9 ≤ 11  (call sites)\n\nNOT COHERENT — 2 of 3 gates failed\n";

/// The root of the repository — the test reads the documents from there.
fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The documented stylish example, read out of a document.
///
/// The example is the fenced code block that starts with `ratchet — `.  A
/// document that cannot be read, that has no such block, or that has more
/// than one is an error: the test must fail, never skip.
fn documented_example(document: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(document)
        .map_err(|e| format!("{} cannot be read: {e}", document.display()))?;

    let mut blocks: Vec<String> = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            match &mut current {
                None => current = Some(Vec::new()),
                Some(lines) => {
                    let content = lines.join("\n");
                    if content.starts_with("ratchet — ") {
                        blocks.push(format!("{content}\n"));
                    }
                    current = None;
                }
            }
        } else if let Some(lines) = &mut current {
            lines.push(line);
        }
    }

    match blocks.len() {
        0 => Err(format!(
            "{} has no documented stylish example",
            document.display()
        )),
        1 => Ok(blocks.remove(0)),
        _ => Err(format!(
            "{} has more than one documented stylish example",
            document.display()
        )),
    }
}

#[test]
fn the_stylish_output_is_the_documented_one() {
    // The example of README.md and SPEC.md §6.1 — byte for byte, read out of
    // the documents: they cannot drift away from the implementation.
    let config = r#"[
      {
        "rules": {
          "unwrapped-action-calls": {
            "class": "layer violation",
            "incident": "F-09: a server action was called without the shared error helper",
            "unit": "call sites",
            "direction": "ceiling",
            "count": { "command": ["echo", "9"] }
          },
          "unexpected-inner-html": {
            "class": "layer violation",
            "incident": "an unescaped HTML sink reached the renderer",
            "unit": "files",
            "direction": "ceiling",
            "count": { "command": ["echo", "7"] }
          },
          "module-test-coverage": {
            "class": "coverage",
            "incident": "a module was merged without tests",
            "unit": "modules",
            "direction": "ceiling",
            "count": { "command": ["echo", "1"] }
          }
        }
      }
    ]"#;
    let baseline = "{\n  \"version\": 1,\n  \"rules\": {\n    \"unwrapped-action-calls\": { \"value\": 11 },\n    \"unexpected-inner-html\": { \"value\": 5 }\n  }\n}\n";
    let repo = Repo::new(config, Some(baseline));

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    for document in ["README.md", "SPEC.md"] {
        let path = repository_root().join(document);
        let expected = documented_example(&path).unwrap_or_else(|error| panic!("{error}"));
        // The document carries the example the binary prints.
        assert_eq!(expected, DOCUMENTED_EXAMPLE, "{document} drifted");
        assert_eq!(stdout(&output), expected, "{document} drifted");
    }
}

#[test]
fn a_document_without_an_example_is_an_error() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let document = directory.path().join("without-example.md");
    std::fs::write(&document, "# A document\n\nNo example here.\n").expect("the file is written");

    let error = documented_example(&document).expect_err("the extraction must fail");

    assert!(error.contains("no documented stylish example"), "{error}");

    // A document that cannot be read is an error as well.
    let missing = directory.path().join("does-not-exist.md");
    let error = documented_example(&missing).expect_err("the extraction must fail");
    assert!(error.contains("cannot be read"), "{error}");
}

#[test]
fn a_document_with_two_examples_is_an_error() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let document = directory.path().join("two-examples.md");
    std::fs::write(
        &document,
        "# A document\n\n```\nratchet — 1 rule, 0 failed\n\nPASS  a  1 ≤ 1  (u)\n\nCOHERENT — 1 of 1 gates passed\n```\n\nand another one:\n\n```\nratchet — 3 rules, 2 failed\n\nFAIL  a  7 > 5  (files)\n\nNOT COHERENT — 2 of 3 gates failed\n```\n",
    )
    .expect("the file is written");

    let error = documented_example(&document).expect_err("the extraction must fail");

    assert!(
        error.contains("more than one documented stylish example"),
        "{error}"
    );
    assert!(error.contains("two-examples.md"), "{error}");
}

#[test]
fn a_generator_that_could_not_be_started_says_so() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["ratchet-generator-that-does-not-exist"],
                  "output_arg": "--output"
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );

    let output = repo.run(&["check"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = format!("{}{}", stdout(&output), stderr(&output));
    // The process never started: it did not exit with anything.
    assert!(
        text.contains("the generator for docs/_audit-raw.json could not be started"),
        "{text}"
    );
    assert!(!text.contains("exited with an unknown status"), "{text}");
}

#[test]
fn the_diagnostic_channel_shows_the_argv_of_the_generator() {
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "files",
                "direction": "ceiling",
                "count": {
                  "file": "docs/_audit-raw.json",
                  "path": "coherence.items",
                  "generate": ["sh", "generate.sh"],
                  "output_arg": "--output"
                }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 3)),
    );
    repo.write(
        "generate.sh",
        "#!/bin/sh\nprintf '{\"coherence\": {\"items\": []}}' > \"$2\"\n",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "generator"]);

    let output = repo.run(&["check", "--debug"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stderr(&output);
    // The argv that was really executed — the output path included.
    assert!(text.contains("sh generate.sh --output"), "{text}");
    assert!(text.contains("ratchet-generate-"), "{text}");

    // The argv is in the diagnostic channel even when the artifact is wrong.
    repo.write("generate.sh", "#!/bin/sh\nprintf '{}' > \"$2\"\n");
    let output = repo.run(&["check", "--debug"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("sh generate.sh --output"), "{text}");
    // The report says what happened; the diagnostic channel shows the argv.
    assert!(stdout(&output).contains("not found"), "{}", stdout(&output));
}

#[test]
fn the_diagnostic_channel_shows_the_stderr_of_a_javascript_config() {
    if !node_available() {
        eprintln!(
            "*** SKIPPED: node is not on the PATH — this JavaScript test did NOT run. \
             Install node (or run the CI, which installs it) to exercise it. ***"
        );
        return;
    }
    let repo = Repo::plain("", None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");
    // The module writes to stdout (the export is not valid JSON any more) and
    // a warning to stderr.
    repo.write(
        "ratchet.config.mjs",
        "console.error(\"a warning from the config module\");\nconsole.log(\"noise\");\nexport default [];\n",
    );

    let output = repo.run(&["check", "--debug"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("a warning from the config module"), "{text}");
    assert!(text.contains("noise"), "{text}");
    // The transcript has the same shape as everywhere else.
    assert!(text.contains("--- stdout ---"), "{text}");
    assert!(text.contains("--- stderr ---"), "{text}");
}

// --- the seventh round of findings ------------------------------------------

#[test]
fn the_error_comes_before_the_transcript() {
    // A broken JavaScript config: the run fails and the diagnostic channel
    // carries the transcript of node.
    if !node_available() {
        eprintln!(
            "*** SKIPPED: node is not on the PATH — this JavaScript test did NOT run. \
             Install node (or run the CI, which installs it) to exercise it. ***"
        );
        return;
    }
    let repo = Repo::plain("", None);
    std::fs::remove_file(repo.path().join("ratchet.config.jsonc")).expect("the config is removed");
    repo.write("ratchet.config.mjs", "export default [\n");

    let output = repo.run(&["check", "--debug"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    let error = text.find("ratchet: error:").expect("the error is reported");
    let transcript = text
        .find("ratchet: debug:")
        .expect("the transcript is printed");
    assert!(
        error < transcript,
        "the error must come before the transcript: {text}"
    );
}

#[test]
fn the_error_comes_before_the_transcript_of_a_failing_counter() {
    // A failing counter is a failed gate, not an operational error: the
    // report carries the rule, the transcript carries the output of the
    // counter.
    let repo = Repo::new(
        r#"[
          {
            "rules": {
              "a": {
                "class": "c",
                "incident": "i",
                "unit": "u",
                "direction": "ceiling",
                "count": { "command": ["sh", "count.sh"] }
              }
            }
          }
        ]"#,
        Some(&baseline("a", 1)),
    );
    repo.write(
        "count.sh",
        "#!/bin/sh\necho 'the reason the counter failed' >&2\nexit 1\n",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "counter"]);

    let output = repo.run(&["check", "--debug"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let text = stderr(&output);
    // The report says what happened, the diagnostic channel the output.
    assert!(
        stdout(&output).contains("the counter exited with status 1"),
        "{text}"
    );
    assert!(
        text.contains("the reason the counter failed"),
        "the diagnostic channel does not carry the output: {text}"
    );
}

#[test]
fn an_empty_class_incident_or_unit_is_a_configuration_error() {
    for (field, value) in [("class", ""), ("incident", ""), ("unit", "")] {
        let config = format!(
            r#"[
              {{
                "rules": {{
                  "a": {{
                    "class": "c",
                    "incident": "i",
                    "unit": "u",
                    "direction": "ceiling",
                    "count": {{ "command": ["echo", "1"] }},
                    "{field}": "{value}"
                  }}
                }}
              }}
            ]"#
        );
        let repo = Repo::new(&config, Some(&baseline("a", 1)));

        let output = repo.run(&["check"]);

        assert_eq!(code(&output), 2, "{field}: {}", stderr(&output));
        let text = stderr(&output);
        assert!(
            text.contains(&format!("rules.a.{field}")),
            "{field}: {text}"
        );
        assert!(text.contains("must not be empty"), "{field}: {text}");
    }
}
