//! The issues backend: the findings registry of a GitHub repository.
//!
//! A rules counter can read its findings from the issue tracker instead of a
//! file beside it (SPEC 13).  The mechanic is the registry's, unchanged: the
//! count is the number of the findings that count, the findings travel with
//! the count, and a failure is a **closed** gate — never a zero.
//!
//! # The network exception
//!
//! The engine opens no connection at runtime except the update check.  This
//! module is the second exception, and it is **opt-in per config**: a project
//! that does not configure `issues` never reaches this code.
//!
//! The base of the API is overridable through [`API_BASE_ENV`] and through
//! nothing else.  A base in the config would move the trust anchor: the config
//! is a file a project commits, and the request carries the token — a config
//! could send the token of the person who runs the gate to any host it names.
//! An environment variable is set by the person who runs the command (and by
//! the tests, which point it at a local socket).

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use crate::registry::{Finding, SEVERITIES, Severity, Status};

/// The default base of the issues API.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";

/// The environment variable that overrides the base of the issues API.
pub const API_BASE_ENV: &str = "RATCHET_ISSUES_API_BASE";

/// The label that makes an issue a finding of the gate.
pub const DEFAULT_LABEL: &str = "ratchet";

/// The prefix of a severity label.
pub const DEFAULT_SEVERITY_PREFIX: &str = "severity:";

/// The severity of an issue without a severity label.
pub const DEFAULT_SEVERITY: Severity = Severity::Medium;

/// The environment variables a token is read from, in order.
pub const TOKEN_ENV: [&str; 2] = ["GITHUB_TOKEN", "GH_TOKEN"];

/// Optional override for the GitHub CLI executable, primarily for tests.
pub const GH_PATH_ENV: &str = "RATCHET_GH_PATH";

/// How long one request may take.
///
/// The counter runs inside a gate: a request that hangs must not hang the
/// gate.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// How many pages the pagination follows.
///
/// A bounded loop is a loop that ends: a tracker that answers with an endless
/// chain of pages is an error, not a run that never returns.
pub const MAX_PAGES: usize = 20;

/// What the counter counts.
pub struct Options<'a> {
    /// The repository: `owner/repo`.
    pub repository: &'a str,
    /// The label that makes an issue a finding.
    pub label: &'a str,
    /// The prefix of a severity label.
    pub severity_prefix: &'a str,
    /// The severity of an issue without a severity label.
    pub default_severity: Severity,
    /// The levels that count; every level counts without the list.
    pub severities: Option<&'a [Severity]>,
    /// The statuses that count; the counting statuses count without the list.
    pub statuses: Option<&'a [Status]>,
}

/// A failure of the issues backend.
///
/// Every one of them is an operational error: the count is not delivered, and
/// the caller must not turn the gap into a zero.
#[derive(Debug, Clone)]
pub struct Error {
    message: String,
}

