//! End-to-end tests of the issues counter: a temporary project, the real
//! binary and a **local fake server** that answers like the issues API.
//!
//! No test touches the network: the base of the API is pointed at a socket of
//! the test, and the token comes from the environment of the child process.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output};
use std::thread;

use common::TempDir;
use serde_json::Value;

/// The repository of the tests.
const REPOSITORY: &str = "acme/widgets";

/// The token of the tests; the fake server does not verify it.
const TOKEN: &str = "test-token";

/// The query of the first page: the API is asked for every state, the
/// membership label and the largest page.
const QUERY: &str = "state=all&labels=ratchet&per_page=100";

/// One answer of the fake server.
struct Route {
    /// The path of the request, query included.
    path: String,
    /// The status line of the answer.
    status: &'static str,
    /// The body of the answer.
    body: Vec<u8>,
    /// The URL of the next page, if the answer names one.
    link: Option<String>,
}

impl Route {
    /// An answer with a body.
    fn ok(path: String, body: Vec<u8>) -> Route {
        Route {
            path,
            status: "200 OK",
            body,
            link: None,
        }
    }

    /// An answer with a status and no body.
    fn failing(path: String, status: &'static str) -> Route {
        Route {
            path,
            status,
            body: Vec::new(),
            link: None,
        }
    }

    /// An answer with a body and a next page.
    fn linked(path: String, body: Vec<u8>, next: String) -> Route {
        Route {
            path,
            status: "200 OK",
            body,
            link: Some(next),
        }
    }
}

/// A fake issues API: a local socket that answers fixed routes.
///
/// The routes are built with the base of the socket, so a `Link` header can
/// name a page of the same server.
struct FakeIssues {
    /// The base of the API.
    api_base: String,
}

impl FakeIssues {
    /// Starts a server that serves the given routes.
    fn start(build: impl FnOnce(&str) -> Vec<Route>) -> FakeIssues {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the socket is bound");
        let address = listener.local_addr().expect("the address is known");
        let api_base = format!("http://{address}");
        let routes = build(&api_base);
        thread::spawn(move || {
            // The API is not anonymous: every request of the counter has to
            // carry the token of the test.  An unauthorized request answers
            // 401, so a counter that forgot the header fails every test and
            // not only the one that looks for the failure.
            let expected = format!("authorization: bearer {}", TOKEN.to_ascii_lowercase());
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    continue;
                };
                let request = read_request(&mut stream);
                if !request.to_ascii_lowercase().contains(&expected) {
                    let response = head("401 Unauthorized", 0, None);
                    let _ = stream.write_all(&response);
                    let _ = stream.flush();
                    continue;
                }
                let path = request_path(&request);
                let route = routes.iter().find(|route| route.path == path);
                let mut response = match route {
                    Some(route) => head(route.status, route.body.len(), route.link.as_deref()),
                    None => head("404 Not Found", 0, None),
                };
                if let Some(route) = route {
                    response.extend_from_slice(&route.body);
                }
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            }
        });
        FakeIssues { api_base }
    }
}

/// The head of a response.
fn head(status: &str, length: usize, link: Option<&str>) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {length}\r\n\
         Content-Type: application/json\r\nConnection: close\r\n"
    );
    if let Some(link) = link {
        head.push_str(&format!("Link: <{link}>; rel=\"next\"\r\n"));
    }
    head.push_str("\r\n");
    head.into_bytes()
}

/// The head of a request.
fn read_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        let Ok(read) = stream.read(&mut buffer) else {
            return String::new();
        };
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&request).into_owned()
}

/// The path of a request, query included, read from its request line.
fn request_path(request: &str) -> String {
    let line = request.lines().next().unwrap_or_default();
    let mut fields = line.split_whitespace();
    let _method = fields.next();
    fields.next().unwrap_or_default().to_string()
}

/// The path of the first page.
fn issues_path() -> String {
    format!("/repos/{REPOSITORY}/issues?{QUERY}")
}

/// The path of a later page.
fn page_path(page: usize) -> String {
    format!("{}&page={page}", issues_path())
}

