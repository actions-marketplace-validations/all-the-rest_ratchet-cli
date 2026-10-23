//! The self-update and the release assets.
//!
//! The tests never touch the network: a fake release is served from a local
//! socket and the endpoints of the binary are pointed at it.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output};
use std::thread;

use common::TempDir;
use ratchet::update::{self, Platform, Version};

/// The config of the test project: one rule with a counter that always works.
const CONFIG: &str = r#"[
  {
    "name": "test",
    "rules": {
      "counted": {
        "class": "a class",
        "incident": "an instance",
        "unit": "things",
        "direction": "ceiling",
        "count": { "command": ["echo", "3"] }
      }
    }
  }
]
"#;

/// The baseline of the test project.
const BASELINE: &str = r#"{
  "version": 1,
  "rules": {
    "counted": { "value": 3 }
  }
}
"#;

/// A fake release: a local socket that answers the API and the assets.
struct FakeRelease {
    /// The base of the release API.
    api_base: String,
    /// The base of the release assets.
    download_base: String,
}

impl FakeRelease {
    /// Starts a server that serves the given routes.
    ///
    /// The routes are the path and the body of an answer.
    fn start(routes: &[(String, Vec<u8>)]) -> FakeRelease {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the socket is bound");
        let address = listener.local_addr().expect("the address is known");
        let routes: Vec<(String, Vec<u8>)> = routes.to_vec();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    continue;
                };
                let path = read_path(&mut stream);
                let body = routes
                    .iter()
                    .find(|(route, _)| *route == path)
                    .map(|(_, body)| body.clone());
                let mut response = match &body {
                    Some(body) => head("200 OK", body.len()),
                    None => head("404 Not Found", 0),
                };
                if let Some(body) = body {
                    response.extend_from_slice(&body);
                }
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            }
        });
        FakeRelease {
            api_base: format!("http://{address}"),
            download_base: format!("http://{address}/download"),
        }
    }
}

/// The head of a response.
fn head(status: &str, length: usize) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {length}\r\n\
         Content-Type: application/octet-stream\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

/// The path of a request, read from its request line.
fn read_path(stream: &mut TcpStream) -> String {
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
    let text = String::from_utf8_lossy(&request);
    let line = text.lines().next().unwrap_or_default();
    let mut fields = line.split_whitespace();
    let _method = fields.next();
    let path = fields.next().unwrap_or_default();
    path.split('?').next().unwrap_or_default().to_string()
}

/// The routes of a fake release.
fn routes(tag: &str, assets: &[(&str, Vec<u8>)], checksums: &str) -> Vec<(String, Vec<u8>)> {
    let mut routes = Vec::new();
    let names: Vec<String> = assets
        .iter()
        .map(|(name, _)| format!("{{\"name\": \"{name}\"}}"))
        .collect();
    let json = format!(
        "{{\"tag_name\": \"{tag}\", \"assets\": [{}]}}",
        names.join(", ")
    );
    routes.push((
        format!("/repos/{}/releases/latest", update::REPOSITORY),
        json.into_bytes(),
    ));
    for (name, body) in assets {
        routes.push((format!("/download/{tag}/{name}"), body.clone()));
    }
    routes.push((
        format!("/download/{tag}/{}", update::CHECKSUMS_NAME),
        checksums.as_bytes().to_vec(),
    ));
    routes
}

/// The stdout of a run.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The stderr of a run.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The version of the running binary.
fn current_version() -> Version {
    update::current_version().expect("the version of the binary parses")
}

/// The platform of the test.
fn platform() -> Platform {
    Platform::current()
}

/// Runs `ratchet update` against the fake release.
///
/// The cache of the update check points into a temporary directory: the tests
/// never touch the cache of the machine that runs them.
fn run_update(release: &FakeRelease, target: &Path) -> Output {
    let cache = TempDir::new().expect("a temporary directory");
    Command::new(env!("CARGO_BIN_EXE_ratchet"))
        .arg("update")
        .env("RATCHET_UPDATE_API_BASE", &release.api_base)
        .env("RATCHET_UPDATE_ASSET_BASE", &release.download_base)
        .env("RATCHET_UPDATE_TARGET", target)
        .env("RATCHET_NO_UPDATE_CHECK", "1")
        .env("XDG_CACHE_HOME", cache.path())
        .output()
        .expect("the binary runs")
}

