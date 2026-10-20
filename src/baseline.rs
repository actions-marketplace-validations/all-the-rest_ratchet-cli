//! The baseline file: machine written, one record per rule.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The baseline file format version.
pub const BASELINE_VERSION: u32 = 1;

/// One baseline record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineRecord {
    /// The committed number the count is compared against.
    pub value: i64,
    /// Why the baseline has this value; only present when one was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The baseline file: `{"version": 1, "rules": { … }}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    /// The format version.
    pub version: u32,
    /// The records, sorted by rule id.
    pub rules: BTreeMap<String, BaselineRecord>,
}

/// A baseline that could not be read.
#[derive(Debug)]
pub struct BaselineError {
    /// The message shown to the user.
    pub message: String,
}

impl std::fmt::Display for BaselineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BaselineError {}

impl Baseline {
    /// An empty baseline.
    pub fn new() -> Self {
        Self {
            version: BASELINE_VERSION,
            rules: BTreeMap::new(),
        }
    }

    /// Reads the baseline file; a missing file is an error.
    ///
    /// `root` is the repository root: the paths in the error messages are
    /// relative to it, never absolute.
    pub fn load(root: Option<&Path>, path: &Path) -> Result<Self, BaselineError> {
        let label = crate::path::display(root, path);
        let text = fs::read_to_string(path).map_err(|e| BaselineError {
            message: format!("cannot read {label}: {e}"),
        })?;
        Self::parse(&text, &label)
    }

    /// Reads the baseline file; a missing file yields an empty baseline.
    pub fn load_if_exists(root: Option<&Path>, path: &Path) -> Result<Self, BaselineError> {
        if !path.exists() {
            return Ok(Self::new());
        }
        Self::load(root, path)
    }

    /// Parses a baseline document; `label` names the document in errors.
    pub fn parse(text: &str, label: &str) -> Result<Self, BaselineError> {
        let baseline: Baseline = serde_json::from_str(text).map_err(|e| BaselineError {
            message: format!("{label} is not a valid baseline: {e}"),
        })?;
        if baseline.version != BASELINE_VERSION {
            return Err(BaselineError {
                message: format!(
                    "{label} has version {} — expected version {BASELINE_VERSION}",
                    baseline.version
                ),
            });
        }
        Ok(baseline)
    }

    /// Writes the baseline file. Rules are sorted by id, the file is a stable diff.
    pub fn save(&self, root: Option<&Path>, path: &Path) -> Result<(), BaselineError> {
        let label = crate::path::display(root, path);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| BaselineError {
                    message: format!("cannot create the baseline directory: {e}"),
                })?;
            }
        }
        let mut text = serde_json::to_string_pretty(self).map_err(|e| BaselineError {
            message: format!("cannot serialize the baseline: {e}"),
        })?;
        text.push('\n');
        fs::write(path, text).map_err(|e| BaselineError {
            message: format!("cannot write {label}: {e}"),
        })
    }

    /// The baseline of a rule, if there is one.
    pub fn get(&self, rule: &str) -> Option<&BaselineRecord> {
        self.rules.get(rule)
    }
}

impl Default for Baseline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_sorted_keys() {
        let text = r#"{
          "version": 1,
          "rules": {
            "b": { "value": 2 },
            "a": { "value": 1, "reason": "because" }
          }
        }"#;
        let baseline = Baseline::parse(text, "ratchet.baseline.json").expect("parses");
        assert_eq!(baseline.version, 1);
        assert_eq!(baseline.get("a").unwrap().value, 1);
        assert_eq!(
            baseline.get("a").unwrap().reason.as_deref(),
            Some("because")
        );
        assert!(baseline.get("b").unwrap().reason.is_none());

        let written = serde_json::to_string_pretty(&baseline).expect("serializes");
        let a = written.find("\"a\"").expect("a is present");
        let b = written.find("\"b\"").expect("b is present");
        assert!(a < b, "rules are sorted by id");
        assert_eq!(
            written.matches("\"reason\"").count(),
            1,
            "a record without a reason does not write one"
        );
    }

    #[test]
    fn rejects_an_unknown_version() {
        let error = Baseline::parse(r#"{"version": 2, "rules": {}}"#, "b.json")
            .expect_err("the version is checked");
        assert!(error.message.contains("version 2"));
    }

    #[test]
    fn rejects_an_unknown_field() {
        assert!(
            Baseline::parse(r#"{"version": 1, "rules": {}, "extra": true}"#, "b.json").is_err()
        );
    }
}
