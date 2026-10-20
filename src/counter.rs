//! Counters: how the count of a rule is produced and how it is interpreted.
//!
//! The report carries ratchet's own words only: a counter that fails is
//! reported as *what* it was (which rule, which exit status), never as *what
//! it printed*.  The raw output of a counter belongs to the diagnostic
//! channel (`--debug`), which is exempt from the determinism rule (SPEC 6.2).

use std::fs;
use std::path::Path;
use std::process::Command;

use globset::GlobMatcher;
use serde_json::Value;

use crate::config::Counter;
use crate::generate::Generator;

/// The value a counter produced.
#[derive(Debug, Clone, Default)]
pub struct CounterValue {
    /// An explicit count, if the counter reported one.
    pub count: Option<i64>,
    /// The counted items, if the counter reported any.
    pub items: Option<Vec<String>>,
}

/// A counter that did not deliver a usable count.
#[derive(Debug, Clone)]
pub struct CounterError {
    /// Ratchet's own sentence; the report carries it.
    pub message: String,
    /// The raw output of the subprocess; the diagnostic channel carries it.
    pub output: Option<String>,
    /// True when the counter itself is misconfigured (not a runtime failure).
    pub is_config: bool,
}

impl CounterError {
    /// A failure of the counter at runtime.
    fn runtime(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            output: None,
            is_config: false,
        }
    }

    /// A failure of the counter at runtime, with the raw output.
    fn runtime_with(message: impl Into<String>, output: String) -> Self {
        Self {
            message: message.into(),
            output: Some(output),
            is_config: false,
        }
    }

    /// A misconfigured counter.
    fn config(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            output: None,
            is_config: true,
        }
    }
}

impl std::fmt::Display for CounterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// The result of a counter: the value and the raw output of the subprocess.
#[derive(Debug, Clone)]
pub struct CounterRun {
    /// The value the counter produced.
    pub value: CounterValue,
    /// The raw output of the subprocess; the diagnostic channel carries it.
    pub output: String,
}

/// Runs the counter and returns its value and the raw output.
pub fn run_counter(counter: &Counter, root: &Path) -> Result<CounterRun, CounterError> {
    match counter {
        Counter::Command { command } => run_command(command, root),
        Counter::File {
            file,
            path,
            generate,
            output_arg,
        } => {
            // A generator writes to a temporary path: `output_arg` is
            // mandatory (the config rejects a counter without it).
            let generator = match (generate, output_arg) {
                (Some(command), Some(output_arg)) => Some(Generator {
                    command: command.clone(),
                    output_arg: output_arg.clone(),
                }),
                (Some(_), None) => {
                    return Err(CounterError::config(
                        "config error at count.output_arg: is required by \"generate\"",
                    ));
                }
                (None, _) => None,
            };
            run_file(file, path.as_deref(), generator, root)
        }
    }
}

/// Applies `exclude` to the items and turns the value into the final count.
pub fn finalize(
    value: CounterValue,
    exclude: &[GlobMatcher],
    exclude_path: &str,
) -> Result<i64, CounterError> {
    let CounterValue { count, items } = value;

    if let Some(items) = items {
        let mut items = items;
        items.sort();
        if exclude.is_empty() {
            let count = count.unwrap_or(items.len() as i64);
            return non_negative(count);
        }
        let kept = items
            .iter()
            .filter(|item| !exclude.iter().any(|glob| glob.is_match(item)))
            .count() as i64;
        return non_negative(kept);
    }

    if !exclude.is_empty() {
        return Err(CounterError::config(format!(
            "config error at {exclude_path}: exclude requires a counter that yields items"
        )));
    }

    let count = count.ok_or_else(|| CounterError::runtime("the counter did not print a count"))?;
    non_negative(count)
}

/// Parses the stdout of a command counter.
pub fn parse_counter_output(raw: &str) -> Result<CounterValue, CounterError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CounterError::runtime("the counter did not print a count"));
    }
    if let Ok(count) = trimmed.parse::<i64>() {
        return Ok(CounterValue {
            count: Some(count),
            items: None,
        });
    }
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|_| CounterError::runtime("the counter did not print a count"))?;
    value_to_counter_value(&value)
}

