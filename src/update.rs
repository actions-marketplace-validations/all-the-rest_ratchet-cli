//! The self-update: the newest release, the checksum, the replacement.
//!
//! The binary replaces **itself** and nothing else (SPEC 9): the tree of the
//! project it runs in is never touched, the new binary is written next to the
//! old one and renamed over it.
//!
//! Every endpoint is overridable, so the tests serve a fake release from a
//! local socket and never touch the network.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The repository the releases are published in.
pub const REPOSITORY: &str = "all-the-rest/ratchet-cli";

/// The default base of the release API.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";

/// The default base of the release assets.
pub const DEFAULT_ASSET_BASE: &str =
    "https://github.com/all-the-rest/ratchet-cli/releases/download";

/// The environment variable that overrides the base of the API.
pub const API_BASE_ENV: &str = "RATCHET_UPDATE_API_BASE";

/// The environment variable that overrides the base of the assets.
pub const ASSET_BASE_ENV: &str = "RATCHET_UPDATE_ASSET_BASE";

/// The environment variable that overrides the binary that is replaced.
///
/// It exists for two reasons: a test must not replace the binary it is
/// running in, and an installation that is not the running one can be
/// updated with it.
pub const TARGET_ENV: &str = "RATCHET_UPDATE_TARGET";

/// The name of the checksums file in a release.
pub const CHECKSUMS_NAME: &str = "checksums.txt";

/// How long a request of the self-update may take.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a request of the update check may take.
///
/// The check runs inside a gate: it may not hold the gate hostage.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// The digits of a hex number.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// A failure of the self-update.
///
/// Every failure leaves the running binary as it is.
#[derive(Debug)]
pub struct Error {
    message: String,
}

impl Error {
    /// Creates an error with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The message of the error.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

/// A version: `MAJOR.MINOR.PATCH`.
///
/// The comparison is hand-written: three numbers do not need a crate, and a
/// pre-release suffix (`1.2.3-rc.1`) does not change the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    /// Parses `MAJOR.MINOR.PATCH`.
    ///
    /// A leading `v` — the tag of a release — and a pre-release or build
    /// suffix are ignored: `v1.2.3-rc.1` is `1.2.3`.  A missing part is a
    /// zero: `1.2` is `1.2.0`.
    pub fn parse(text: &str) -> Result<Version, String> {
        let trimmed = text.trim();
        let trimmed = trimmed.strip_prefix('v').unwrap_or(trimmed);
        // The suffix of a pre-release ("1.2.3-rc.1") and of a build
        // ("1.2.3+7") does not take part in the order.
        let core = trimmed.split(['-', '+']).next().unwrap_or(trimmed);

        let mut numbers = [0u64; 3];
        for (index, part) in core.split('.').enumerate() {
            if index == numbers.len() {
                return Err(format!(
                    "{text} is not a version — expected MAJOR.MINOR.PATCH"
                ));
            }
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(format!(
                    "{text} is not a version — expected MAJOR.MINOR.PATCH"
                ));
            }
            let number = part
                .parse::<u64>()
                .map_err(|_| format!("{text} is not a version — {part} is not a number"))?;
            numbers[index] = number;
        }
        Ok(Version {
            major: numbers[0],
            minor: numbers[1],
            patch: numbers[2],
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The platform a release asset is named for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    /// The operating system: `linux`, `macos` or `windows`.
    pub os: String,
    /// The architecture: `x86_64` or `aarch64`.
    pub arch: String,
}

impl Platform {
    /// The platform this binary runs on.
    pub fn current() -> Platform {
        Platform {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }
    }
}

/// The stem of the asset names of a platform: `ratchet-<version>-<os>-<arch>`.
pub fn asset_stem(version: &Version, platform: &Platform) -> String {
    format!("ratchet-{}-{}-{}", version, platform.os, platform.arch)
}

/// The name of the asset that carries the binary of a platform.
///
/// The name is defined **once**, here.  The release workflow mirrors it and a
/// test compares the two, so the self-update can never look for an asset the
/// release does not produce.
pub fn asset_name(version: &Version, platform: &Platform) -> String {
    format!(
        "{}{}",
        asset_stem(version, platform),
        executable_suffix(platform)
    )
}

/// The name of the archive that carries the binary of a platform.
///
/// The archive is what a human unpacks; the self-update takes the binary
/// itself, so that it needs nothing but the file it downloads.
pub fn archive_name(version: &Version, platform: &Platform) -> String {
    format!(
        "{}{}",
        asset_stem(version, platform),
        archive_suffix(platform)
    )
}

/// The suffix of the executable of a platform: `.exe` on windows.
fn executable_suffix(platform: &Platform) -> &'static str {
    if platform.os == "windows" { ".exe" } else { "" }
}

/// The suffix of the archive of a platform: `.zip` on windows, `.tar.gz` on
/// every other platform.
fn archive_suffix(platform: &Platform) -> &'static str {
    if platform.os == "windows" {
        ".zip"
    } else {
        ".tar.gz"
    }
}

