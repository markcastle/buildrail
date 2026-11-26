//! Release version numbers: `major.minor.patch` and nothing else.
//!
//! BuildRail's releases never carry pre-release or build suffixes, so the
//! parser accepts exactly three numbers. Anything else is a typing mistake,
//! and refusing it is kinder than releasing `v0.3` by accident.

use std::fmt;

/// A release number. Ordering compares major, then minor, then patch, so
/// 0.10.0 sorts after 0.9.0 (as numbers, not as text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// Changes only by typing a version by hand: leaving 0.x is a judgement.
    pub major: u64,
    /// Something a person would notice as new.
    pub minor: u64,
    /// Fixes and adjustments.
    pub patch: u64,
}

impl Version {
    /// Builds a version from its three numbers.
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses `x.y.z`, refusing leading zeros, signs, suffixes and blanks.
    pub fn parse(text: &str) -> Result<Self, String> {
        let refuse =
            || format!("\"{text}\" is not a version. Use major.minor.patch, such as 0.3.0.");
        let parts: Vec<&str> = text.split('.').collect();
        if parts.len() != 3 {
            return Err(refuse());
        }
        let mut numbers = [0_u64; 3];
        for (slot, part) in numbers.iter_mut().zip(&parts) {
            let digits_only = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
            let leading_zero = part.len() > 1 && part.starts_with('0');
            if !digits_only || leading_zero {
                return Err(refuse());
            }
            *slot = part.parse().map_err(|_| refuse())?;
        }
        Ok(Self::new(numbers[0], numbers[1], numbers[2]))
    }

    /// The next minor release: 0.2.3 becomes 0.3.0.
    pub const fn next_minor(self) -> Self {
        Self::new(self.major, self.minor + 1, 0)
    }

    /// The next patch: 0.2.3 becomes 0.2.4.
    pub const fn next_patch(self) -> Self {
        Self::new(self.major, self.minor, self.patch + 1)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The highest `vX.Y.Z` among git tag names, ignoring anything that is not a
/// release tag (such as `v1.0.0-rc1` or `nightly`).
pub fn highest_tag<'a>(tags: impl IntoIterator<Item = &'a str>) -> Option<Version> {
    tags.into_iter()
        .filter_map(|tag| tag.trim().strip_prefix('v'))
        .filter_map(|v| Version::parse(v).ok())
        .max()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parses_three_numbers() {
        assert_eq!(Version::parse("0.2.0").unwrap(), Version::new(0, 2, 0));
        assert_eq!(
            Version::parse("10.20.30").unwrap(),
            Version::new(10, 20, 30)
        );
    }

    #[test]
    fn refuses_anything_else() {
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "1.2.x",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "1.2.3-rc1",
            "1..3",
            " 1.2.3",
            "+1.2.3",
            "99999999999999999999.0.0",
        ] {
            let error = Version::parse(bad).unwrap_err();
            assert!(error.contains("is not a version"), "{bad}: {error}");
        }
    }

    #[test]
    fn orders_as_numbers_not_text() {
        assert!(Version::parse("0.10.0").unwrap() > Version::parse("0.9.9").unwrap());
        assert!(Version::parse("1.0.0").unwrap() > Version::parse("0.99.99").unwrap());
    }

    #[test]
    fn bumps() {
        let v = Version::new(0, 2, 3);
        assert_eq!(v.next_minor().to_string(), "0.3.0");
        assert_eq!(v.next_patch().to_string(), "0.2.4");
    }

    #[test]
    fn highest_tag_ignores_non_release_tags() {
        let tags = [
            "v0.9.0",
            "v0.10.0",
            "v1.0.0-rc1",
            "nightly",
            "v0.2.0",
            " v0.10.1 \r",
        ];
        assert_eq!(highest_tag(tags), Some(Version::new(0, 10, 1)));
        assert_eq!(highest_tag(["nightly"]), None);
        assert_eq!(highest_tag([]), None);
    }
}
