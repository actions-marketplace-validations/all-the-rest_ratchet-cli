//! The output formats: `stylish` (human readable) and `json` (machine readable).

use serde::Serialize;

use crate::check::{BaselineAction, Report, RuleResult, RuleStatus};
use crate::config::Direction;
use crate::drift::DriftEntry;

/// The JSON schema version of the report.
pub const SCHEMA: u32 = 1;

/// Renders the human readable report.
pub fn render_stylish(report: &Report, quiet: bool) -> String {
    let mut out = String::new();
    let failed = report.failed();
    let drifted = report.drifted();

    if !quiet {
        out.push_str(&format!(
            "ratchet — {}{}, {} failed\n\n",
            plural(report.rules.len(), "rule"),
            generated_suffix(report),
            failed + drifted
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

    let generated: Vec<&DriftEntry> = report
        .generated
        .iter()
        .filter(|entry| !quiet || entry.status.is_failure())
        .collect();
    if !generated.is_empty() {
        out.push_str("\ngenerated files:\n\n");
        for entry in generated {
            let status = if entry.status.is_failure() {
                "FAIL"
            } else {
                "PASS"
            };
            out.push_str(&line(&format!(
                "  {status}  {:<width$}  {}",
                entry.path,
                entry.detail.clone().unwrap_or_default(),
                width = width
            )));
            if let Some(diff) = &entry.diff {
                // The diff is a unified diff of its own: its lines start with
                // a sign, and indenting them would change their meaning.
                out.push('\n');
                out.push_str(diff);
            }
        }
    }

    if !quiet {
        out.push('\n');
        if report.ok {
            out.push_str(&format!(
                "COHERENT — {} of {} gates passed\n",
                report.gates(),
                report.gates()
            ));
        } else {
            out.push_str(&format!(
                "NOT COHERENT — {} of {} gates failed\n",
                failed + drifted,
                report.gates()
            ));
        }
    }

    out
}

/// The part of the headline that names the generated files.
fn generated_suffix(report: &Report) -> String {
    if report.generated.is_empty() {
        return String::new();
    }
    format!(", {}", plural(report.generated.len(), "generated file"))
}

/// One line of the report, without the padding of an empty detail.
///
/// The label column is padded, and a row without a detail would end in
/// spaces — the report carries words, not padding.
fn line(text: &str) -> String {
    format!("{}\n", text.trim_end())
}

/// Renders the human readable output of the drift command.
///
/// The drift command runs the generated files only: there are no rules in its
/// report, and the headline names the generated files alone.
///
/// The verdict is the one the run decided on — the same one the exit code and
/// the JSON report carry.  The line must never say COHERENT while the run
/// fails.
pub fn render_drift(entries: &[DriftEntry], ok: bool, quiet: bool) -> String {
    let mut out = String::new();
    let drifted = entries
        .iter()
        .filter(|entry| entry.status.is_failure())
        .count();

    if !quiet {
        out.push_str(&format!(
            "ratchet — {}, {drifted} failed\n\n",
            plural(entries.len(), "generated file")
        ));
    }

    let width = entries
        .iter()
        .map(|entry| entry.path.chars().count())
        .max()
        .unwrap_or(0)
        + 6;
    let shown: Vec<&DriftEntry> = entries
        .iter()
        .filter(|entry| !quiet || entry.status.is_failure())
        .collect();
    for entry in shown {
        let status = if entry.status.is_failure() {
            "FAIL"
        } else {
            "PASS"
        };
        out.push_str(&line(&format!(
            "  {status}  {:<width$}  {}",
            entry.path,
            entry.detail.clone().unwrap_or_default(),
            width = width
        )));
        if let Some(diff) = &entry.diff {
            out.push('\n');
            out.push_str(diff);
        }
    }

    if !quiet {
        out.push('\n');
        if ok {
            out.push_str(&format!(
                "COHERENT — {} of {} gates passed\n",
                entries.len(),
                entries.len()
            ));
        } else {
            out.push_str(&format!(
                "NOT COHERENT — {drifted} of {} gates failed\n",
                entries.len()
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
            executed: report.checks(),
            generated: report.generated.len(),
            drifted: report.drifted(),
        },
        rules: report.rules.iter().map(JsonRule::from).collect(),
        generated: report.generated.iter().map(JsonGenerated::from).collect(),
        notes: &report.notes,
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
        notes: &[],
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
        notes: &[],
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
    let rules = report
        .rules
        .iter()
        .map(|rule| label(rule).chars().count())
        .max()
        .unwrap_or(0);
    let generated = report
        .generated
        .iter()
        .map(|entry| entry.path.chars().count())
        .max()
        .unwrap_or(0);
    rules.max(generated) + 6
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
    generated: Vec<JsonGenerated<'a>>,
    /// The notes the human output prints on stderr: the tier floor, the
    /// skipped monotonicity comparison, a skipped drift gate.  A machine
    /// consumer must be able to learn why the run is not green without the
    /// note being only on stderr (SPEC 6.3).
    notes: &'a [String],
}

#[derive(Serialize)]
struct JsonSummary {
    rules: usize,
    failed: usize,
    executed: usize,
    generated: usize,
    drifted: usize,
}

/// One generated file of the report.
#[derive(Serialize)]
struct JsonGenerated<'a> {
    path: &'a str,
    status: &'a str,
    /// The unified diff (committed → produced); absent when there is none.
    diff: Option<&'a str>,
    /// The reason for a status that is not `ok` (SPEC 6.3).
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
}

impl<'a> From<&'a DriftEntry> for JsonGenerated<'a> {
    fn from(entry: &'a DriftEntry) -> Self {
        Self {
            path: &entry.path,
            status: entry.status.as_str(),
            diff: entry.diff.as_deref(),
            detail: entry.detail.as_deref(),
        }
    }
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
    notes: &'static [String],
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
    notes: &'static [String],
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