impl Error {
    /// Creates an error with the given message.
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The message of the error.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

/// The base of the issues API: the environment overrides the default.
pub fn api_base() -> String {
    std::env::var(API_BASE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
}

/// The token of the request: `GITHUB_TOKEN` first, `GH_TOKEN` second.
fn token_from_env(repository: &str) -> Result<String, Error> {
    for name in TOKEN_ENV {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim();
            if !value.is_empty() {
                return Ok(value.to_string());
            }
        }
    }
    let gh = std::env::var_os(GH_PATH_ENV).unwrap_or_else(|| "gh".into());
    match std::process::Command::new(gh)
        .args(["auth", "token"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
            Err(token_unavailable(repository))
        }
        _ => Err(token_unavailable(repository)),
    }
}

fn token_unavailable(repository: &str) -> Error {
    Error::new(format!(
        "the issues of {repository} cannot be read: neither GITHUB_TOKEN nor GH_TOKEN is set, \
         and GitHub CLI authentication is unavailable — run `gh auth login` or set a token"
    ))
}

/// Reads the findings of the repository and selects the ones that count.
///
/// The list is what the count is made of: the report shows **these** findings,
/// not a second reading of the API (SPEC 14).
pub fn findings(options: &Options<'_>) -> Result<Vec<Finding>, Error> {
    let base = api_base();
    let token = token_from_env(options.repository)?;
    let first_page = format!(
        "{base}/repos/{}/issues?state=all&labels={}&per_page=100",
        options.repository,
        percent_encode(options.label)
    );

    let mut found: BTreeMap<u64, Finding> = BTreeMap::new();
    let mut url = first_page;
    let mut page = 1;
    loop {
        let (body, next) = get(&url, &token)?;
        for (number, finding) in parse_page(&body, options)? {
            // The number is the identity of the issue: a page that repeats
            // one does not count it twice.
            found.insert(number, finding);
        }
        let Some(next) = next else {
            break;
        };
        if page >= MAX_PAGES {
            return Err(Error::new(format!(
                "the issues of {} span more than {MAX_PAGES} pages — the pagination is bounded",
                options.repository
            )));
        }
        if !under_base(&next, &base) {
            return Err(Error::new(format!(
                "the issues of {} cannot be read: the next page is not under {base} — \
                 the token is not sent elsewhere",
                options.repository
            )));
        }
        url = next;
        page += 1;
    }

    // The map is ordered by issue number: the same data produces the same
    // bytes, whatever order the pages arrived in.
    let findings: Vec<Finding> = found
        .into_values()
        .filter(|finding| match options.severities {
            None => true,
            Some(levels) => levels.contains(&finding.severity),
        })
        .filter(|finding| match options.statuses {
            None => finding.status.counts(),
            Some(list) => list.contains(&finding.status),
        })
        .collect();
    Ok(findings)
}

/// True when a next page stays under the base the token was sent to.
fn under_base(next: &str, base: &str) -> bool {
    next == base || next.starts_with(&format!("{base}/"))
}

/// Reads one page: the issues and the next page, if the answer names one.
fn get(url: &str, token: &str) -> Result<(Vec<u8>, Option<String>), Error> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .header("User-Agent", user_agent())
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|error| match error {
            ureq::Error::StatusCode(code) => {
                Error::new(format!("{url} answers with the status {code}"))
            }
            other => Error::new(format!("{url} cannot be fetched: {other}")),
        })?;
    if response.status() != 200 {
        return Err(Error::new(format!(
            "{url} answers with the status {}",
            response.status()
        )));
    }
    // The header has to be read before the body: reading the body consumes
    // the response.
    let link = response
        .headers()
        .get("link")
        .and_then(|value| value.to_str().ok())
        .and_then(next_link);
    let body = response
        .body_mut()
        .read_to_vec()
        .map_err(|error| Error::new(format!("the body of {url} cannot be read: {error}")))?;
    Ok((body, link))
}

/// The `rel="next"` URL of a `Link` header.
fn next_link(header: &str) -> Option<String> {
    for part in header.split(',') {
        let mut fields = part.split(';');
        let Some(url) = fields.next() else {
            continue;
        };
        // A part that is not `<url>; rel="…"` is not a link and is skipped:
        // one odd part must not hide the next one.
        let Some(url) = url
            .trim()
            .strip_prefix('<')
            .and_then(|url| url.strip_suffix('>'))
        else {
            continue;
        };
        if fields.any(|parameter| {
            let parameter = parameter.trim();
            parameter == "rel=\"next\"" || parameter == "rel=next"
        }) {
            return Some(url.to_string());
        }
    }
    None
}

/// The agent the requests identify themselves with.
///
/// The GitHub API refuses a request without one.
fn user_agent() -> String {
    format!("ratchet/{}", env!("CARGO_PKG_VERSION"))
}

/// The issues of one page, with the numbers that identify them.
///
/// A pull request is not an issue of the tracker, although the issues API
/// carries it: an entry with a `pull_request` field is skipped.
fn parse_page(body: &[u8], options: &Options<'_>) -> Result<Vec<(u64, Finding)>, Error> {
    let document: Value = serde_json::from_slice(body).map_err(|error| {
        Error::new(format!(
            "the issues of {} cannot be read: the answer is not valid JSON: {error}",
            options.repository
        ))
    })?;
    let items = document.as_array().ok_or_else(|| {
        Error::new(format!(
            "the issues of {} cannot be read: the answer is not an array",
            options.repository
        ))
    })?;

    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(issue) = parse_issue(item, index, options)? {
            out.push(issue);
        }
    }
    Ok(out)
}