/// The JSON of one issue.
fn issue(number: u64, title: &str, state: &str, labels: &[&str], assignees: &[&str]) -> String {
    let labels: Vec<String> = labels
        .iter()
        .map(|label| format!("{{\"name\": \"{label}\"}}"))
        .collect();
    let assignees: Vec<String> = assignees
        .iter()
        .map(|login| format!("{{\"login\": \"{login}\"}}"))
        .collect();
    format!(
        "{{\"number\": {number}, \"title\": \"{title}\", \"state\": \"{state}\", \
         \"labels\": [{}], \"assignees\": [{}]}}",
        labels.join(", "),
        assignees.join(", ")
    )
}

/// The JSON of one pull request: the issues API carries it with the issues.
fn pull_request(number: u64) -> String {
    format!(
        "{{\"number\": {number}, \"title\": \"a pull request\", \"state\": \"open\", \
         \"labels\": [], \"assignees\": [], \
         \"pull_request\": {{\"url\": \"https://api.github.com/pulls/{number}\"}}}}"
    )
}

/// The JSON body of a page.
fn page(issues: &[String]) -> Vec<u8> {
    format!("[{}]", issues.join(", ")).into_bytes()
}

/// The config of the test project; the count object is the parameter.
fn config(count: &str) -> String {
    format!(
        r#"[{{
          "name": "test",
          "rules": {{
            "open-findings": {{
              "class": "unresolved findings",
              "incident": "prevention: the findings live in the tracker",
              "unit": "open findings",
              "direction": "ceiling",
              "tier": "slow",
              "count": {count}
            }}
          }}
        }}]"#
    )
}

/// The default count object of the tests.
fn count_object() -> String {
    format!(r#"{{ "issues": {{ "repository": "{REPOSITORY}" }} }}"#)
}

/// A baseline with one record.
fn baseline(value: i64) -> String {
    format!(
        "{{\n  \"version\": 1,\n  \"rules\": {{\n    \"open-findings\": {{ \"value\": {value} }}\n  }}\n}}\n"
    )
}

/// A project with a rule that counts the issues.
fn project(count: &str, baseline_value: i64) -> TempDir {
    let directory = TempDir::new().expect("a temporary directory");
    fs::write(directory.path().join("ratchet.config.jsonc"), config(count))
        .expect("the config is written");
    fs::write(
        directory.path().join("ratchet.baseline.json"),
        baseline(baseline_value),
    )
    .expect("the baseline is written");
    directory
}

/// The command that runs the binary in the project.
///
/// The token is removed from the inherited environment: a token of the machine
/// that runs the tests must not decide what a test proves.  The update check
/// needs the network and the tests do not.
fn command(project: &Path, api_base: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ratchet"));
    command
        .current_dir(project)
        .env("RATCHET_ISSUES_API_BASE", api_base)
        .env("RATCHET_NO_UPDATE_CHECK", "1")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN");
    command
}

/// Runs the binary with a token in the environment.
fn run(project: &Path, api_base: &str, arguments: &[&str]) -> Output {
    command(project, api_base)
        .env("GITHUB_TOKEN", TOKEN)
        .args(arguments)
        .output()
        .expect("the binary runs")
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

/// The count of the rule in the JSON report.
fn count_of(output: &Output) -> i64 {
    let report: Value = serde_json::from_str(&stdout(output)).expect("the stdout is JSON");
    report["rules"][0]["count"]
        .as_i64()
        .expect("the rule has a count")
}

/// The detail of the rule in the JSON report.
fn detail_of(output: &Output) -> String {
    let report: Value = serde_json::from_str(&stdout(output)).expect("the stdout is JSON");
    report["rules"][0]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn the_count_is_the_number_of_the_open_issues() {
    // #1 (critical), #2 (no level: medium) and #4 are open, #3 is closed.
    let body = page(&[
        issue(
            1,
            "a critical one",
            "open",
            &["ratchet", "severity:critical"],
            &["alice"],
        ),
        issue(2, "no level", "open", &["ratchet"], &[]),
        issue(
            3,
            "a closed one",
            "closed",
            &["ratchet", "severity:high"],
            &[],
        ),
        issue(4, "a low one", "open", &["ratchet", "severity:low"], &[]),
    ]);
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(&count_object(), 3);

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 3);
}

#[test]
fn a_pull_request_does_not_count() {
    let body = page(&[
        issue(1, "an issue", "open", &["ratchet"], &[]),
        pull_request(2),
    ]);
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(&count_object(), 1);

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 1);
}

#[test]
fn the_severity_filter_narrows_the_count() {
    let body = page(&[
        issue(
            1,
            "a critical one",
            "open",
            &["ratchet", "severity:critical"],
            &[],
        ),
        issue(2, "a high one", "open", &["ratchet", "severity:high"], &[]),
        issue(
            3,
            "a medium one",
            "open",
            &["ratchet", "severity:medium"],
            &[],
        ),
    ]);
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(
        &format!(
            r#"{{ "issues": {{ "repository": "{REPOSITORY}" }}, "severities": ["critical", "high"] }}"#
        ),
        2,
    );

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 2);
}