/// Interprets a JSON value as a counter value.
pub fn value_to_counter_value(value: &Value) -> Result<CounterValue, CounterError> {
    match value {
        Value::Number(number) => {
            let count = number
                .as_i64()
                .ok_or_else(|| CounterError::runtime("the counter did not print a count"))?;
            Ok(CounterValue {
                count: Some(count),
                items: None,
            })
        }
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let text = item
                    .as_str()
                    .ok_or_else(|| CounterError::runtime("the counter did not print a count"))?;
                out.push(text.to_string());
            }
            Ok(CounterValue {
                count: None,
                items: Some(out),
            })
        }
        Value::Object(map) => {
            let count =
                match map.get("count") {
                    None => None,
                    Some(count) => Some(count.as_i64().ok_or_else(|| {
                        CounterError::runtime("the counter did not print a count")
                    })?),
                };
            let items = match map.get("items") {
                None => None,
                Some(items) => {
                    let items = items.as_array().ok_or_else(|| {
                        CounterError::runtime("the counter did not print a count")
                    })?;
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        let text = item.as_str().ok_or_else(|| {
                            CounterError::runtime("the counter did not print a count")
                        })?;
                        out.push(text.to_string());
                    }
                    Some(out)
                }
            };
            if count.is_none() && items.is_none() {
                return Err(CounterError::runtime("the counter did not print a count"));
            }
            Ok(CounterValue { count, items })
        }
        _ => Err(CounterError::runtime("the counter did not print a count")),
    }
}

fn non_negative(count: i64) -> Result<i64, CounterError> {
    if count < 0 {
        return Err(CounterError::runtime(
            "the counter printed a negative count",
        ));
    }
    Ok(count)
}

fn run_command(command: &[String], root: &Path) -> Result<CounterRun, CounterError> {
    let (program, arguments) = command
        .split_first()
        .ok_or_else(|| CounterError::runtime("the counter command is empty"))?;

    let output = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|e| CounterError::runtime(format!("the counter could not be started: {e}")))?;

    let transcript = transcript(command, &output);

    if !output.status.success() {
        let status = match output.status.code() {
            Some(code) => format!("status {code}"),
            None => "an unknown status".to_string(),
        };
        return Err(CounterError::runtime_with(
            format!("the counter exited with {status}"),
            transcript,
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value = parse_counter_output(&stdout)
        .map_err(|error| CounterError::runtime_with(error.message, transcript.clone()))?;

    Ok(CounterRun {
        value,
        output: transcript,
    })
}

fn run_file(
    file: &str,
    json_path: Option<&str>,
    generate: Option<Generator>,
    root: &Path,
) -> Result<CounterRun, CounterError> {
    // Every path in the report is relative to the repository root.
    let display = crate::path::display(Some(root), &root.join(file));

    let (text, output) = match generate {
        Some(generator) => {
            // The artifact is regenerated first: a count read from a stale
            // artifact is a gate that lies.  The generator writes to a
            // temporary path; the tree is never touched.
            let generated = generator
                .run(Path::new(file), root)
                .map_err(|error| CounterError::runtime_with(error.message, error.output))?;
            let text = std::str::from_utf8(&generated.bytes)
                .map_err(|_| {
                    CounterError::runtime_with(
                        format!("the generator for {display} did not write UTF-8"),
                        generated.output.clone(),
                    )
                })?
                .to_string();
            (text, generated.output.clone())
        }
        None => {
            let full = root.join(file);
            let text = fs::read_to_string(&full)
                .map_err(|e| CounterError::runtime(format!("cannot read {display}: {e}")))?;
            (text, String::new())
        }
    };

    // The output of the generator belongs to the diagnostic channel; it is
    // attached to every error that happens after it ran (SPEC 6.2).
    let value: Value = serde_json::from_str(&text).map_err(|e| {
        CounterError::runtime_with(format!("{display} is not valid JSON: {e}"), output.clone())
    })?;
    let value: &Value = match json_path {
        None => &value,
        Some(path) => navigate(&value, path)
            .map_err(|e| CounterError::runtime_with(format!("{display}: {e}"), output.clone()))?,
    };
    let value = value_to_counter_value(value).map_err(|_| {
        CounterError::runtime_with(
            format!("{display} does not contain a count"),
            output.clone(),
        )
    })?;

    Ok(CounterRun { value, output })
}

fn navigate<'a>(value: &'a Value, path: &str) -> Result<&'a Value, String> {
    let mut current = value;
    for segment in path.split('.') {
        let map = current
            .as_object()
            .ok_or_else(|| format!("\"{path}\" not found: {segment} is not an object"))?;
        current = map
            .get(segment)
            .ok_or_else(|| format!("\"{path}\" not found: no \"{segment}\""))?;
    }
    Ok(current)
}