/// Runs `ratchet check` in the project, with the update check **enabled**.
///
/// The inherited environment is not trusted: the variable that disables the
/// check is removed from the environment of the child.  A variable that is
/// set job-wide in the CI is a global switch — it switches off not only the
/// runs but also the tests that verify the behaviour it switches off, and a
/// test that needs the behaviour switched on has to be able to lift the
/// switch for its own process.
fn run_check(release: &FakeRelease, project: &Path, cache: &Path) -> Output {
    check_command(release, project, cache)
        .env_remove("RATCHET_NO_UPDATE_CHECK")
        .output()
        .expect("the binary runs")
}

/// The command that runs `ratchet check` in the project.
///
/// The cache of the update check points into the directory of the test: the
/// tests never write into the cache of the machine that runs them.
fn check_command(release: &FakeRelease, project: &Path, cache: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ratchet"));
    command
        .arg("check")
        .current_dir(project)
        .env("RATCHET_UPDATE_API_BASE", &release.api_base)
        .env("RATCHET_UPDATE_ASSET_BASE", &release.download_base)
        .env("XDG_CACHE_HOME", cache);
    command
}

/// The line the update check prints.
const CHECK_LINE: &str = "ratchet: note: a new version is available: 99.0.0 — run `ratchet update`";

/// The lines of a run that belong to the update check.
///
/// The engine prints its own notes too — the ones about a missing git
/// repository, for example.  Only the line of the check is counted.
fn check_lines(output: &Output) -> Vec<String> {
    stderr(output)
        .lines()
        .filter(|line| line.contains("a new version is available"))
        .map(|line| line.to_string())
        .collect()
}

/// A project with a config and a baseline: the gate runs and is green.
fn project() -> TempDir {
    let directory = TempDir::new().expect("a temporary directory");
    fs::write(directory.path().join("ratchet.config.jsonc"), CONFIG)
        .expect("the config is written");
    fs::write(directory.path().join("ratchet.baseline.json"), BASELINE)
        .expect("the baseline is written");
    directory
}

#[test]
fn a_current_binary_is_left_alone() {
    let version = current_version();
    let tag = format!("v{version}");
    let name = update::asset_name(&version, &platform());
    let body = b"the new binary".to_vec();
    let checksums = format!("{}  {name}\n", update::sha256_hex(&body));
    let release = FakeRelease::start(&routes(&tag, &[(name.as_str(), body)], &checksums));
    let directory = TempDir::new().expect("a temporary directory");
    let target = directory.path().join("ratchet");
    fs::write(&target, b"the old binary").expect("the target is written");

    let output = run_update(&release, &target);

    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("is up to date"), "{text}");
    assert!(text.contains(&version.to_string()), "{text}");
    // The binary is untouched.
    assert_eq!(
        fs::read(&target).expect("the target is readable"),
        b"the old binary"
    );
}

#[test]
#[cfg(unix)]
fn a_newer_release_replaces_the_binary() {
    let current = current_version();
    let newer = Version::parse("99.0.0").expect("the version parses");
    let tag = format!("v{newer}");
    let name = update::asset_name(&newer, &platform());
    let body = b"the new binary".to_vec();
    let checksums = format!("{}  {name}\n", update::sha256_hex(&body));
    let release = FakeRelease::start(&routes(&tag, &[(name.as_str(), body)], &checksums));
    let directory = TempDir::new().expect("a temporary directory");
    let target = directory.path().join("ratchet");
    fs::write(&target, b"the old binary").expect("the target is written");

    let output = run_update(&release, &target);

    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains(&current.to_string()), "{text}");
    assert!(text.contains("99.0.0"), "{text}");
    assert!(text.contains("the binary was replaced"), "{text}");
    // The new binary is in place and the staged file is gone.
    assert_eq!(
        fs::read(&target).expect("the target is readable"),
        b"the new binary"
    );
    assert!(!directory.path().join("ratchet.new").exists());
}

#[test]
#[cfg(windows)]
fn a_newer_release_is_staged_when_the_running_binary_cannot_be_replaced() {
    let current = current_version();
    let newer = Version::parse("99.0.0").expect("the version parses");
    let tag = format!("v{newer}");
    let name = update::asset_name(&newer, &platform());
    let body = b"the new binary".to_vec();
    let checksums = format!("{}  {name}\n", update::sha256_hex(&body));
    let release = FakeRelease::start(&routes(&tag, &[(name.as_str(), body)], &checksums));
    let directory = TempDir::new().expect("a temporary directory");
    let target = directory.path().join("ratchet");
    fs::write(&target, b"the old binary").expect("the target is written");

    let output = run_update(&release, &target);

    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains(&current.to_string()), "{text}");
    assert!(text.contains("99.0.0"), "{text}");
    assert!(
        text.contains("a running executable cannot be replaced on windows"),
        "{text}"
    );
    assert_eq!(
        fs::read(&target).expect("the target is readable"),
        b"the old binary"
    );
    assert_eq!(
        fs::read(directory.path().join("ratchet.new")).expect("the staged binary is readable"),
        b"the new binary"
    );
}