/// A release of the repository, as the API reports it.
#[derive(Debug, Deserialize)]
pub struct Release {
    /// The tag of the release, e.g. `v0.2.0`.
    pub tag_name: String,
    /// The assets the release publishes.
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// An asset of a release.
#[derive(Debug, Deserialize)]
pub struct Asset {
    /// The file name of the asset.
    pub name: String,
}

/// What an update run needs.
pub struct Options {
    /// The version of the binary that runs.
    pub current: Version,
    /// The base of the release API.
    pub api_base: String,
    /// The base of the release assets.
    pub download_base: String,
    /// The binary that is replaced.
    pub target: PathBuf,
}

/// The outcome of an update run.
pub enum Outcome {
    /// The running binary is at least as new as the newest release.
    Current {
        /// The version of the newest release.
        release: Version,
    },
    /// The binary was replaced.
    Updated {
        /// The version that was installed.
        version: Version,
    },
    /// The binary could not be replaced; the new one waits next to it.
    Deferred {
        /// The version that was downloaded.
        version: Version,
        /// The path of the downloaded binary.
        path: PathBuf,
        /// The file the downloaded binary has to replace.
        file: PathBuf,
    },
}

/// The base of the release API: the environment overrides the default.
pub fn api_base() -> String {
    std::env::var(API_BASE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
}

/// The base of the release assets: the environment overrides the default.
pub fn download_base() -> String {
    std::env::var(ASSET_BASE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ASSET_BASE.to_string())
}

/// The version of the running binary.
pub fn current_version() -> Result<Version, Error> {
    Version::parse(env!("CARGO_PKG_VERSION")).map_err(Error::new)
}

/// The binary that is replaced: the running one, unless the environment
/// names another one.
pub fn target_path() -> Result<PathBuf, Error> {
    if let Ok(path) = std::env::var(TARGET_ENV) {
        if !path.trim().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    std::env::current_exe()
        .map_err(|error| Error::new(format!("the running binary cannot be located: {error}")))
}

/// The newest release of the repository.
pub fn latest_release(api_base: &str, timeout: Duration) -> Result<Release, Error> {
    let url = format!(
        "{}/repos/{REPOSITORY}/releases/latest",
        api_base.trim_end_matches('/')
    );
    let body = get(&url, timeout)?;
    serde_json::from_slice(&body).map_err(|error| {
        Error::new(format!(
            "the newest release of {REPOSITORY} cannot be read: {error}"
        ))
    })
}

/// Updates the binary: the newest release for this platform, verified and
/// put in place.
///
/// The binary is only touched when the download succeeded **and** the
/// checksum of the asset matches the one of the release.
pub fn run(options: &Options) -> Result<Outcome, Error> {
    let release = latest_release(&options.api_base, REQUEST_TIMEOUT)?;
    let version = Version::parse(&release.tag_name).map_err(Error::new)?;
    if version <= options.current {
        return Ok(Outcome::Current { release: version });
    }

    let platform = Platform::current();
    let name = asset_name(&version, &platform);
    // The release has to publish the asset: a release without it is not a
    // release this binary can be updated from.
    if !release.assets.iter().any(|asset| asset.name == name) {
        return Err(Error::new(format!(
            "the release {} does not publish {name}",
            release.tag_name
        )));
    }

    let bytes = get(
        &asset_url(&options.download_base, &release.tag_name, &name),
        REQUEST_TIMEOUT,
    )?;
    let checksums = get(
        &asset_url(&options.download_base, &release.tag_name, CHECKSUMS_NAME),
        REQUEST_TIMEOUT,
    )?;
    verify(&name, &bytes, &checksums)?;
    replace(options, &version, &bytes)
}

/// The URL of an asset of a release.
fn asset_url(asset_base: &str, tag: &str, name: &str) -> String {
    format!("{}/{}/{}", asset_base.trim_end_matches('/'), tag, name)
}

/// Verifies the bytes of an asset against the checksums of the release.
///
/// A checksum that does not match refuses the update: the binary stays as
/// it is, and the download is thrown away.
fn verify(name: &str, bytes: &[u8], checksums: &[u8]) -> Result<(), Error> {
    let checksums = String::from_utf8_lossy(checksums);
    let Some(expected) = checksum_of(&checksums, name) else {
        return Err(Error::new(format!(
            "{CHECKSUMS_NAME} does not name {name} — the update is refused"
        )));
    };
    let actual = sha256_hex(bytes);
    if !expected.eq_ignore_ascii_case(&actual) {
        return Err(Error::new(format!(
            "refusing: {name} has the checksum {actual}, but {CHECKSUMS_NAME} says {expected} \
             — the binary was not touched"
        )));
    }
    Ok(())
}

/// The checksum of an asset in a `checksums.txt`.
///
/// The format is the one of `sha256sum`: the hash, a space and the file
/// name.  A name that starts with `*` is the binary-mode marker of the same
/// format and names the same file.
fn checksum_of(checksums: &str, name: &str) -> Option<String> {
    for line in checksums.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(hash) = fields.next() else {
            continue;
        };
        let Some(file) = fields.next() else {
            continue;
        };
        let file = file.strip_prefix('*').unwrap_or(file);
        if file == name {
            return Some(hash.to_string());
        }
    }
    None
}

/// The SHA-256 of `bytes`, in lower case hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest.iter() {
        hex.push(char::from(HEX[usize::from(byte >> 4)]));
        hex.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    hex
}

/// Puts the new binary in place.
///
/// On unix the new binary is written next to the old one and renamed over
/// it: the running process keeps its inode, the file on disk is the new one.
/// On windows a running executable cannot be replaced — the new binary is
/// written next to it (`ratchet.exe.new`) and the caller says what to do.
/// Doing nothing silently would be the worst of the three.
fn replace(options: &Options, version: &Version, bytes: &[u8]) -> Result<Outcome, Error> {
    // The file that is replaced: the target itself, or the file behind it
    // when the target is a symbolic link.  A rename over a symbolic link
    // would replace the link with a regular file and change the tree — the
    // file behind the link is replaced instead, and the link is recreated,
    // the same rule the drift restore follows (SPEC 7).
    let link = symlink_of(&options.target)?;
    let file = match &link {
        Some(raw) => resolve(&options.target, raw),
        None => options.target.clone(),
    };

    let directory = match file.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ratchet".to_string());
    let staged = directory.join(format!("{name}.new"));

    fs::write(&staged, bytes)
        .map_err(|error| Error::new(format!("{} cannot be written: {error}", staged.display())))?;
    replace_platform(options, version, staged, file, link)
}

/// Puts the staged binary over the old one.
///
/// On unix the rename is atomic: the running process keeps its inode, the
/// file on disk is the new one.
#[cfg(not(windows))]
fn replace_platform(
    options: &Options,
    version: &Version,
    staged: PathBuf,
    file: PathBuf,
    link: Option<PathBuf>,
) -> Result<Outcome, Error> {
    copy_mode(&staged, &file)?;
    fs::rename(&staged, &file)
        .map_err(|error| Error::new(format!("{} cannot be replaced: {error}", file.display())))?;
    // The link is recreated: the rename replaced the file behind it, and a
    // link that is not recreated would keep the metadata of a link to the
    // old binary.
    if let Some(raw) = link {
        recreate_link(&options.target, &raw)?;
    }
    Ok(Outcome::Updated { version: *version })
}

/// Keeps the staged binary next to the old one.
///
/// A running executable cannot be replaced on windows: the new binary waits
/// as `ratchet.exe.new`, and the caller says what to do with it.
#[cfg(windows)]
fn replace_platform(
    _options: &Options,
    version: &Version,
    staged: PathBuf,
    file: PathBuf,
    _link: Option<PathBuf>,
) -> Result<Outcome, Error> {
    Ok(Outcome::Deferred {
        version: *version,
        path: staged,
        file,
    })
}

/// The raw target of a symbolic link, if the path is one.
///
/// A path that does not exist is no link: it is a file that is not there
/// yet, and the update creates it.
fn symlink_of(path: &Path) -> Result<Option<PathBuf>, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let raw = fs::read_link(path).map_err(|error| {
                Error::new(format!(
                    "the symbolic link {} cannot be read: {error}",
                    path.display()
                ))
            })?;
            Ok(Some(raw))
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::new(format!(
            "{} cannot be read: {error}",
            path.display()
        ))),
    }
}

