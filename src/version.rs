//! The version of a release: three numbers and the step that moves them.
//!
//! The version is **defined once**, here: the self-update compares versions
//! (SPEC §9) and the release step computes the next one (SPEC §16).  Two
//! definitions would drift, and a version that drifts from the history is
//! the failure this tool exists to prevent.

use std::fmt;

/// The step a version moves by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Step {
    /// The version does not move.
    #[default]
    None,
    /// The patch part moves: `1.2.3` becomes `1.2.4`.
    Patch,
    /// The minor part moves: `1.2.3` becomes `1.3.0`.
    Minor,
    /// The major part moves: `1.2.3` becomes `2.0.0`.
    Major,
}

impl Step {
    /// Parses a step: `major`, `minor`, `patch` or `none`.
    pub fn parse(text: &str) -> Result<Step, String> {
        match text {
            "major" => Ok(Step::Major),
            "minor" => Ok(Step::Minor),
            "patch" => Ok(Step::Patch),
            "none" => Ok(Step::None),
            other => Err(format!(
                "{other} is not a step — expected \"major\", \"minor\", \"patch\" or \"none\""
            )),
        }
    }

    /// The name of the step, as the configuration writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Step::None => "none",
            Step::Patch => "patch",
            Step::Minor => "minor",
            Step::Major => "major",
        }
    }
}

/// A version: `MAJOR.MINOR.PATCH`.
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

    /// The major part.
    pub fn major(&self) -> u64 {
        self.major
    }

    /// The minor part.
    pub fn minor(&self) -> u64 {
        self.minor
    }

    /// The patch part.
    pub fn patch(&self) -> u64 {
        self.patch
    }

    /// The version after the step.
    ///
    /// A step that moves a part resets the parts below it: `1.2.3` with a
    /// minor step is `1.3.0`, not `1.3.3`.
    pub fn bump(&self, step: Step) -> Version {
        match step {
            Step::None => *self,
            Step::Patch => Version {
                patch: self.patch + 1,
                ..*self
            },
            Step::Minor => Version {
                minor: self.minor + 1,
                patch: 0,
                ..*self
            },
            Step::Major => Version {
                major: self.major + 1,
                minor: 0,
                patch: 0,
            },
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Parses the tag of a release.
///
/// A release tag is the **prefix** and three numbers — `v1.2.3`.  Nothing
/// more: a pre-release (`v1.2.3-rc.1`) is not a release, and the moving
/// major tag of the action (`v1`) is not a release either.  A tag that is
/// not a release tag must not be used as the base of a release: the version
/// it stands for is a guess.
pub fn parse_tag(tag: &str, prefix: &str) -> Result<Version, String> {
    let trimmed = tag.trim();
    let rest = if prefix.is_empty() {
        trimmed
    } else {
        trimmed.strip_prefix(prefix).ok_or_else(|| {
            format!("{tag} is not a release tag — a release tag is {prefix}MAJOR.MINOR.PATCH")
        })?
    };
    let parts: Vec<&str> = rest.split('.').collect();
    if parts.len() != 3 {
        return Err(format!(
            "{tag} is not a release tag — a release tag is {prefix}MAJOR.MINOR.PATCH"
        ));
    }
    if parts
        .iter()
        .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(format!(
            "{tag} is not a release tag — a release tag is {prefix}MAJOR.MINOR.PATCH"
        ));
    }
    Version::parse(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_three_numbers() {
        let version = Version::parse("1.2.3").expect("the version parses");
        assert_eq!(version.major(), 1);
        assert_eq!(version.minor(), 2);
        assert_eq!(version.patch(), 3);
        assert_eq!(version.to_string(), "1.2.3");
    }

    #[test]
    fn a_tag_is_a_version() {
        assert_eq!(
            Version::parse("v1.2.3").expect("the version parses"),
            Version::parse("1.2.3").expect("the version parses")
        );
    }

    #[test]
    fn a_suffix_does_not_take_part_in_the_order() {
        assert_eq!(
            Version::parse("v1.2.3-rc.1").expect("the version parses"),
            Version::parse("1.2.3").expect("the version parses")
        );
    }

    #[test]
    fn a_missing_part_is_a_zero() {
        assert_eq!(
            Version::parse("1.2").expect("the version parses"),
            Version::parse("1.2.0").expect("the version parses")
        );
    }

    #[test]
    fn a_text_is_not_a_version() {
        assert!(Version::parse("1.2.x").is_err());
        assert!(Version::parse("1.2.3.4").is_err());
        assert!(Version::parse("").is_err());
    }

    #[test]
    fn a_step_moves_one_part_and_resets_the_ones_below() {
        let version = Version::parse("1.2.3").expect("the version parses");
        assert_eq!(version.bump(Step::Patch).to_string(), "1.2.4");
        assert_eq!(version.bump(Step::Minor).to_string(), "1.3.0");
        assert_eq!(version.bump(Step::Major).to_string(), "2.0.0");
        assert_eq!(version.bump(Step::None).to_string(), "1.2.3");
    }

    #[test]
    fn a_step_has_a_name_and_a_parsing() {
        assert_eq!(Step::parse("major").expect("the step parses"), Step::Major);
        assert_eq!(Step::parse("none").expect("the step parses"), Step::None);
        assert_eq!(Step::Major.as_str(), "major");
        assert!(Step::parse("huge").is_err());
    }

    #[test]
    fn the_steps_are_ordered() {
        assert!(Step::Major > Step::Minor);
        assert!(Step::Minor > Step::Patch);
        assert!(Step::Patch > Step::None);
    }

    #[test]
    fn a_release_tag_is_the_prefix_and_three_numbers() {
        assert_eq!(
            parse_tag("v1.2.3", "v")
                .expect("the tag parses")
                .to_string(),
            "1.2.3"
        );
        assert_eq!(
            parse_tag("1.2.3", "").expect("the tag parses").to_string(),
            "1.2.3"
        );
    }

    #[test]
    fn a_tag_that_is_not_a_release_is_refused() {
        // The moving major tag of the action.
        assert!(parse_tag("v1", "v").is_err());
        // A pre-release.
        assert!(parse_tag("v1.2.3-rc.1", "v").is_err());
        // A tag without the prefix.
        assert!(parse_tag("1.2.3", "v").is_err());
        // A tag with too few parts.
        assert!(parse_tag("v1.2", "v").is_err());
        assert!(parse_tag("v1.2.3.4", "v").is_err());
    }
}