/// The release that replaces the binary, with the asset for this platform.
#[cfg(unix)]
fn newer_release() -> (FakeRelease, String) {
    let newer = Version::parse("99.0.0").expect("the version parses");
    let tag = format!("v{newer}");
    let name = update::asset_name(&newer, &platform());
    let body = b"the new binary".to_vec();
    let checksums = format!("{}  {name}\n", update::sha256_hex(&body));
    let release = FakeRelease::start(&routes(&tag, &[(name.as_str(), body)], &checksums));
    (release, tag)
}

#[test]
#[cfg(unix)]
fn the_mode_of_the_old_binary_is_kept() {
    use std::os::unix::fs::PermissionsExt;

    let (release, _tag) = newer_release();
    let directory = TempDir::new().expect("a temporary directory");
    let target = directory.path().join("ratchet");
    fs::write(&target, b"the old binary").expect("the target is written");
    // A binary that only its owner may run.
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).expect("the mode is set");

    let output = run_update(&release, &target);

    assert!(output.status.success(), "{}", stderr(&output));
    let mode = fs::metadata(&target)
        .expect("the target is there")
        .permissions()
        .mode()
        & 0o7777;
    // The replacement is not wider than what it replaces.
    assert_eq!(mode, 0o700, "the mode of the old binary must be kept");
}

#[test]
#[cfg(unix)]
fn a_symbolic_link_is_recreated_and_the_file_behind_it_is_replaced() {
    let (release, _tag) = newer_release();
    let directory = TempDir::new().expect("a temporary directory");
    let bin = directory.path().join("bin");
    let lib = directory.path().join("lib");
    fs::create_dir_all(&bin).expect("the directory is created");
    fs::create_dir_all(&lib).expect("the directory is created");
    let behind = lib.join("ratchet-0.1.0");
    fs::write(&behind, b"the old binary").expect("the file is written");
    let link = bin.join("ratchet");
    std::os::unix::fs::symlink("../lib/ratchet-0.1.0", &link).expect("the link is created");

    let output = run_update(&release, &link);

    assert!(output.status.success(), "{}", stderr(&output));
    // The link is still a link and points where it pointed.
    let metadata = fs::symlink_metadata(&link).expect("the link is there");
    assert!(
        metadata.file_type().is_symlink(),
        "the link must stay a link"
    );
    assert_eq!(
        fs::read_link(&link).expect("the link is read"),
        Path::new("../lib/ratchet-0.1.0")
    );
    // The file behind the link is the new binary, and reading through the
    // link gives the new binary.
    assert_eq!(
        fs::read(&behind).expect("the file is readable"),
        b"the new binary"
    );
    assert_eq!(
        fs::read(&link).expect("the link is readable"),
        b"the new binary"
    );
}

#[test]
fn a_wrong_checksum_refuses_the_update() {
    let newer = Version::parse("99.0.0").expect("the version parses");
    let tag = format!("v{newer}");
    let name = update::asset_name(&newer, &platform());
    let body = b"the new binary".to_vec();
    // The checksum of the release does not belong to the asset.
    let checksums = format!("{}  {name}\n", "0".repeat(64));
    let release = FakeRelease::start(&routes(&tag, &[(name.as_str(), body)], &checksums));
    let directory = TempDir::new().expect("a temporary directory");
    let target = directory.path().join("ratchet");
    fs::write(&target, b"the old binary").expect("the target is written");

    let output = run_update(&release, &target);

    assert!(!output.status.success(), "the update must fail");
    let text = stderr(&output);
    assert!(text.contains("refusing"), "{text}");
    assert!(text.contains("was not touched"), "{text}");
    // The binary is untouched.
    assert_eq!(
        fs::read(&target).expect("the target is readable"),
        b"the old binary"
    );
}