/// The path a symbolic link points to.
///
/// A relative target is relative to the directory of the link.
fn resolve(target: &Path, raw: &Path) -> PathBuf {
    if raw.is_absolute() {
        crate::path::normalize(raw)
    } else {
        let directory = match target.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        crate::path::normalize(&directory.join(raw))
    }
}

/// Recreates a symbolic link with its raw target.
///
/// The new link is created under a name of its own and renamed over the old
/// one: the path is never missing, and the link keeps pointing where it
/// pointed.
#[cfg(unix)]
fn recreate_link(target: &Path, raw: &Path) -> Result<(), Error> {
    let directory = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ratchet".to_string());
    let temporary = directory.join(format!("{name}.link.new"));
    std::os::unix::fs::symlink(raw, &temporary).map_err(|error| {
        Error::new(format!(
            "the symbolic link {} cannot be created: {error}",
            temporary.display()
        ))
    })?;
    fs::rename(&temporary, target).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        Error::new(format!(
            "the symbolic link {} cannot be recreated: {error}",
            target.display()
        ))
    })
}

/// A symbolic link cannot be recreated on a platform that has none.
#[cfg(all(not(unix), not(windows)))]
fn recreate_link(_target: &Path, _raw: &Path) -> Result<(), Error> {
    Err(Error::new(
        "the binary is a symbolic link and symbolic links are not supported here",
    ))
}