#[test]
fn an_issue_without_a_severity_label_counts_as_the_default_level() {
    // The default level is `medium`; the rule counts the medium ones.
    let body = page(&[
        issue(1, "no level", "open", &["ratchet"], &[]),
        issue(2, "a low one", "open", &["ratchet", "severity:low"], &[]),
    ]);
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(
        &format!(r#"{{ "issues": {{ "repository": "{REPOSITORY}" }}, "severities": ["medium"] }}"#),
        1,
    );

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 1);
}

#[test]
fn the_statuses_filter_counts_a_closed_issue_as_fixed() {
    let body = page(&[
        issue(
            1,
            "an open one",
            "open",
            &["ratchet", "severity:medium"],
            &[],
        ),
        issue(
            2,
            "a closed one",
            "closed",
            &["ratchet", "severity:high"],
            &[],
        ),
    ]);
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(
        &format!(r#"{{ "issues": {{ "repository": "{REPOSITORY}" }}, "statuses": ["fixed"] }}"#),
        1,
    );

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 1);
}

#[test]
fn a_custom_label_is_requested() {
    let body = page(&[issue(1, "an issue", "open", &["gate"], &[])]);
    let server = FakeIssues::start(|_| {
        vec![Route::ok(
            format!("/repos/{REPOSITORY}/issues?state=all&labels=gate&per_page=100"),
            body,
        )]
    });
    let project = project(
        &format!(r#"{{ "issues": {{ "repository": "{REPOSITORY}", "label": "gate" }} }}"#),
        1,
    );

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 1);
}

#[test]
fn a_missing_token_is_an_operational_error() {
    // Nothing listens here: the counter must fail before a request.
    let project = project(&count_object(), 0);

    let output = command(project.path(), "http://127.0.0.1:1")
        .args(["check", "--format", "json"])
        .output()
        .expect("the binary runs");

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("GITHUB_TOKEN"), "{message}");
    assert!(message.contains("GH_TOKEN"), "{message}");
}

#[test]
fn the_gh_token_is_the_fallback() {
    let body = page(&[issue(1, "an issue", "open", &["ratchet"], &[])]);
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(&count_object(), 1);

    let output = command(project.path(), &server.api_base)
        .env("GH_TOKEN", TOKEN)
        .args(["check", "--format", "json"])
        .output()
        .expect("the binary runs");

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 1);
}

#[test]
fn a_failing_request_is_an_operational_error() {
    let server =
        FakeIssues::start(|_| vec![Route::failing(issues_path(), "500 Internal Server Error")]);
    let project = project(&count_object(), 0);

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("500"), "{message}");
}

#[test]
fn a_success_status_other_than_200_is_an_operational_error() {
    let body = page(&[issue(1, "an issue", "open", &["ratchet"], &[])]);
    let server = FakeIssues::start(|_| {
        vec![Route {
            path: issues_path(),
            status: "201 Created",
            body,
            link: None,
        }]
    });
    let project = project(&count_object(), 0);

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("201"), "{message}");
}

#[test]
fn an_invalid_json_answer_is_an_operational_error() {
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), b"not json".to_vec())]);
    let project = project(&count_object(), 0);

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("not valid JSON"), "{message}");
}

#[test]
fn a_missing_field_is_an_operational_error() {
    // An issue without a title.
    let body = "[{\"number\": 3, \"state\": \"open\", \"labels\": [{\"name\": \"ratchet\"}], \"assignees\": []}]"
        .to_string()
        .into_bytes();
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(&count_object(), 0);

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("#3"), "{message}");
    assert!(message.contains("title"), "{message}");
}

#[test]
fn missing_assignees_is_an_operational_error() {
    let body = "[{\"number\": 3, \"title\": \"an issue\", \"state\": \"open\", \"labels\": [{\"name\": \"ratchet\"}]}]"
        .to_string()
        .into_bytes();
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(&count_object(), 0);

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("#3"), "{message}");
    assert!(message.contains("assignees"), "{message}");
}

