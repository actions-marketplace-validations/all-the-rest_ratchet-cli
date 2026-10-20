//! The check: count against baseline, plus the monotonicity of the baseline.

use crate::baseline::Baseline;
use crate::config::{Config, Direction, Rule, Tier};
use crate::counter;
use crate::git::Git;

/// The status of one rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleStatus {
    /// The count respects the baseline.
    Ok,
    /// The count is above a ceiling baseline.
    OverBaseline,
    /// The count is below a floor baseline.
    UnderFloor,
    /// A ceiling baseline was increased against the committed one.
    BaselineRaised,
    /// A floor baseline was decreased against the committed one.
    BaselineLowered,
    /// The rule has no baseline.
    MissingBaseline,
    /// The counter did not deliver a usable count.
    CounterError,
}

impl RuleStatus {
    /// The name used in the JSON report.
    pub fn as_str(self) -> &'static str {
        match self {
            RuleStatus::Ok => "ok",
            RuleStatus::OverBaseline => "over-baseline",
            RuleStatus::UnderFloor => "under-floor",
            RuleStatus::BaselineRaised => "baseline-raised",
            RuleStatus::BaselineLowered => "baseline-lowered",
            RuleStatus::MissingBaseline => "missing-baseline",
            RuleStatus::CounterError => "counter-error",
        }
    }

    /// True when the rule failed.
    pub fn is_failure(self) -> bool {
        !matches!(self, RuleStatus::Ok)
    }
}

/// The outcome of one rule.
#[derive(Debug, Clone)]
pub struct RuleResult {
    /// The rule id.
    pub id: String,
    /// The status.
    pub status: RuleStatus,
    /// The current count, if the counter delivered one.
    pub count: Option<i64>,
    /// The baseline the count was compared against.
    pub baseline: Option<i64>,
    /// The direction of the rule.
    pub direction: Option<Direction>,
    /// The count unit.
    pub unit: Option<String>,
    /// The defect class.
    pub class: Option<String>,
    /// The tier of the rule.
    pub tier: Option<Tier>,
    /// A human readable explanation of the status.
    pub detail: Option<String>,
    /// The committed baseline, for the monotonicity statuses.
    pub committed: Option<i64>,
}

impl RuleResult {
    /// A result for a rule that could not be counted.
    ///
    /// The metadata of the rule is known even when the counter fails, so it is
    /// reported (SPEC §6.3).
    fn counter_error(id: &str, rule: &Rule, message: String) -> Self {
        Self {
            id: id.to_string(),
            status: RuleStatus::CounterError,
            count: None,
            baseline: None,
            direction: Some(rule.direction),
            unit: Some(rule.unit.clone()),
            class: Some(rule.class.clone()),
            tier: Some(rule.tier),
            detail: Some(message),
            committed: None,
        }
    }
}

/// The result of a run.
#[derive(Debug, Clone)]
pub struct Report {
    /// One entry per evaluated rule, sorted by rule id.
    pub rules: Vec<RuleResult>,
    /// The number of rules that were executed.
    pub executed: usize,
    /// The number of rules whose counter failed.
    pub counter_errors: usize,
    /// Notes about what was skipped and why.
    pub notes: Vec<String>,
    /// The debug log (`--debug`).
    pub debug: Vec<String>,
    /// True when the run is green: no gate failed and no counter failed.
    pub ok: bool,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            executed: 0,
            counter_errors: 0,
            notes: Vec::new(),
            debug: Vec::new(),
            ok: true,
        }
    }
}

impl Report {
    /// The number of failed rules.
    pub fn failed(&self) -> usize {
        self.rules
            .iter()
            .filter(|rule| rule.status.is_failure())
            .count()
    }
}

/// Runs the gates: count every rule, compare with the baseline and check that
/// the baseline only moved in the declared direction.
pub fn check(
    config: &Config,
    baseline: &Baseline,
    committed: Option<&Baseline>,
    root: &std::path::Path,
    rule_filter: Option<&str>,
    paths: &[String],
) -> Report {
    let mut report = Report::default();

    for (id, rule) in &config.rules {
        if let Some(filter) = rule_filter {
            if filter != id {
                continue;
            }
        }
        if !rule.in_scope(paths) {
            report.debug.push(format!(
                "rule {id} is out of scope for the selected paths — skipped"
            ));
            continue;
        }
        report.executed += 1;

        let run = match counter::run_counter(&rule.count, root) {
            Ok(run) => run,
            Err(error) => {
                push_counter_error(&mut report, id, rule, error);
                continue;
            }
        };
        report.debug.push(format!(
            "counter {id}: {}\n{}\n--- count ---",
            rule.count.describe(),
            run.output
        ));

        let count = match counter::finalize(
            run.value,
            &rule.exclude_globs,
            &format!("rules.{id}.exclude"),
        ) {
            Ok(count) => count,
            Err(error) => {
                push_counter_error(&mut report, id, rule, error);
                continue;
            }
        };
        report.debug.push(format!("counter {id}: count {count}"));

        let record = baseline.get(id);
        let count_status = match record {
            None => RuleStatus::MissingBaseline,
            Some(record) => match rule.direction {
                Direction::Ceiling if count > record.value => RuleStatus::OverBaseline,
                Direction::Floor if count < record.value => RuleStatus::UnderFloor,
                _ => RuleStatus::Ok,
            },
        };
        let mut status = count_status;
        let mut detail = match (count_status, record) {
            (RuleStatus::MissingBaseline, _) => Some(format!("no baseline for {id}")),
            (RuleStatus::OverBaseline, Some(record)) => {
                Some(format!("count {count} > baseline {}", record.value))
            }
            (RuleStatus::UnderFloor, Some(record)) => {
                Some(format!("count {count} < baseline {}", record.value))
            }
            _ => None,
        };
        let mut committed_value = None;

        // Monotonicity: the baseline may only move in the declared direction.
        if let (Some(record), Some(committed)) = (record, committed) {
            if let Some(committed_record) = committed.get(id) {
                committed_value = Some(committed_record.value);
                let violation = match rule.direction {
                    Direction::Ceiling if record.value > committed_record.value => Some((
                        RuleStatus::BaselineRaised,
                        format!(
                            "baseline {} raised to {} — a ceiling may only decrease",
                            committed_record.value, record.value
                        ),
                    )),
                    Direction::Floor if record.value < committed_record.value => Some((
                        RuleStatus::BaselineLowered,
                        format!(
                            "baseline {} lowered to {} — a floor may only increase",
                            committed_record.value, record.value
                        ),
                    )),
                    _ => None,
                };
                if let Some((violation, explanation)) = violation {
                    // The monotonicity check takes precedence; the count
                    // comparison is reported as an additional note (SPEC 5.2).
                    if count_status.is_failure() && count_status != RuleStatus::MissingBaseline {
                        report.notes.push(format!(
                            "{id}: the count {count} also fails against the baseline {} ({})",
                            record.value,
                            count_status.as_str()
                        ));
                    }
                    status = violation;
                    detail = Some(explanation);
                }
            }
        }

        report.rules.push(RuleResult {
            id: id.clone(),
            status,
            count: Some(count),
            baseline: record.map(|record| record.value),
            direction: Some(rule.direction),
            unit: Some(rule.unit.clone()),
            class: Some(rule.class.clone()),
            tier: Some(rule.tier),
            detail,
            committed: committed_value,
        });
    }

    // An empty selection is never a pass.
    report.ok = report.failed() == 0 && report.counter_errors == 0 && report.executed > 0;
    report
}