#[test]
fn a_release_without_the_asset_is_refused() {
    let newer = Version::parse("99.0.0").expect("the version parses");
    let tag = format!("v{newer}");
    // The release publishes nothing for this platform.
    let release = FakeRelease::start(&routes(&tag, &[], ""));
    let directory = TempDir::new().expect("a temporary directory");
    let target = directory.path().join("ratchet");
    fs::write(&target, b"the old binary").expect("the target is written");

    let output = run_update(&release, &target);

    assert!(!output.status.success(), "the update must fail");
    let text = stderr(&output);
    assert!(text.contains("does not publish"), "{text}");
    assert_eq!(
        fs::read(&target).expect("the target is readable"),
        b"the old binary"
    );
}

#[test]
fn the_update_check_prints_one_line_and_waits_a_day() {
    let newer = Version::parse("99.0.0").expect("the version parses");
    let tag = format!("v{newer}");
    let release = FakeRelease::start(&routes(&tag, &[], ""));
    let directory = project();
    let cache = directory.path().join("cache");

    let first = run_check(&release, directory.path(), &cache);

    // The gate is green and the check does not change that.
    assert!(first.status.success(), "{}", stderr(&first));
    // The assertion looks at the line of the check and at nothing else: the
    // engine prints its own notes — "not a git repository" and the like —
    // and they are none of the check's business.
    let lines = check_lines(&first);
    assert_eq!(lines, vec![CHECK_LINE], "the check prints one line");
    // The state of the check is written.
    assert!(
        cache.join("ratchet").join("update-check").is_file(),
        "the check writes its state"
    );

    // The second run does not ask again: the state is younger than a day.
    let second = run_check(&release, directory.path(), &cache);
    assert!(second.status.success(), "{}", stderr(&second));
    assert!(
        check_lines(&second).is_empty(),
        "the check waits a day: {}",
        stderr(&second)
    );
}

#[test]
fn a_failing_update_check_is_silent() {
    let directory = project();
    let cache = directory.path().join("cache");
    // Nothing listens on this port: the check cannot reach the API.
    let release = FakeRelease {
        api_base: "http://127.0.0.1:1".to_string(),
        download_base: "http://127.0.0.1:1/download".to_string(),
    };

    let output = run_check(&release, directory.path(), &cache);

    // The gate is green and the failure of the check is invisible.
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        !stderr(&output).contains("a new version is available"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_update_check_can_be_disabled_by_the_flag() {
    let newer = Version::parse("99.0.0").expect("the version parses");
    let tag = format!("v{newer}");
    let release = FakeRelease::start(&routes(&tag, &[], ""));
    let directory = project();
    let cache = directory.path().join("cache");

    // The flag is what disables the check here: the environment of the
    // machine that runs the test does not matter.
    let output = check_command(&release, directory.path(), &cache)
        .arg("--no-update-check")
        .env_remove("RATCHET_NO_UPDATE_CHECK")
        .output()
        .expect("the binary runs");

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        check_lines(&output).is_empty(),
        "the check must be silent: {}",
        stderr(&output)
    );
    // The disabled check does not even write its state.
    assert!(!cache.join("ratchet").join("update-check").exists());
}

#[test]
fn the_update_check_can_be_disabled_by_the_environment() {
    let newer = Version::parse("99.0.0").expect("the version parses");
    let tag = format!("v{newer}");
    let release = FakeRelease::start(&routes(&tag, &[], ""));
    let directory = project();
    let cache = directory.path().join("cache");

    // The variable is what disables the check here: the environment of the
    // machine that runs the test does not matter.
    let output = check_command(&release, directory.path(), &cache)
        .env("RATCHET_NO_UPDATE_CHECK", "1")
        .output()
        .expect("the binary runs");

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        check_lines(&output).is_empty(),
        "the check must be silent: {}",
        stderr(&output)
    );
    // The disabled check does not even write its state.
    assert!(!cache.join("ratchet").join("update-check").exists());
}

/// The release workflow of this repository.
const WORKFLOW: &str = ".github/workflows/release.yml";

/// The value of a key of the workflow.
fn value_of(workflow: &str, key: &str) -> String {
    let values = values_of(workflow, key);
    assert_eq!(values.len(), 1, "the workflow has one {key}");
    values.into_iter().next().expect("the value is there")
}

