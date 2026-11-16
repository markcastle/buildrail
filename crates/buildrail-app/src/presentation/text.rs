//! Small text helpers shared by the strip, the detail popup and the
//! accessible names: relative ages and short commit hashes.
//!
//! Two age styles exist on purpose: the strip is 24 px tall and needs the
//! compact form ("5 min ago"), while screen readers read abbreviations
//! badly and get the spelled-out form ("5 minutes ago").

use chrono::TimeDelta;

/// Number of hex digits shown for a commit, like `git log --oneline`.
pub const SHORT_COMMIT_LEN: usize = 7;

/// Compact relative age for the strip and tooltips: `just now`,
/// `5 min ago`, `2 h ago`, `3 d ago`. Negative ages (a provider clock a
/// little ahead of ours) read as `just now`.
pub fn age_short(age: TimeDelta) -> String {
    let secs = age.num_seconds().max(0);
    match secs {
        0..60 => "just now".to_owned(),
        60..3_600 => format!("{} min ago", secs / 60),
        3_600..86_400 => format!("{} h ago", secs / 3_600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

/// Spelled-out relative age for accessible names: `just now`,
/// `1 minute ago`, `2 hours ago`, `3 days ago`.
pub fn age_long(age: TimeDelta) -> String {
    let secs = age.num_seconds().max(0);
    let (value, unit) = match secs {
        0..60 => return "just now".to_owned(),
        60..3_600 => (secs / 60, "minute"),
        3_600..86_400 => (secs / 3_600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    let plural = if value == 1 { "" } else { "s" };
    format!("{value} {unit}{plural} ago")
}

/// The first [`SHORT_COMMIT_LEN`] characters of a commit hash, trimmed.
/// Shorter or non-hex values (some providers send tags) are kept as is.
pub fn short_commit(commit: &str) -> String {
    commit.trim().chars().take(SHORT_COMMIT_LEN).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_ages() {
        let cases = [
            (-30, "just now"),
            (0, "just now"),
            (59, "just now"),
            (60, "1 min ago"),
            (5 * 60 + 59, "5 min ago"),
            (3_599, "59 min ago"),
            (3_600, "1 h ago"),
            (86_399, "23 h ago"),
            (86_400, "1 d ago"),
            (10 * 86_400, "10 d ago"),
        ];
        for (secs, expected) in cases {
            assert_eq!(age_short(TimeDelta::seconds(secs)), expected, "{secs}s");
        }
    }

    #[test]
    fn long_ages() {
        let cases = [
            (-5, "just now"),
            (30, "just now"),
            (60, "1 minute ago"),
            (120, "2 minutes ago"),
            (3_600, "1 hour ago"),
            (7_200, "2 hours ago"),
            (86_400, "1 day ago"),
            (3 * 86_400, "3 days ago"),
        ];
        for (secs, expected) in cases {
            assert_eq!(age_long(TimeDelta::seconds(secs)), expected, "{secs}s");
        }
    }

    #[test]
    fn commits_are_shortened() {
        assert_eq!(
            short_commit("a7c412e9f0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5"),
            "a7c412e"
        );
        assert_eq!(short_commit(" abc "), "abc");
        assert_eq!(short_commit(""), "");
    }
}