/// Records a counter that did not deliver a usable count.
fn push_counter_error(report: &mut Report, id: &str, rule: &Rule, error: counter::CounterError) {
    report.counter_errors += 1;
    // The raw output of the subprocess belongs to the diagnostic channel
    // (SPEC 6.2); the report carries ratchet's own sentence.
    report.debug.push(format!(
        "counter {id}: {}\n{}",
        rule.count.describe(),
        error.output.clone().unwrap_or_default()
    ));
    report
        .rules
        .push(RuleResult::counter_error(id, rule, error.message));
}

/// The note about the skipped monotonicity check.
pub fn monotonicity_note(
    git: &Git,
    baseline_rel: &str,
    committed: Option<&Baseline>,
) -> Option<String> {
    if committed.is_some() {
        return None;
    }
    if git.is_repo() {
        Some(format!(
            "monotonicity skipped: {baseline_rel} is not committed yet"
        ))
    } else {
        Some("monotonicity skipped: not a git repository".to_string())
    }
}

/// The selected paths, relative to the repository root.
pub fn selected_paths(
    paths: &[String],
    cwd: &std::path::Path,
    root: &std::path::Path,
) -> Vec<String> {
    paths
        .iter()
        .map(|path| {
            let path = std::path::Path::new(path);
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            match crate::path::relative_to(root, &absolute) {
                Some(relative) => relative,
                None => path.to_string_lossy().replace('\\', "/"),
            }
        })
        .collect()
}

/// Builds the report of a `count` run: the counts without a comparison.
pub fn counts(config: &Config, root: &std::path::Path, rule_filter: Option<&str>) -> Report {
    let mut report = Report::default();
    for (id, rule) in &config.rules {
        if let Some(filter) = rule_filter {
            if filter != id {
                continue;
            }
        }
        report.executed += 1;
        let run = match counter::run_counter(&rule.count, root) {
            Ok(run) => run,
            Err(error) => {
                push_counter_error(&mut report, id, rule, error);
                continue;
            }
        };
        report.debug.push(format!(
            "counter {id}: {}\n{}\n--- count ---",
            rule.count.describe(),
            run.output
        ));
        let count = match counter::finalize(
            run.value,
            &rule.exclude_globs,
            &format!("rules.{id}.exclude"),
        ) {
            Ok(count) => count,
            Err(error) => {
                push_counter_error(&mut report, id, rule, error);
                continue;
            }
        };
        report.debug.push(format!("counter {id}: count {count}"));
        report.rules.push(RuleResult {
            id: id.clone(),
            status: RuleStatus::Ok,
            count: Some(count),
            baseline: None,
            direction: Some(rule.direction),
            unit: Some(rule.unit.clone()),
            class: Some(rule.class.clone()),
            tier: Some(rule.tier),
            detail: None,
            committed: None,
        });
    }
    report.ok = report.failed() == 0 && report.counter_errors == 0;
    report
}

/// The result of a `baseline init` or `baseline update` run.
#[derive(Debug, Clone)]
pub struct BaselineAction {
    /// The rule id.
    pub id: String,
    /// What happened.
    pub kind: ActionKind,
    /// The baseline value, if the rule has one.
    pub value: Option<i64>,
    /// A human readable message.
    pub message: String,
}

/// The kind of a baseline action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// A baseline was written for a rule that had none.
    Added,
    /// An existing baseline was left untouched.
    Kept,
    /// An existing baseline was moved towards the current count.
    Updated,
    /// The baseline was not allowed to move.
    Refused,
}

impl ActionKind {
    /// The name used in the JSON report.
    pub fn as_str(self) -> &'static str {
        match self {
            ActionKind::Added => "added",
            ActionKind::Kept => "kept",
            ActionKind::Updated => "updated",
            ActionKind::Refused => "refused",
        }
    }
}