/// The values of a key of the workflow, in the order of the file.
fn values_of(workflow: &str, key: &str) -> Vec<String> {
    let prefix = format!("{key}:");
    workflow
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let rest = trimmed.strip_prefix(&prefix)?;
            let value = rest.trim();
            let value = value.strip_prefix('\'').unwrap_or(value);
            let value = value.strip_suffix('\'').unwrap_or(value);
            let value = value.strip_prefix('"').unwrap_or(value);
            let value = value.strip_suffix('"').unwrap_or(value);
            Some(value.to_string())
        })
        .collect()
}

/// The workflow of the release.
fn workflow() -> String {
    fs::read_to_string(WORKFLOW).expect("the workflow is readable")
}

/// A manifest with a version.
const MANIFEST: &str = "[package]\nname = \"ratchet\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";

/// The script of a step of the workflow.
///
/// The step is found by its name; the script is the block that follows its
/// `run:` key, with the indentation of the block removed.
fn step_script(workflow: &str, name: &str) -> String {
    let lines: Vec<&str> = workflow.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == format!("- name: {name}"))
        .unwrap_or_else(|| panic!("the workflow has no step {name}"));
    let run = lines[start..]
        .iter()
        .position(|line| line.trim_start().starts_with("run:"))
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("the step {name} has no run"));
    let indentation = lines[run].len() - lines[run].trim_start().len();

    let mut block = Vec::new();
    for line in &lines[run + 1..] {
        if line.trim().is_empty() {
            block.push(String::new());
            continue;
        }
        if line.len() - line.trim_start().len() <= indentation {
            break;
        }
        block.push((*line).to_string());
    }
    let common = block
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    block
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                line[common..].to_string()
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Runs a script of the workflow in a directory, with a tag.
///
/// The tag reaches the script as `RELEASE_TAG`: that is the variable the
/// workflow defines, and every job reads the tag from it.
fn run_step(script: &str, directory: &Path, tag: &str) -> Output {
    Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(directory)
        .env("RELEASE_TAG", tag)
        .env("GITHUB_REF_NAME", tag)
        .env("GITHUB_OUTPUT", directory.join("github-output"))
        .output()
        .expect("the script runs")
}

/// The version of this crate, read from the manifest.
///
/// The version is **read** and not written down here: a pre-release version
/// in the manifest has to show up in this test — the assets of a release
/// carry the version of the manifest, while the binary asks for the version
/// without the suffix of a pre-release.
fn manifest_version() -> String {
    let manifest = fs::read_to_string("Cargo.toml").expect("the manifest is readable");
    let mut section = String::new();
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed.to_string();
            continue;
        }
        if section != "[package]" {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("version") else {
            continue;
        };
        let rest = rest.trim_start().strip_prefix('=').unwrap_or(rest).trim();
        let rest = rest.strip_prefix('"').unwrap_or(rest);
        let rest = rest.strip_suffix('"').unwrap_or(rest);
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    panic!("the manifest of this crate has no version");
}

/// Fills the placeholders of a template.
fn substitute(template: &str, values: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (key, value) in values {
        result = result.replace(&format!("{{{key}}}"), value);
    }
    result
}

#[test]
fn the_asset_names_of_the_workflow_match_the_binary() {
    let workflow = workflow();
    let asset_template = value_of(&workflow, "ASSET_TEMPLATE");
    let archive_template = value_of(&workflow, "ARCHIVE_TEMPLATE");
    let systems = values_of(&workflow, "asset_os");
    let architectures = values_of(&workflow, "asset_arch");
    let executables = values_of(&workflow, "asset_exe");
    let compressions = values_of(&workflow, "asset_compression");
    let count = systems.len();
    assert!(count > 0, "the workflow builds something");
    assert_eq!(
        architectures.len(),
        count,
        "every build has an architecture"
    );
    assert_eq!(executables.len(), count, "every build has an executable");
    assert_eq!(compressions.len(), count, "every build has a compression");

    // The version of the manifest is the version of the release.
    let version = manifest_version();
    let parsed = Version::parse(&version).expect("the version parses");
    for index in 0..count {
        let platform = Platform {
            os: systems[index].clone(),
            arch: architectures[index].clone(),
        };
        let asset = substitute(
            &asset_template,
            &[
                ("version", &version),
                ("os", &systems[index]),
                ("arch", &architectures[index]),
                ("exe", &executables[index]),
            ],
        );
        assert_eq!(
            asset,
            update::asset_name(&parsed, &platform),
            "the asset name of {platform:?} differs"
        );
        let archive = substitute(
            &archive_template,
            &[
                ("version", &version),
                ("os", &systems[index]),
                ("arch", &architectures[index]),
                ("compression", &compressions[index]),
            ],
        );
        assert_eq!(
            archive,
            update::archive_name(&parsed, &platform),
            "the archive name of {platform:?} differs"
        );
    }
}

