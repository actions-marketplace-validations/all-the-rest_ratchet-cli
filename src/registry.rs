//! The findings registry: a JSON file that turns a list of findings into a
//! number a gate can hold.
//!
//! The registry is a **counter source** (SPEC 13): the engine reads the file,
//! filters the entries by the levels and the statuses the rule names, and
//! counts what is left.  The mechanic is the one of every other counter — the
//! baseline, the direction and the monotonicity are unchanged.
//!
//! The vocabulary is the canonical one of the `ratchet-gates` skill: a level
//! or a status outside it is an **error**, never a fallback.  A registry that
//! cannot be read is an error too: it is never a gate that reports zero
//! findings.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{Map, Value};

/// The canonical severity levels.
pub const SEVERITIES: [&str; 5] = ["critical", "high", "medium", "low", "info"];

/// The canonical statuses.
pub const STATUSES: [&str; 5] = ["open", "in-progress", "fixed", "accepted", "wontfix"];

/// The statuses that count when a rule does not name them.
///
/// `open` and `in-progress` are the findings that are not addressed yet;
/// `accepted` and `wontfix` are decisions and `fixed` is resolved.
pub const COUNTING_STATUSES: [&str; 2] = ["open", "in-progress"];

/// The fields of a finding.
const FINDING_FIELDS: [&str; 10] = [
    "id",
    "title",
    "severity",
    "status",
    "status_since",
    "owner",
    "ticket",
    "regression_test",
    "path",
    "note",
];

/// The severity of a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Breaks a security boundary, loses data, or blocks the core path.
    Critical,
    /// A security boundary or a core path is degraded.
    High,
    /// A real defect with a workaround.
    Medium,
    /// A nitpick or an improvement.
    Low,
    /// An observation, nothing to fix.
    Info,
}

impl Severity {
    /// The level of a name; `None` when the name is not canonical.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "critical" => Some(Severity::Critical),
            "high" => Some(Severity::High),
            "medium" => Some(Severity::Medium),
            "low" => Some(Severity::Low),
            "info" => Some(Severity::Info),
            _ => None,
        }
    }

    /// The name used in configs and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Critical => "critical",
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
            Severity::Info => "info",
        }
    }
}

/// The status of the handling of a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    /// Not addressed yet.
    #[serde(rename = "open")]
    Open,
    /// Being worked on, not fixed yet.
    #[serde(rename = "in-progress")]
    InProgress,
    /// Addressed and independently verified.
    #[serde(rename = "fixed")]
    Fixed,
    /// Consciously accepted, with an owner decision.
    #[serde(rename = "accepted")]
    Accepted,
    /// Consciously not fixed — by design, not a defect.
    #[serde(rename = "wontfix")]
    Wontfix,
}

impl Status {
    /// The status of a name; `None` when the name is not canonical.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "open" => Some(Status::Open),
            "in-progress" => Some(Status::InProgress),
            "fixed" => Some(Status::Fixed),
            "accepted" => Some(Status::Accepted),
            "wontfix" => Some(Status::Wontfix),
            _ => None,
        }
    }

    /// The name used in configs and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::InProgress => "in-progress",
            Status::Fixed => "fixed",
            Status::Accepted => "accepted",
            Status::Wontfix => "wontfix",
        }
    }

    /// True when a finding in this status counts without a rule that names
    /// the statuses.
    pub fn counts(self) -> bool {
        COUNTING_STATUSES.contains(&self.as_str())
    }
}

/// One finding of the registry.
#[derive(Debug, Clone)]
pub struct Finding {
    /// The stable id of the finding.
    pub id: String,
    /// The one-line title.
    pub title: String,
    /// The severity level.
    pub severity: Severity,
    /// The status of the handling.
    pub status: Status,
    /// The date the status was reached.
    pub status_since: Option<String>,
    /// Who owns the finding.
    pub owner: Option<String>,
    /// The decision, the review round or the issue that tracks the finding.
    pub ticket: Option<String>,
    /// The test that proves the fix.
    pub regression_test: Option<String>,
    /// Where the defect lives.
    pub path: Option<String>,
    /// Free text.
    pub note: Option<String>,
}

/// The registry: the findings of a project.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    /// The findings, in the order of the file.
    pub findings: Vec<Finding>,
}

/// A registry that does not hold what it must hold.
///
/// The message names the offending entry: its `id`, or its position when it
/// has none.
#[derive(Debug, Clone)]
pub struct RegistryError {
    /// What is wrong with the registry.
    pub message: String,
}