/// One issue of a page; `None` for a pull request.
fn parse_issue(
    value: &Value,
    index: usize,
    options: &Options<'_>,
) -> Result<Option<(u64, Finding)>, Error> {
    let repository = options.repository;
    let map = value.as_object().ok_or_else(|| {
        Error::new(format!(
            "the issues of {repository} cannot be read: entry {index} is not an object"
        ))
    })?;
    if map.contains_key("pull_request") {
        return Ok(None);
    }

    let number = map
        .get("number")
        .and_then(Value::as_u64)
        .ok_or_else(|| missing(repository, index, "number"))?;
    let entry = format!("#{number}");

    let labels = map
        .get("labels")
        .and_then(Value::as_array)
        .ok_or_else(|| missing_entry(repository, &entry, "labels"))?;
    // The level of an issue is a label; an issue without one is the default
    // level.  A label with the prefix and a level outside the vocabulary is
    // an error: a silent fallback would hide the typo that changed a gate.
    let mut is_member = false;
    for label in labels {
        let name = label.get("name").and_then(Value::as_str).ok_or_else(|| {
            Error::new(format!(
                "the issues of {repository} cannot be read: a label of {entry} has no \"name\""
            ))
        })?;
        if name == options.label {
            is_member = true;
        }
    }
    // The API query requests this label, but membership is a property of the
    // finding, not of the query. Check the response too: a server or proxy
    // that ignores the filter must not widen the gate.
    if !is_member {
        return Ok(None);
    }

    // The level of an issue is a label; an issue without one is the default
    // level. A label with the prefix and a level outside the vocabulary is an
    // error: a silent fallback would hide the typo that changed a gate.
    let mut levels: Vec<(Severity, String)> = Vec::new();
    for label in labels {
        let name = label.get("name").and_then(Value::as_str).ok_or_else(|| {
            Error::new(format!(
                "the issues of {repository} cannot be read: a label of {entry} has no \"name\""
            ))
        })?;
        let Some(level) = name.strip_prefix(options.severity_prefix) else {
            continue;
        };
        let Some(severity) = Severity::parse(level) else {
            return Err(Error::new(format!(
                "the issues of {repository} cannot be read: {entry} carries the label \"{name}\", \
                 which is no severity — expected {}<{}>",
                options.severity_prefix,
                SEVERITIES.join("|")
            )));
        };
        if !levels.iter().any(|(known, _)| *known == severity) {
            levels.push((severity, name.to_string()));
        }
    }
    if levels.len() > 1 {
        let names: Vec<&str> = levels.iter().map(|(_, name)| name.as_str()).collect();
        return Err(Error::new(format!(
            "the issues of {repository} cannot be read: {entry} carries the severity labels {} \
             — one issue carries one level",
            names.join(", ")
        )));
    }
    let severity = levels
        .first()
        .map(|(severity, _)| *severity)
        .unwrap_or(options.default_severity);

    let title = required_string(map, "title", repository, &entry)?;
    let state = required_string(map, "state", repository, &entry)?;
    let status = match state.as_str() {
        "open" => Status::Open,
        "closed" => Status::Fixed,
        other => {
            return Err(Error::new(format!(
                "the issues of {repository} cannot be read: {entry} has the state \"{other}\" \
                 — expected \"open\" or \"closed\""
            )));
        }
    };

    // The owner is the first assignee: an issue without one has nobody, and
    // the report says so with a dash.
    let assignees = map
        .get("assignees")
        .and_then(Value::as_array)
        .ok_or_else(|| missing_entry(repository, &entry, "assignees"))?;
    let owner = match assignees.first() {
        None => None,
        Some(assignee) => Some(
            assignee
                .get("login")
                .and_then(Value::as_str)
                .filter(|login| !login.trim().is_empty())
                .ok_or_else(|| {
                    Error::new(format!(
                        "the issues of {repository} cannot be read: the first assignee of \
                         {entry} has no \"login\""
                    ))
                })?
                .to_string(),
        ),
    };

    Ok(Some((
        number,
        Finding {
            id: entry.clone(),
            title,
            severity,
            status,
            status_since: None,
            owner,
            ticket: Some(entry),
            regression_test: None,
            path: None,
            note: None,
        },
    )))
}

/// A required string field of an issue.
fn required_string(
    map: &serde_json::Map<String, Value>,
    field: &str,
    repository: &str,
    entry: &str,
) -> Result<String, Error> {
    let value = map.get(field).ok_or_else(|| {
        Error::new(format!(
            "the issues of {repository} cannot be read: {entry} has no \"{field}\""
        ))
    })?;
    value.as_str().map(str::to_string).ok_or_else(|| {
        Error::new(format!(
            "the issues of {repository} cannot be read: {entry} has a \"{field}\" that is \
                 not a string"
        ))
    })
}

/// The message of a field that is missing before the issue has a number.
fn missing(repository: &str, index: usize, field: &str) -> Error {
    Error::new(format!(
        "the issues of {repository} cannot be read: entry {index} has no \"{field}\""
    ))
}

/// The message of a field that is missing after the issue has a number.
fn missing_entry(repository: &str, entry: &str, field: &str) -> Error {
    Error::new(format!(
        "the issues of {repository} cannot be read: {entry} has no \"{field}\""
    ))
}