/// Gives the new binary the mode of the old one.
///
/// The mode is taken as it is — the file-type bits are masked out — so the
/// replacement is never wider than what it replaces.  A binary that was
/// `700` stays `700`.
#[cfg(unix)]
fn copy_mode(staged: &Path, target: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(target)
        .map(|metadata| metadata.permissions().mode() & 0o7777)
        .unwrap_or(0o755);
    fs::set_permissions(staged, fs::Permissions::from_mode(mode)).map_err(|error| {
        Error::new(format!(
            "{} cannot get the mode of the old binary: {error}",
            staged.display()
        ))
    })
}

/// The permissions of a binary are not changed on a platform that has none.
#[cfg(all(not(unix), not(windows)))]
fn copy_mode(_staged: &Path, _target: &Path) -> Result<(), Error> {
    Ok(())
}

/// A client with a timeout.
///
/// The timeout is the whole point: a request that hangs must not hang the
/// gate.
fn agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build();
    config.into()
}

/// A GET request that returns the body.
fn get(url: &str, timeout: Duration) -> Result<Vec<u8>, Error> {
    let mut response = agent(timeout)
        .get(url)
        .header("User-Agent", user_agent())
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| match error {
            ureq::Error::StatusCode(code) => {
                Error::new(format!("{url} answers with the status {code}"))
            }
            other => Error::new(format!("{url} cannot be fetched: {other}")),
        })?;
    response
        .body_mut()
        .read_to_vec()
        .map_err(|error| Error::new(format!("the body of {url} cannot be read: {error}")))
}