#[test]
fn an_issue_without_the_membership_label_is_not_counted() {
    // The response deliberately violates the API filter; the counter still
    // verifies membership instead of trusting the response to be filtered.
    let body = page(&[
        issue(1, "a member", "open", &["ratchet"], &[]),
        issue(2, "not a member", "open", &["unrelated"], &[]),
    ]);
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(&count_object(), 1);

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 1);
}

#[test]
fn an_unknown_severity_label_is_an_operational_error() {
    let body = page(&[issue(
        7,
        "a typo",
        "open",
        &["ratchet", "severity:blocker"],
        &[],
    )]);
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(&count_object(), 0);

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("#7"), "{message}");
    assert!(message.contains("severity:blocker"), "{message}");
}

#[test]
fn the_pagination_follows_the_link_header() {
    let first = page(&[
        issue(1, "the first", "open", &["ratchet"], &[]),
        issue(2, "the second", "open", &["ratchet"], &[]),
    ]);
    let second = page(&[issue(3, "the third", "open", &["ratchet"], &[])]);
    let server = FakeIssues::start(|base| {
        let next = format!("{base}{}", page_path(2));
        vec![
            Route::linked(issues_path(), first, next),
            Route::ok(page_path(2), second),
        ]
    });
    let project = project(&count_object(), 3);

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 0, "{}", said(&output));
    assert_eq!(count_of(&output), 3);
}

#[test]
fn more_than_twenty_pages_is_an_operational_error() {
    let server = FakeIssues::start(|base| {
        (1..=25)
            .map(|page_number| {
                let path = if page_number == 1 {
                    issues_path()
                } else {
                    page_path(page_number)
                };
                let body = page(&[issue(
                    page_number as u64,
                    "an issue",
                    "open",
                    &["ratchet"],
                    &[],
                )]);
                if page_number < 25 {
                    let next = format!("{base}{}", page_path(page_number + 1));
                    Route::linked(path, body, next)
                } else {
                    Route::ok(path, body)
                }
            })
            .collect()
    });
    let project = project(&count_object(), 0);

    let output = run(
        project.path(),
        &server.api_base,
        &["check", "--format", "json"],
    );

    assert_eq!(code(&output), 2, "{}", said(&output));
    let message = detail_of(&output);
    assert!(message.contains("20 pages"), "{message}");
}

#[test]
fn the_report_shows_the_findings() {
    // The answer is out of order: the report walks the issue numbers upwards.
    let body = page(&[
        issue(
            10,
            "the tenth",
            "open",
            &["ratchet", "severity:high"],
            &["bob"],
        ),
        issue(
            2,
            "the second",
            "open",
            &["ratchet", "severity:critical"],
            &["alice"],
        ),
    ]);
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(&count_object(), 2);

    let output = run(
        project.path(),
        &server.api_base,
        &["report", "--format", "md"],
    );

    assert_eq!(code(&output), 0, "{}", said(&output));
    let text = stdout(&output);
    assert!(text.contains("## Findings"), "{text}");
    assert!(text.contains("### open-findings"), "{text}");
    assert!(
        text.contains("| #2 | critical | open | the second | alice | #2 |"),
        "{text}"
    );
    assert!(
        text.contains("| #10 | high | open | the tenth | bob | #10 |"),
        "{text}"
    );
    let second = text.find("| #2 |").expect("the second is shown");
    let tenth = text.find("| #10 |").expect("the tenth is shown");
    assert!(second < tenth, "the findings are ordered by issue number");
}

#[test]
fn two_runs_produce_the_same_bytes() {
    let body = page(&[
        issue(3, "the third", "open", &["ratchet"], &[]),
        issue(1, "the first", "open", &["ratchet"], &[]),
    ]);
    let server = FakeIssues::start(|_| vec![Route::ok(issues_path(), body)]);
    let project = project(&count_object(), 2);

    // The report carries the findings table: the same answer has to produce
    // the same rows in the same order, whatever order the page arrived in.
    let first = run(
        project.path(),
        &server.api_base,
        &["report", "--format", "md"],
    );
    let second = run(
        project.path(),
        &server.api_base,
        &["report", "--format", "md"],
    );

    assert_eq!(code(&first), 0, "{}", said(&first));
    assert!(stdout(&first).contains("| #1 |"), "{}", stdout(&first));
    assert_eq!(stdout(&first), stdout(&second));
}