/// The transcript of a subprocess: the diagnostic channel (SPEC 6.2).
///
/// It shows the argv that was really executed.
fn transcript(command: &[String], output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = match output.status.code() {
        Some(code) => format!("status {code}"),
        None => "an unknown status".to_string(),
    };
    format!(
        "{}: {status}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        command.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use globset::GlobBuilder;

    fn matcher(pattern: &str) -> GlobMatcher {
        GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .expect("valid glob")
            .compile_matcher()
    }

    #[test]
    fn parses_an_integer() {
        let value = parse_counter_output("42\n").expect("parses");
        assert_eq!(value.count, Some(42));
        assert!(value.items.is_none());
    }

    #[test]
    fn parses_an_array_of_strings() {
        let value = parse_counter_output(r#"["a.ts", "b.ts"]"#).expect("parses");
        assert_eq!(value.count, None);
        assert_eq!(value.items.unwrap(), vec!["a.ts", "b.ts"]);
    }

    #[test]
    fn parses_an_object_with_a_count() {
        let value = parse_counter_output(r#"{"count": 7}"#).expect("parses");
        assert_eq!(value.count, Some(7));
    }

    #[test]
    fn parses_an_object_with_a_count_and_items() {
        let value = parse_counter_output(r#"{"count": 2, "items": ["a", "b"]}"#).expect("parses");
        assert_eq!(value.count, Some(2));
        assert_eq!(value.items.unwrap().len(), 2);
    }

    #[test]
    fn rejects_unparseable_output() {
        assert!(parse_counter_output("not a number").is_err());
        assert!(parse_counter_output("").is_err());
        assert!(parse_counter_output(r#"{"nope": 1}"#).is_err());
    }

    #[test]
    fn the_message_of_a_counter_error_is_ratchets_own() {
        let error = parse_counter_output("not a number").expect_err("fails");
        assert_eq!(error.message, "the counter did not print a count");
    }

    #[test]
    fn counts_the_items_of_an_array() {
        let value = parse_counter_output(r#"["a.ts", "b.ts"]"#).expect("parses");
        let count = finalize(value, &[], "rules.a.exclude").expect("counts");
        assert_eq!(count, 2);
    }

    #[test]
    fn exclude_removes_items_from_the_count() {
        let value =
            parse_counter_output(r#"["src/a.ts", "src/b.ts", "docs/c.ts"]"#).expect("parses");
        let count = finalize(value, &[matcher("src/**")], "rules.a.exclude").expect("counts");
        assert_eq!(count, 1);
    }

    #[test]
    fn exclude_on_a_scalar_counter_is_a_config_error() {
        let value = parse_counter_output("5").expect("parses");
        let error = finalize(value, &[matcher("src/**")], "rules.a.exclude").expect_err("fails");
        assert!(error.is_config);
        assert!(error.message.contains("rules.a.exclude"));
    }

    #[test]
    fn a_negative_count_is_an_error() {
        let value = parse_counter_output("-3").expect("parses");
        let error = finalize(value, &[], "rules.a.exclude").expect_err("fails");
        assert_eq!(error.message, "the counter printed a negative count");
    }

    #[test]
    fn a_star_does_not_cross_a_path_separator() {
        assert!(matcher("src/*").is_match("src/a.ts"));
        assert!(!matcher("src/*").is_match("src/nested/a.ts"));
        assert!(matcher("src/**").is_match("src/nested/a.ts"));
    }
}