#[test]
fn the_release_covers_every_platform() {
    let workflow = workflow();
    let systems = values_of(&workflow, "asset_os");
    let architectures = values_of(&workflow, "asset_arch");
    let platforms: Vec<String> = systems
        .iter()
        .zip(architectures.iter())
        .map(|(system, architecture)| format!("{system}-{architecture}"))
        .collect();
    for expected in [
        "linux-x86_64",
        "linux-aarch64",
        "macos-x86_64",
        "macos-aarch64",
        "windows-x86_64",
    ] {
        assert!(
            platforms.iter().any(|platform| platform == expected),
            "the release does not cover {expected}: {platforms:?}"
        );
    }
}

#[test]
fn the_release_checks_the_tag_against_the_manifest() {
    // The step of the workflow is not read but **executed**: a check that
    // only looks for two words in the file would still pass if the
    // comparison were inverted or the failure were removed.
    let workflow = workflow();
    let script = step_script(&workflow, "Read the version from the manifest");
    let directory = TempDir::new().expect("a temporary directory");
    fs::write(directory.path().join("Cargo.toml"), MANIFEST).expect("the manifest is written");

    // A tag that matches the manifest passes and hands the version on.
    let output = run_step(&script, directory.path(), "v0.1.0");
    assert!(
        output.status.success(),
        "the step must pass: {}\n{}\n{}",
        stderr(&output),
        stdout(&output),
        script
    );
    let written =
        fs::read_to_string(directory.path().join("github-output")).expect("the output is read");
    assert!(written.contains("version=0.1.0"), "{written}");

    // A tag that does not match fails the release.
    let output = run_step(&script, directory.path(), "v0.2.0");
    assert!(!output.status.success(), "the release must fail");
    // The error of the workflow is a command on stdout: that is where
    // GitHub reads it from.
    let text = stdout(&output);
    assert!(
        text.contains("::error::the tag v0.2.0 does not match the version 0.1.0 of Cargo.toml"),
        "{text}"
    );
}

#[test]
fn the_release_reads_the_tag_from_one_place() {
    let workflow = workflow();
    // The tag of the release reaches every job through one variable: a run
    // that was started by the push of a tag and a run that was started by
    // hand have to publish the same release.
    assert!(
        workflow.contains("RELEASE_TAG: ${{ inputs.tag || github.ref_name }}"),
        "the workflow defines the tag of the release"
    );
    // The scripts read that variable — not the one of the event: the two
    // differ in a run that was started by hand.
    for step in [
        "Read the version from the manifest",
        "Create the draft release",
        "Check the release",
    ] {
        let script = step_script(&workflow, step);
        assert!(
            !script.contains("GITHUB_REF_NAME"),
            "{step} reads the tag of the event, not the tag of the release"
        );
        assert!(
            script.contains("RELEASE_TAG"),
            "{step} does not read the tag of the release"
        );
    }
}

#[test]
fn the_release_is_a_draft_so_the_marketplace_click_is_possible() {
    let workflow = workflow();
    let script = step_script(&workflow, "Create the draft release");
    // Publishing a release is the click that also publishes the Marketplace
    // listing; a workflow cannot tick that box. A release the workflow
    // published by itself is invisible in the Marketplace, and nothing in the
    // history would say why.
    assert!(
        script.contains("--draft"),
        "the release workflow publishes the release instead of drafting it"
    );
}

#[test]
fn the_release_fails_when_the_manifest_has_no_version() {
    let workflow = workflow();
    let script = step_script(&workflow, "Read the version from the manifest");
    let directory = TempDir::new().expect("a temporary directory");
    fs::write(
        directory.path().join("Cargo.toml"),
        "[package]\nname = \"ratchet\"\n",
    )
    .expect("the manifest is written");

    let output = run_step(&script, directory.path(), "v0.1.0");

    assert!(!output.status.success(), "the release must fail");
    let text = stdout(&output);
    assert!(
        text.contains("::error::the version cannot be read from Cargo.toml"),
        "{text}"
    );
}

#[test]
fn the_release_writes_the_checksums_of_every_asset() {
    let workflow = workflow();
    // The checksums cover every asset of the release.
    assert!(
        workflow.contains("checksums.txt") && workflow.contains("sha256sum"),
        "the workflow writes the checksums of the assets"
    );
}
