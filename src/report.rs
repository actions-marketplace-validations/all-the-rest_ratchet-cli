//! The output formats: `stylish` (human readable) and `json` (machine readable).

use serde::Serialize;

use crate::check::{BaselineAction, Report, RuleResult, RuleStatus};
use crate::config::Direction;

/// The JSON schema version of the report.
pub const SCHEMA: u32 = 1;

/// Renders the human readable report.
pub fn render_stylish(report: &Report, quiet: bool) -> String {
    let mut out = String::new();
    let failed = report.failed();

    if !quiet {
        out.push_str(&format!(
            "ratchet — {}, {failed} failed\n\n",
            plural(report.rules.len(), "rule")
        ));
    }

    let width = label_width(report);
    for rule in &report.rules {
        if quiet && !rule.status.is_failure() {
            continue;
        }
        let status = if rule.status.is_failure() {
            "FAIL"
        } else {
            "PASS"
        };
        out.push_str(&format!(
            "  {status}  {:<width$}  {}\n",
            label(rule),
            detail(rule),
            width = width
        ));
    }

    if !quiet {
        out.push('\n');
        if report.ok {
            out.push_str(&format!(
                "COHERENT — {} of {} gates passed\n",
                report.rules.len(),
                report.rules.len()
            ));
        } else {
            out.push_str(&format!(
                "NOT COHERENT — {} of {} gates failed\n",
                report.failed(),
                report.rules.len()
            ));
        }
    }

    out
}

/// Renders the human readable `count` output.
pub fn render_counts(report: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "ratchet — {} counted\n\n",
        plural(report.rules.len(), "rule")
    ));

    let width = label_width(report);
    for rule in &report.rules {
        let count = match (rule.count, &rule.detail) {
            (Some(count), _) => count.to_string(),
            (None, Some(detail)) => format!("failed — {detail}"),
            (None, None) => "failed".to_string(),
        };
        let unit = rule
            .unit
            .as_ref()
            .map(|unit| format!("  ({unit})"))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {:<width$}  {count}{unit}\n",
            rule.id,
            width = width
        ));
    }

    out
}

/// Renders the human readable output of the baseline commands.
pub fn render_baseline(command: &str, path: &str, actions: &[BaselineAction]) -> String {
    let mut out = String::new();
    out.push_str(&format!("ratchet — baseline {command}\n\n"));
    for action in actions {
        out.push_str(&format!("  {}: {}\n", action.id, action.message));
    }
    out.push_str(&format!("\n{path}\n"));
    out
}

/// Renders the machine readable report.
pub fn render_json(report: &Report) -> String {
    let json = JsonReport {
        schema: SCHEMA,
        ok: report.ok,
        summary: JsonSummary {
            rules: report.rules.len(),
            failed: report.failed(),
            executed: report.executed,
            generated: 0,
            drifted: 0,
        },
        rules: report.rules.iter().map(JsonRule::from).collect(),
        generated: Vec::new(),
    };
    serde_json::to_string_pretty(&json).unwrap_or_else(|_| "{}".to_string())
}

/// Renders the machine readable output of the baseline commands.
pub fn render_baseline_json(command: &str, actions: &[BaselineAction], failed: usize) -> String {
    let json = JsonBaselineReport {
        schema: SCHEMA,
        ok: failed == 0,
        summary: JsonSummary {
            rules: actions.len(),
            failed,
            executed: actions.len(),
            generated: 0,
            drifted: 0,
        },
        rules: actions.iter().map(JsonBaselineAction::from).collect(),
        generated: Vec::new(),
        baseline: JsonBaseline {
            command: command.to_string(),
        },
    };
    serde_json::to_string_pretty(&json).unwrap_or_else(|_| "{}".to_string())
}

/// Renders the machine readable output of `ratchet init`.
pub fn render_init_json(config: &str) -> String {
    let json = JsonInitReport {
        schema: SCHEMA,
        ok: true,
        summary: JsonSummary {
            rules: 0,
            failed: 0,
            executed: 0,
            generated: 0,
            drifted: 0,
        },
        rules: Vec::new(),
        generated: Vec::new(),
        config: config.to_string(),
    };
    serde_json::to_string_pretty(&json).unwrap_or_else(|_| "{}".to_string())
}

