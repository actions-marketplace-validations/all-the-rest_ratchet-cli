//! Regenerating an artifact before it is counted.
//!
//! A count read from a stale artifact is a gate that lies.  The generator runs
//! before the count is read and writes to a **temporary path**: the tree is
//! never touched.  The in-place mode — where the generator overwrites the
//! artifact and the engine has to restore it — belongs to the drift gate
//! (SPEC §7), which owns a verified restore.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::fsutil::Temporary;

/// A generator: the argv and the output argument.
#[derive(Debug, Clone)]
pub struct Generator {
    /// The argv of the generator.
    pub command: Vec<String>,
    /// The argument the generator writes its output to.
    pub output_arg: String,
}

/// A generator that did not produce an artifact.
#[derive(Debug, Clone)]
pub struct GeneratorError {
    /// Ratchet's own sentence; the report carries it.
    pub message: String,
    /// The raw output of the subprocess; the diagnostic channel carries it.
    pub output: String,
}

/// The bytes of a freshly generated artifact.
#[derive(Debug)]
pub struct GeneratedArtifact {
    /// The produced bytes.
    pub bytes: Vec<u8>,
    /// The raw output of the generator; the diagnostic channel carries it.
    pub output: String,
    /// Removes the temporary directory when it goes out of scope.
    _temporary: Temporary,
}

impl Generator {
    /// Runs the generator and returns the produced bytes of `artifact`.
    ///
    /// `artifact` is the path of the counted file, relative to the repository
    /// root.  The generator writes to a temporary path; the tree stays as it
    /// is.
    pub fn run(&self, artifact: &Path, root: &Path) -> Result<GeneratedArtifact, GeneratorError> {
        // The guard removes the directory on every path out of this function:
        // a successful run, a failing generator, and every early return in
        // between.
        let temporary = Temporary::new("generate").map_err(|e| GeneratorError {
            message: "the generator could not be started".to_string(),
            output: format!("cannot create the temporary directory: {e}"),
        })?;
        let name = artifact
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "artifact".to_string());
        let output = temporary.path().join(name);

        // Every path in the report is relative to the repository root.
        let display = crate::path::display(Some(root), &root.join(artifact));

        let mut command = self.command.clone();
        command.push(self.output_arg.clone());
        command.push(output.to_string_lossy().into_owned());

        let transcript = run_generator(&command, root).map_err(|failure| GeneratorError {
            message: format!("the generator for {display} {}", failure.message),
            output: failure.transcript,
        })?;

        let bytes = fs::read(&output).map_err(|e| GeneratorError {
            message: format!("the generator for {display} did not write its output"),
            output: format!("{transcript}\ncannot read the generated artifact: {e}"),
        })?;

        Ok(GeneratedArtifact {
            bytes,
            output: transcript,
            _temporary: temporary,
        })
    }
}

/// A generator that did not succeed.
struct Failure {
    /// What happened, in ratchet's own words.
    message: String,
    /// The raw output of the generator.
    transcript: String,
}

/// Runs the generator and fails when it does not succeed.
fn run_generator(command: &[String], root: &Path) -> Result<String, Failure> {
    let (program, arguments) = command.split_first().ok_or_else(|| Failure {
        message: "could not be started: the command is empty".to_string(),
        transcript: "the generator command is empty".to_string(),
    })?;

    let output = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|e| Failure {
            // The process never started: it did not exit with anything.
            message: format!("could not be started: {e}"),
            transcript: format!("{}: could not be started\n{e}", command_line(command)),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = match output.status.code() {
        Some(code) => format!("status {code}"),
        None => "an unknown status".to_string(),
    };
    // The transcript shows the argv that was really executed — the output
    // path the generator was told to write to included (SPEC 6.2).
    let transcript = format!(
        "{}: {status}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        command_line(command)
    );

    if !output.status.success() {
        return Err(Failure {
            message: format!("exited with {status}"),
            transcript,
        });
    }

    Ok(transcript)
}

/// The argv of a subprocess, as a human readable line.
fn command_line(command: &[String]) -> String {
    command.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_to_a_temporary_path() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let artifact = directory.path().join("artifact.json");
        fs::write(&artifact, "original").expect("the file is written");

        let generator = Generator {
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf 'produced' > \"$1\"".to_string(),
            ],
            output_arg: "--output".to_string(),
        };

        {
            let produced = generator
                .run(Path::new("artifact.json"), directory.path())
                .expect("it runs");
            assert_eq!(produced.bytes, b"produced");
        }

        // The artifact is untouched: the generator wrote to a temporary path.
        assert_eq!(
            fs::read_to_string(&artifact).expect("the file is readable"),
            "original"
        );
    }

    #[test]
    fn a_failing_generator_reports_its_status_and_keeps_its_output() {
        let directory = tempfile::tempdir().expect("a temporary directory");

        let generator = Generator {
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo 'the reason' >&2; exit 3".to_string(),
            ],
            output_arg: "--output".to_string(),
        };

        let error = generator
            .run(Path::new("artifact.json"), directory.path())
            .expect_err("it fails");

        assert_eq!(
            error.message,
            "the generator for artifact.json exited with status 3"
        );
        // The raw output belongs to the diagnostic channel.
        assert!(error.output.contains("the reason"), "{}", error.output);
    }
}