/// The agent the requests identify themselves with.
///
/// The GitHub API refuses a request without one.
fn user_agent() -> String {
    format!("ratchet/{}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).expect("the version parses")
    }

    fn platform(os: &str, arch: &str) -> Platform {
        Platform {
            os: os.to_string(),
            arch: arch.to_string(),
        }
    }

    #[test]
    fn a_version_is_three_numbers() {
        let parsed = version("1.2.3");
        assert_eq!(parsed.to_string(), "1.2.3");
        // A missing part is a zero.
        assert_eq!(version("1.2").to_string(), "1.2.0");
        assert_eq!(version("1").to_string(), "1.0.0");
    }

    #[test]
    fn a_tag_is_a_version() {
        assert_eq!(version("v1.2.3").to_string(), "1.2.3");
        // The suffix of a pre-release does not change the order.
        assert_eq!(version("v1.2.3-rc.1").to_string(), "1.2.3");
        assert_eq!(version("1.2.3+build.7").to_string(), "1.2.3");
    }

    #[test]
    fn a_text_that_is_no_version_is_refused() {
        for text in ["", "x", "1.x", "1.2.3.4", "1..2", "-1.2.3"] {
            assert!(
                Version::parse(text).is_err(),
                "{text} is not a version and must be refused"
            );
        }
    }

    #[test]
    fn versions_are_compared_by_their_numbers() {
        assert!(version("1.2.3") > version("1.2.2"));
        assert!(version("1.10.0") > version("1.9.0"));
        assert!(version("2.0.0") > version("1.99.99"));
        assert!(version("1.2.3") == version("v1.2.3"));
        // The suffix of a pre-release is ignored, not ordered.
        assert!(version("1.2.3") == version("1.2.3-rc.1"));
    }

    #[test]
    fn the_asset_names_follow_the_release() {
        let version = version("0.2.0");
        let linux = platform("linux", "x86_64");
        assert_eq!(asset_name(&version, &linux), "ratchet-0.2.0-linux-x86_64");
        assert_eq!(
            archive_name(&version, &linux),
            "ratchet-0.2.0-linux-x86_64.tar.gz"
        );
        let windows = platform("windows", "x86_64");
        assert_eq!(
            asset_name(&version, &windows),
            "ratchet-0.2.0-windows-x86_64.exe"
        );
        assert_eq!(
            archive_name(&version, &windows),
            "ratchet-0.2.0-windows-x86_64.zip"
        );
        let macos = platform("macos", "aarch64");
        assert_eq!(asset_name(&version, &macos), "ratchet-0.2.0-macos-aarch64");
        assert_eq!(
            archive_name(&version, &macos),
            "ratchet-0.2.0-macos-aarch64.tar.gz"
        );
    }

    #[test]
    fn the_sha256_is_the_known_one() {
        // The published test vectors of SHA-256.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn a_checksum_file_names_its_assets() {
        let checksums = "\
0000000000000000000000000000000000000000000000000000000000000000  ratchet-0.2.0-linux-x86_64
1111111111111111111111111111111111111111111111111111111111111111  ratchet-0.2.0-linux-aarch64.tar.gz
";
        assert_eq!(
            checksum_of(checksums, "ratchet-0.2.0-linux-x86_64"),
            Some("0000000000000000000000000000000000000000000000000000000000000000".to_string())
        );
        // The binary-mode marker of the format names the same file.
        assert_eq!(
            checksum_of(
                "2222222222222222222222222222222222222222222222222222222222222222 *ratchet-0.2.0-macos-aarch64\n",
                "ratchet-0.2.0-macos-aarch64"
            ),
            Some("2222222222222222222222222222222222222222222222222222222222222222".to_string())
        );
        // An asset that is not named has no checksum.
        assert_eq!(checksum_of(checksums, "ratchet-0.2.0-windows-x86_64"), None);
    }

    #[test]
    fn a_wrong_checksum_is_refused() {
        let bytes = b"the new binary";
        let checksums = format!("{}  ratchet-0.2.0-linux-x86_64\n", "0".repeat(64));
        let error = verify("ratchet-0.2.0-linux-x86_64", bytes, checksums.as_bytes())
            .expect_err("the checksum does not match");
        assert!(error.message().contains("refusing"), "{error}");
    }

    #[test]
    fn a_matching_checksum_is_accepted() {
        let bytes = b"the new binary";
        let checksums = format!("{}  ratchet-0.2.0-linux-x86_64\n", sha256_hex(bytes));
        verify("ratchet-0.2.0-linux-x86_64", bytes, checksums.as_bytes())
            .expect("the checksum matches");
    }
}
