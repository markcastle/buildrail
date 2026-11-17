//! Fixed facts and rules: the feed address, the targets, which download
//! addresses are acceptable, size caps, and when a version counts as newer.

use semver::Version;

/// Where every copy of BuildRail looks for updates. GitHub redirects it to
/// `latest.json` on whichever release is currently "latest".
pub const FEED_URL: &str =
    "https://github.com/markcastle/buildrail/releases/latest/download/latest.json";

/// Every manifest asset URL must start with this.
pub const RELEASE_DOWNLOAD_PREFIX: &str =
    "https://github.com/markcastle/buildrail/releases/download/";

/// Hosts a download may end up on after GitHub's redirects.
pub const ALLOWED_DOWNLOAD_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// The platforms a release can carry, as named in `latest.json`.
pub const TARGETS: [&str; 4] = [
    "windows-x86_64",
    "macos-aarch64",
    "macos-x86_64",
    "linux-x86_64",
];

/// Largest `latest.json` the client reads (64 KiB).
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// Largest compressed payload a manifest may announce (200 MiB).
pub const MAX_PAYLOAD_BYTES: u64 = 200 * 1024 * 1024;

/// Largest executable the client will decompress (512 MiB), so a small
/// "gzip bomb" cannot fill memory or disk.
pub const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

/// This build's target name, or `None` when there is no release asset for
/// it (such a build cannot update itself).
pub fn current_target() -> Option<&'static str> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("windows-x86_64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("macos-aarch64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("macos-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-x86_64")
    } else {
        None
    }
}

/// Whether `name` is one of [`TARGETS`].
pub fn is_known_target(name: &str) -> bool {
    TARGETS.contains(&name)
}

/// The release asset name for a payload: `buildrail-v{version}-{target}.gz`.
pub fn payload_file_name(version: &str, target: &str) -> String {
    format!("buildrail-v{version}-{target}.gz")
}

/// Whether `host` is one a release download may be served from.
pub fn is_allowed_download_host(host: &str) -> bool {
    ALLOWED_DOWNLOAD_HOSTS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(host))
}

/// Why an asset URL in a manifest is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    /// Not under BuildRail's GitHub release downloads.
    #[error("the download address is not a BuildRail release on GitHub")]
    NotBuildRailRelease,
    /// Under the prefix, but not exactly `{tag}/{file}` of plain characters.
    #[error("the download address is not a plain release asset address")]
    Malformed,
}

/// Accepts only `https://github.com/markcastle/buildrail/releases/download/{tag}/{file}`
/// where `tag` and `file` are plain names (letters, digits, `.`, `-`, `_`, `+`).
///
/// Strict on purpose: no query strings, fragments, user info, `..`,
/// percent-escapes or extra path segments.
pub fn validate_asset_url(url: &str) -> Result<(), UrlError> {
    let rest = url
        .strip_prefix(RELEASE_DOWNLOAD_PREFIX)
        .ok_or(UrlError::NotBuildRailRelease)?;
    let mut parts = rest.split('/');
    let plain = |segment: &str| {
        !segment.is_empty()
            && !segment.starts_with('.')
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
    };
    match (parts.next(), parts.next(), parts.next()) {
        (Some(tag), Some(file), None) if plain(tag) && plain(file) => Ok(()),
        _ => Err(UrlError::Malformed),
    }
}

/// Whether `candidate` should be offered to a copy running `current`.
///
/// * Only strictly higher versions (by semver precedence, so build
///   metadata is ignored): equal or lower is never offered, which is why a
///   rollback has to be released as a new, higher version.
/// * A pre-release (`0.4.0-rc.1`) is offered only to a copy that is itself
///   running a pre-release; people on stable builds only move to stable
///   builds.
pub fn is_newer(current: &Version, candidate: &Version) -> bool {
    let higher = candidate.cmp_precedence(current).is_gt();
    let channel_ok = candidate.pre.is_empty() || !current.pre.is_empty();
    higher && channel_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn version_policy_table() {
        let cases = [
            ("0.2.0", "0.3.0", true),
            ("0.2.0", "0.2.1", true),
            ("0.2.0", "1.0.0", true),
            ("0.3.0", "0.3.0", false),         // equal
            ("0.3.0", "0.2.9", false),         // downgrade
            ("0.3.0", "0.3.0+build.7", false), // build metadata only
            ("0.2.0", "0.3.0-rc.1", false),    // stable build ignores pre-releases
            ("0.3.0-rc.1", "0.3.0-rc.2", true),
            ("0.3.0-rc.1", "0.3.0", true),
            ("0.3.0-rc.1", "0.4.0-alpha.1", true),
            ("0.3.0-rc.2", "0.3.0-rc.1", false),
        ];
        for (current, candidate, expected) in cases {
            assert_eq!(
                is_newer(&v(current), &v(candidate)),
                expected,
                "{current} -> {candidate}"
            );
        }
    }

    #[test]
    fn current_target_is_known_or_none() {
        if let Some(target) = current_target() {
            assert!(is_known_target(target));
        }
        assert!(!is_known_target("linux-aarch64"));
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        assert_eq!(current_target(), Some("linux-x86_64"));
    }

    #[test]
    fn payload_name() {
        assert_eq!(
            payload_file_name("0.3.0", "macos-aarch64"),
            "buildrail-v0.3.0-macos-aarch64.gz"
        );
    }

    #[test]
    fn hosts() {
        assert!(is_allowed_download_host("github.com"));
        assert!(is_allowed_download_host(
            "Release-Assets.GitHubUserContent.com"
        ));
        assert!(!is_allowed_download_host("evil.example"));
        assert!(!is_allowed_download_host("github.com.evil.example"));
    }

    #[test]
    fn asset_url_rules() {
        let ok = "https://github.com/markcastle/buildrail/releases/download/v0.3.0/buildrail-v0.3.0-linux-x86_64.gz";
        assert_eq!(validate_asset_url(ok), Ok(()));
        let not_ours = [
            "http://github.com/markcastle/buildrail/releases/download/v0.3.0/a.gz",
            "https://github.com/someone/buildrail/releases/download/v0.3.0/a.gz",
            "https://github.com.evil.example/markcastle/buildrail/releases/download/v0.3.0/a.gz",
            "https://evil.example/https://github.com/markcastle/buildrail/releases/download/v/a.gz",
            "https://github.com/markcastle/buildrail-evil/releases/download/v0.3.0/a.gz",
        ];
        for url in not_ours {
            assert_eq!(
                validate_asset_url(url),
                Err(UrlError::NotBuildRailRelease),
                "{url}"
            );
        }
        let malformed = [
            "https://github.com/markcastle/buildrail/releases/download/v0.3.0",
            "https://github.com/markcastle/buildrail/releases/download/v0.3.0/",
            "https://github.com/markcastle/buildrail/releases/download/../x/a.gz",
            "https://github.com/markcastle/buildrail/releases/download/v0.3.0/a/b.gz",
            "https://github.com/markcastle/buildrail/releases/download/v0.3.0/a.gz?x=1",
            "https://github.com/markcastle/buildrail/releases/download/v0.3.0/a.gz#f",
            "https://github.com/markcastle/buildrail/releases/download/v0.3.0/a%2e.gz",
            "https://github.com/markcastle/buildrail/releases/download//a.gz",
        ];
        for url in malformed {
            assert_eq!(validate_asset_url(url), Err(UrlError::Malformed), "{url}");
        }
        assert!(UrlError::Malformed
            .to_string()
            .contains("plain release asset"));
    }
}
