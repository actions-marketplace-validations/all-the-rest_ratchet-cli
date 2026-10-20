//! The update check: at most once a day, silent on every failure.
//!
//! The check asks the release API for the newest version and prints one line
//! when there is a newer one.  It never fails a gate: every error — a network
//! problem, an unreadable cache file — is swallowed (SPEC 9).
//!
//! The state of the check lives in the cache directory:
//! `$XDG_CACHE_HOME/ratchet/update-check`, or `~/.cache/ratchet/update-check`
//! when the variable is not set.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::update::{self, Version};

/// The environment variable that disables the check.
pub const NO_UPDATE_CHECK_ENV: &str = "RATCHET_NO_UPDATE_CHECK";

/// The name of the state file in the cache directory.
const STATE_FILE: &str = "update-check";

/// The shortest time between two checks.
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Runs the update check and returns the line to print, if there is one.
///
/// The check happens at most once a day: the timestamp of the last attempt
/// lives in the cache directory.  Every failure is silent — a network
/// problem must never turn a green gate red.
pub fn run(api_base: &str, current: Version) -> Option<String> {
    if disabled() {
        return None;
    }
    let state = state_path()?;
    if checked_recently(&state) {
        return None;
    }
    // The attempt counts, not the success: a gate that waits for the network
    // twice in a day is worse than a gate that misses a release.
    let _ = write_state(&state);

    let release = update::latest_release(api_base, update::CHECK_TIMEOUT).ok()?;
    let version = Version::parse(&release.tag_name).ok()?;
    if version <= current {
        return None;
    }
    Some(format!(
        "a new version is available: {version} — run `ratchet update`"
    ))
}

/// Marks the check as done, so it does not run again today.
///
/// The self-update calls it: the binary is up to date now, and the check
/// must not announce the version that was just installed.
///
/// A disabled check writes nothing: the variable that disables it also keeps
/// the tool from touching the cache directory.
pub fn mark_checked() {
    if disabled() {
        return;
    }
    if let Some(state) = state_path() {
        let _ = write_state(&state);
    }
}

/// True when the environment disables the check.
pub fn disabled() -> bool {
    disabled_by(std::env::var(NO_UPDATE_CHECK_ENV).ok().as_deref())
}

/// True when a value of the environment variable disables the check.
///
/// Every value but an empty one and `0` disables it: the variable is a
/// switch, not a truth value.
fn disabled_by(value: Option<&str>) -> bool {
    match value {
        Some(value) => {
            let value = value.trim();
            !value.is_empty() && value != "0"
        }
        None => false,
    }
}

/// The path of the state file.
///
/// The cache directory is `$XDG_CACHE_HOME/ratchet`, or `~/.cache/ratchet`
/// when the variable is not set.
pub fn state_path() -> Option<PathBuf> {
    if let Ok(base) = std::env::var("XDG_CACHE_HOME") {
        if !base.trim().is_empty() {
            return Some(state_in(Path::new(&base)));
        }
    }
    let home = std::env::var("HOME").ok()?;
    if home.trim().is_empty() {
        return None;
    }
    Some(state_in(&Path::new(&home).join(".cache")))
}

/// The path of the state file in a cache directory.
fn state_in(cache: &Path) -> PathBuf {
    cache.join("ratchet").join(STATE_FILE)
}

/// True when the last attempt is younger than a day.
///
/// A state file that cannot be read is no attempt: the check runs again.
fn checked_recently(state: &Path) -> bool {
    let Ok(text) = fs::read_to_string(state) else {
        return false;
    };
    let Ok(stamp) = text.trim().parse::<u64>() else {
        return false;
    };
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return false;
    };
    now.as_secs().saturating_sub(stamp) < INTERVAL.as_secs()
}

/// Writes the timestamp of the attempt into the state file.
fn write_state(state: &Path) -> Option<()> {
    let directory = state.parent()?;
    fs::create_dir_all(directory).ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    fs::write(state, now.to_string()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_is_disabled_by_the_environment() {
        // The variable is read from the process environment; the decision
        // itself is a pure function of its value.
        assert!(disabled_by(Some("1")));
        assert!(disabled_by(Some("true")));
        assert!(!disabled_by(Some("0")));
        assert!(!disabled_by(Some("")));
        assert!(!disabled_by(Some("  ")));
        assert!(!disabled_by(None));
    }

    #[test]
    fn the_state_lives_in_the_cache_directory() {
        assert_eq!(
            state_in(Path::new("/var/cache")),
            PathBuf::from("/var/cache/ratchet/update-check")
        );
    }
}