/// The label of a rule in the human readable output.
fn label(rule: &RuleResult) -> &str {
    if rule.status == RuleStatus::MissingBaseline {
        "missing-baseline"
    } else {
        &rule.id
    }
}

/// The width of the label column.
fn label_width(report: &Report) -> usize {
    report
        .rules
        .iter()
        .map(|rule| label(rule).chars().count())
        .max()
        .unwrap_or(0)
        + 6
}

/// The detail of a rule in the human readable output.
fn detail(rule: &RuleResult) -> String {
    match rule.status {
        RuleStatus::Ok | RuleStatus::OverBaseline | RuleStatus::UnderFloor => {
            let operator = match (rule.direction, rule.status) {
                (Some(Direction::Ceiling), RuleStatus::Ok) => "≤",
                (Some(Direction::Floor), RuleStatus::Ok) => "≥",
                (Some(Direction::Floor), _) => "<",
                _ => ">",
            };
            let unit = rule
                .unit
                .as_ref()
                .map(|unit| format!("  ({unit})"))
                .unwrap_or_default();
            format!(
                "{} {operator} {}{unit}",
                rule.count.unwrap_or_default(),
                rule.baseline.unwrap_or_default()
            )
        }
        RuleStatus::MissingBaseline => format!("no baseline for {}", rule.id),
        RuleStatus::BaselineRaised => format!(
            "baseline {} raised to {} — a ceiling may only decrease",
            rule.committed.unwrap_or_default(),
            rule.baseline.unwrap_or_default()
        ),
        RuleStatus::BaselineLowered => format!(
            "baseline {} lowered to {} — a floor may only increase",
            rule.committed.unwrap_or_default(),
            rule.baseline.unwrap_or_default()
        ),
        RuleStatus::CounterError => rule.detail.clone().unwrap_or_default(),
    }
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("{count} {word}")
    } else {
        format!("{count} {word}s")
    }
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema: u32,
    ok: bool,
    summary: JsonSummary,
    rules: Vec<JsonRule<'a>>,
    generated: Vec<serde_json::Value>,
}

#[derive(Serialize)]
struct JsonSummary {
    rules: usize,
    failed: usize,
    executed: usize,
    generated: usize,
    drifted: usize,
}

#[derive(Serialize)]
struct JsonRule<'a> {
    id: &'a str,
    status: &'a str,
    count: Option<i64>,
    baseline: Option<i64>,
    direction: Option<&'a str>,
    unit: Option<&'a str>,
    class: Option<&'a str>,
    tier: Option<&'a str>,
    /// The reason for a status that is not `ok` (SPEC 6.3).
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
}

impl<'a> From<&'a RuleResult> for JsonRule<'a> {
    fn from(rule: &'a RuleResult) -> Self {
        Self {
            id: &rule.id,
            status: rule.status.as_str(),
            count: rule.count,
            baseline: rule.baseline,
            direction: rule.direction.map(|direction| direction.as_str()),
            unit: rule.unit.as_deref(),
            class: rule.class.as_deref(),
            tier: rule.tier.map(|tier| tier.as_str()),
            detail: rule.detail.as_deref(),
        }
    }
}

#[derive(Serialize)]
struct JsonBaselineReport {
    schema: u32,
    ok: bool,
    summary: JsonSummary,
    rules: Vec<JsonBaselineAction>,
    generated: Vec<serde_json::Value>,
    baseline: JsonBaseline,
}

#[derive(Serialize)]
struct JsonBaseline {
    command: String,
}

#[derive(Serialize)]
struct JsonInitReport {
    schema: u32,
    ok: bool,
    summary: JsonSummary,
    rules: Vec<serde_json::Value>,
    generated: Vec<serde_json::Value>,
    config: String,
}

#[derive(Serialize)]
struct JsonBaselineAction {
    id: String,
    action: &'static str,
    value: Option<i64>,
    message: String,
}

impl From<&BaselineAction> for JsonBaselineAction {
    fn from(action: &BaselineAction) -> Self {
        Self {
            id: action.id.clone(),
            action: action.kind.as_str(),
            value: action.value,
            message: action.message.clone(),
        }
    }
}