/// The percent encoding of a query parameter.
///
/// Everything but the unreserved characters of RFC 3986 is encoded: a label
/// may carry a space, a colon or a non-ASCII character, and the query has to
/// say what the label is.
fn percent_encode(text: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => {
                out.push('%');
                out.push(char::from(HEX[usize::from(other >> 4)]));
                out.push(char::from(HEX[usize::from(other & 0x0f)]));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options<'a>(
        severities: Option<&'a [Severity]>,
        statuses: Option<&'a [Status]>,
    ) -> Options<'a> {
        Options {
            repository: "acme/widgets",
            label: DEFAULT_LABEL,
            severity_prefix: DEFAULT_SEVERITY_PREFIX,
            default_severity: DEFAULT_SEVERITY,
            severities,
            statuses,
        }
    }

    fn page(issues: &[String]) -> Vec<u8> {
        format!("[{}]", issues.join(", ")).into_bytes()
    }

    fn issue(number: u64, state: &str, labels: &[&str]) -> String {
        let labels: Vec<String> = labels
            .iter()
            .map(|label| format!("{{\"name\": \"{label}\"}}"))
            .collect();
        format!(
            "{{\"number\": {number}, \"title\": \"the issue {number}\", \"state\": \"{state}\", \
             \"labels\": [{}], \"assignees\": []}}",
            labels.join(", ")
        )
    }

    #[test]
    fn a_link_header_names_the_next_page() {
        let header = "<https://api.github.com/repositories/1/issues?page=2>; rel=\"next\", \
                      <https://api.github.com/repositories/1/issues?page=5>; rel=\"last\"";
        assert_eq!(
            next_link(header).as_deref(),
            Some("https://api.github.com/repositories/1/issues?page=2")
        );
        assert_eq!(
            next_link("<https://api.github.com/x?page=2>; rel=\"last\""),
            None
        );
    }

    #[test]
    fn a_label_is_percent_encoded() {
        assert_eq!(percent_encode("ratchet"), "ratchet");
        assert_eq!(percent_encode("severity:high"), "severity%3Ahigh");
        assert_eq!(percent_encode("good first issue"), "good%20first%20issue");
        assert_eq!(percent_encode("ä"), "%C3%A4");
    }

    #[test]
    fn a_pull_request_is_skipped() {
        let body = page(&[
            issue(1, "open", &["ratchet"]),
            "{\"number\": 2, \"title\": \"a PR\", \"state\": \"open\", \"labels\": [], \
             \"pull_request\": {\"url\": \"https://api.github.com/pulls/2\"}}"
                .to_string(),
        ]);
        let parsed = parse_page(&body, &options(None, None)).expect("the page parses");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, 1);
    }

    #[test]
    fn an_issue_without_a_severity_label_is_the_default_level() {
        let body = page(&[issue(3, "open", &["ratchet"])]);
        let parsed = parse_page(&body, &options(None, None)).expect("the page parses");
        assert_eq!(parsed[0].1.severity, Severity::Medium);
    }

    #[test]
    fn an_issue_without_the_membership_label_is_not_a_finding() {
        let body = page(&[issue(8, "open", &["severity:blocker"])]);
        let parsed = parse_page(&body, &options(None, None)).expect("the issue is not a finding");
        assert!(parsed.is_empty());
    }

    #[test]
    fn an_unknown_severity_label_names_the_issue_and_the_label() {
        let body = page(&[issue(7, "open", &["ratchet", "severity:blocker"])]);
        let error = parse_page(&body, &options(None, None)).expect_err("the label is unknown");
        assert!(error.message().contains("#7"), "{}", error.message());
        assert!(
            error.message().contains("severity:blocker"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn two_severity_labels_on_one_issue_are_an_error() {
        let body = page(&[issue(
            4,
            "open",
            &["ratchet", "severity:high", "severity:low"],
        )]);
        let error = parse_page(&body, &options(None, None)).expect_err("two levels");
        assert!(error.message().contains("#4"), "{}", error.message());
    }

    #[test]
    fn a_closed_issue_is_fixed() {
        let body = page(&[issue(5, "closed", &["ratchet", "severity:high"])]);
        let parsed = parse_page(&body, &options(None, None)).expect("the page parses");
        assert_eq!(parsed[0].1.status, Status::Fixed);
        assert_eq!(parsed[0].1.ticket.as_deref(), Some("#5"));
    }

    #[test]
    fn the_next_page_stays_under_the_base() {
        assert!(under_base(
            "https://api.github.com/repositories/1/issues?page=2",
            "https://api.github.com"
        ));
        assert!(!under_base(
            "https://example.com/repositories/1/issues?page=2",
            "https://api.github.com"
        ));
        // A prefix that is no boundary is no base.
        assert!(!under_base(
            "https://api.github.com.example/1",
            "https://api.github.com"
        ));
    }
}