impl RegistryError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Registry {
    /// Reads a registry and checks it against the vocabulary.
    ///
    /// `name` is the name of the file as it is shown in the messages: the
    /// report never carries an absolute path.
    pub fn parse(text: &str, name: &str) -> Result<Registry, RegistryError> {
        let document: Value = serde_json::from_str(text)
            .map_err(|e| RegistryError::new(format!("{name} is not valid JSON: {e}")))?;
        let map = document.as_object().ok_or_else(|| {
            RegistryError::new(format!(
                "{name}: expected an object, found {}",
                type_name(&document)
            ))
        })?;

        // The version is the schema of the file: a file of another version is
        // not read with a guess.
        match map.get("version") {
            None => return Err(RegistryError::new(format!("{name}: has no \"version\""))),
            Some(value) => {
                let version = value.as_i64().ok_or_else(|| {
                    RegistryError::new(format!("{name}: the version is not an integer"))
                })?;
                if version != 1 {
                    return Err(RegistryError::new(format!(
                        "{name}: the version is {version} — expected 1"
                    )));
                }
            }
        }
        for key in map.keys() {
            if key != "version" && key != "findings" {
                return Err(RegistryError::new(format!(
                    "{name}: has the unknown field \"{key}\""
                )));
            }
        }

        let items = map
            .get("findings")
            .ok_or_else(|| RegistryError::new(format!("{name}: has no \"findings\"")))?;
        let items = items
            .as_array()
            .ok_or_else(|| RegistryError::new(format!("{name}: the findings are not an array")))?;

        let mut findings = Vec::with_capacity(items.len());
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for (index, item) in items.iter().enumerate() {
            let finding = parse_finding(item, index, name)?;
            if !ids.insert(finding.id.clone()) {
                return Err(RegistryError::new(format!(
                    "{name}: the id \"{}\" is used twice — every id is unique",
                    finding.id
                )));
            }
            findings.push(finding);
        }

        Ok(Registry { findings })
    }

    /// The findings that count.
    ///
    /// `severities` names the levels that count; without it every level
    /// counts.  `statuses` names the statuses that count; without it the
    /// counting statuses — `open` and `in-progress` — count.
    ///
    /// The list is what the count is made of: the report shows **these**
    /// findings, not a second reading of the file (SPEC 14).  A report that
    /// counts the findings again could tell a different story than the number
    /// it claims to report.
    pub fn selecting(
        &self,
        severities: Option<&[Severity]>,
        statuses: Option<&[Status]>,
    ) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|finding| match severities {
                None => true,
                Some(levels) => levels.contains(&finding.severity),
            })
            .filter(|finding| match statuses {
                None => finding.status.counts(),
                Some(list) => list.contains(&finding.status),
            })
            .collect()
    }

    /// The number of the findings that count.
    pub fn count(&self, severities: Option<&[Severity]>, statuses: Option<&[Status]>) -> i64 {
        self.selecting(severities, statuses).len() as i64
    }
}

/// Reads one finding of the registry.
fn parse_finding(value: &Value, index: usize, name: &str) -> Result<Finding, RegistryError> {
    let map = value
        .as_object()
        .ok_or_else(|| RegistryError::new(format!("{name}: findings[{index}] is not an object")))?;

    // The id names the entry in every message.  A finding without one is
    // named by its position.
    let id = match map.get("id") {
        None => {
            return Err(RegistryError::new(format!(
                "{name}: findings[{index}] has no \"id\""
            )));
        }
        Some(value) => {
            let id = expect_string(value, name, &format!("findings[{index}]"), "id")?;
            if id.trim().is_empty() {
                return Err(RegistryError::new(format!(
                    "{name}: findings[{index}] has an empty \"id\""
                )));
            }
            id
        }
    };
    let entry = id.clone();

    for key in map.keys() {
        if !FINDING_FIELDS.contains(&key.as_str()) {
            return Err(RegistryError::new(format!(
                "{name}: {entry} has the unknown field \"{key}\""
            )));
        }
    }

    let title = require_string(map, "title", name, &entry)?;
    let severity = require_string(map, "severity", name, &entry)?;
    let severity = Severity::parse(&severity).ok_or_else(|| {
        RegistryError::new(format!(
            "{name}: {entry} has the unknown severity \"{severity}\" — expected one of {}",
            SEVERITIES.join(", ")
        ))
    })?;
    let status = require_string(map, "status", name, &entry)?;
    let status = Status::parse(&status).ok_or_else(|| {
        RegistryError::new(format!(
            "{name}: {entry} has the unknown status \"{status}\" — expected one of {}",
            STATUSES.join(", ")
        ))
    })?;

    // An open finding has nobody: a graveyard entry.  A closed one has its
    // commit and its regression test.  An empty string is a missing value
    // that happens to pass the type check.
    let owner = blank_as_none(optional_string(map, "owner", name, &entry)?);
    let ticket = blank_as_none(optional_string(map, "ticket", name, &entry)?);
    if status != Status::Fixed {
        if owner.is_none() {
            return Err(RegistryError::new(format!(
                "{name}: {entry} has no \"owner\" — a finding that is not fixed needs an owner and a ticket"
            )));
        }
        if ticket.is_none() {
            return Err(RegistryError::new(format!(
                "{name}: {entry} has no \"ticket\" — a finding that is not fixed needs an owner and a ticket"
            )));
        }
    }

    Ok(Finding {
        id,
        title,
        severity,
        status,
        status_since: optional_string(map, "status_since", name, &entry)?,
        owner,
        ticket,
        regression_test: optional_string(map, "regression_test", name, &entry)?,
        path: optional_string(map, "path", name, &entry)?,
        note: optional_string(map, "note", name, &entry)?,
    })
}

/// A required field of a finding: a non-empty string.
fn require_string(
    map: &Map<String, Value>,
    field: &str,
    name: &str,
    entry: &str,
) -> Result<String, RegistryError> {
    let value = map
        .get(field)
        .ok_or_else(|| RegistryError::new(format!("{name}: {entry} has no \"{field}\"")))?;
    let text = expect_string(value, name, entry, field)?;
    if text.trim().is_empty() {
        return Err(RegistryError::new(format!(
            "{name}: {entry} has an empty \"{field}\""
        )));
    }
    Ok(text)
}

/// An optional field of a finding: a string when it is there.
fn optional_string(
    map: &Map<String, Value>,
    field: &str,
    name: &str,
    entry: &str,
) -> Result<Option<String>, RegistryError> {
    match map.get(field) {
        None => Ok(None),
        Some(value) => Ok(Some(expect_string(value, name, entry, field)?)),
    }
}

/// An empty string is a missing value that happens to pass the type check.
fn blank_as_none(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

/// A field of a finding that has to be a string.
fn expect_string(
    value: &Value,
    name: &str,
    entry: &str,
    field: &str,
) -> Result<String, RegistryError> {
    value.as_str().map(str::to_string).ok_or_else(|| {
        RegistryError::new(format!(
            "{name}: {entry} has a \"{field}\" that is not a string, found {}",
            type_name(value)
        ))
    })
}

/// The name of a JSON type, for the messages.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registry with one finding of every level and every status.
    const MIXED: &str = r#"{
      "version": 1,
      "findings": [
        { "id": "SEC-01", "title": "a critical one", "severity": "critical", "status": "open",
          "owner": "alice", "ticket": "D-1" },
        { "id": "SEC-02", "title": "a high one", "severity": "high", "status": "in-progress",
          "owner": "alice", "ticket": "D-2" },
        { "id": "SEC-03", "title": "a medium one", "severity": "medium", "status": "open",
          "owner": "alice", "ticket": "D-3" },
        { "id": "SEC-04", "title": "a low one", "severity": "low", "status": "accepted",
          "owner": "alice", "ticket": "D-4" },
        { "id": "SEC-05", "title": "an info one", "severity": "info", "status": "wontfix",
          "owner": "alice", "ticket": "D-5" },
        { "id": "SEC-06", "title": "a fixed one", "severity": "critical", "status": "fixed" }
      ]
    }"#;

    fn registry(text: &str) -> Registry {
        Registry::parse(text, "ratchet.findings.json").expect("the registry parses")
    }

    fn error(text: &str) -> String {
        Registry::parse(text, "ratchet.findings.json")
            .expect_err("the registry is broken")
            .message
    }

    #[test]
    fn counts_the_findings_that_count() {
        let registry = registry(MIXED);
        // Without a rule: every level, every counting status.
        assert_eq!(registry.count(None, None), 3);
    }

    #[test]
    fn counts_the_levels_the_rule_names() {
        let registry = registry(MIXED);
        let levels = [Severity::Critical, Severity::High];
        assert_eq!(registry.count(Some(&levels), None), 2);
    }

    #[test]
    fn counts_the_statuses_the_rule_names() {
        let registry = registry(MIXED);
        let statuses = [Status::Open];
        assert_eq!(registry.count(None, Some(&statuses)), 2);
    }

    #[test]
    fn a_decision_does_not_count() {
        let registry = registry(MIXED);
        let statuses = [Status::Open, Status::InProgress];
        assert_eq!(registry.count(None, Some(&statuses)), 3);
    }

    #[test]
    fn rejects_an_unknown_version() {
        let message = error(r#"{ "version": 2, "findings": [] }"#);
        assert!(message.contains("the version is 2"), "{message}");
    }

    #[test]
    fn rejects_a_missing_version() {
        let message = error(r#"{ "findings": [] }"#);
        assert!(message.contains("has no \"version\""), "{message}");
    }

    #[test]
    fn rejects_a_missing_field() {
        let message = error(
            r#"{ "version": 1, "findings": [ { "id": "SEC-01", "severity": "low", "status": "open",
                 "owner": "a", "ticket": "b" } ] }"#,
        );
        assert!(message.contains("SEC-01"), "{message}");
        assert!(message.contains("has no \"title\""), "{message}");
    }

    #[test]
    fn names_a_finding_without_an_id_by_its_position() {
        let message = error(
            r#"{ "version": 1, "findings": [ { "title": "a", "severity": "low", "status": "open",
                 "owner": "a", "ticket": "b" } ] }"#,
        );
        assert!(message.contains("findings[0]"), "{message}");
    }

    #[test]
    fn rejects_a_duplicate_id() {
        let message = error(
            r#"{ "version": 1, "findings": [
                 { "id": "SEC-01", "title": "a", "severity": "low", "status": "fixed" },
                 { "id": "SEC-01", "title": "b", "severity": "low", "status": "fixed" } ] }"#,
        );
        assert!(message.contains("SEC-01"), "{message}");
        assert!(message.contains("used twice"), "{message}");
    }

    #[test]
    fn rejects_an_unknown_severity() {
        let message = error(
            r#"{ "version": 1, "findings": [ { "id": "SEC-01", "title": "a", "severity": "blocker",
                 "status": "fixed" } ] }"#,
        );
        assert!(message.contains("SEC-01"), "{message}");
        assert!(
            message.contains("unknown severity \"blocker\""),
            "{message}"
        );
    }

    #[test]
    fn rejects_an_unknown_status() {
        let message = error(
            r#"{ "version": 1, "findings": [ { "id": "SEC-01", "title": "a", "severity": "low",
                 "status": "doing" } ] }"#,
        );
        assert!(message.contains("SEC-01"), "{message}");
        assert!(message.contains("unknown status \"doing\""), "{message}");
    }

    #[test]
    fn rejects_a_missing_owner() {
        let message = error(
            r#"{ "version": 1, "findings": [ { "id": "SEC-01", "title": "a", "severity": "low",
                 "status": "open", "ticket": "D-1" } ] }"#,
        );
        assert!(message.contains("SEC-01"), "{message}");
        assert!(message.contains("has no \"owner\""), "{message}");
    }

    #[test]
    fn rejects_a_missing_ticket() {
        let message = error(
            r#"{ "version": 1, "findings": [ { "id": "SEC-01", "title": "a", "severity": "low",
                 "status": "open", "owner": "alice" } ] }"#,
        );
        assert!(message.contains("SEC-01"), "{message}");
        assert!(message.contains("has no \"ticket\""), "{message}");
    }

    #[test]
    fn a_fixed_finding_needs_neither_an_owner_nor_a_ticket() {
        let registry = registry(
            r#"{ "version": 1, "findings": [ { "id": "SEC-01", "title": "a", "severity": "low",
                 "status": "fixed" } ] }"#,
        );
        assert_eq!(registry.findings.len(), 1);
    }

    #[test]
    fn rejects_an_unknown_field() {
        let message = error(
            r#"{ "version": 1, "findings": [ { "id": "SEC-01", "title": "a", "sevrity": "low",
                 "status": "fixed" } ] }"#,
        );
        assert!(message.contains("SEC-01"), "{message}");
        assert!(message.contains("unknown field \"sevrity\""), "{message}");
    }

    #[test]
    fn rejects_a_document_that_is_not_json() {
        let message = error("not json");
        assert!(message.contains("is not valid JSON"), "{message}");
    }

    #[test]
    fn parses_the_optional_fields() {
        let registry = registry(
            r#"{ "version": 1, "findings": [ { "id": "SEC-01", "title": "a", "severity": "low",
                 "status": "fixed", "status_since": "2026-10-19", "regression_test": "T",
                 "path": "src/a.rs", "note": "a note" } ] }"#,
        );
        let finding = &registry.findings[0];
        assert_eq!(finding.status_since.as_deref(), Some("2026-10-19"));
        assert_eq!(finding.regression_test.as_deref(), Some("T"));
        assert_eq!(finding.path.as_deref(), Some("src/a.rs"));
        assert_eq!(finding.note.as_deref(), Some("a note"));
    }
}
