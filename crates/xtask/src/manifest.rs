//! The version in the root `Cargo.toml`.
//!
//! Every crate takes its version from `[workspace.package]`, so that one
//! line is the only place the number lives in the source. These functions
//! edit that line and nothing else, so comments, ordering and spacing in the
//! rest of the file survive a release untouched.

use crate::version::Version;

const TABLE: &str = "workspace.package";

/// Where the version string sits: the line, and the byte range of the text
/// between its quotes.
struct Found {
    line: usize,
    start: usize,
    end: usize,
}

fn is_table_header(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

fn table_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('[')?;
    if inner.starts_with('[') {
        return None;
    }
    let close = inner.find(']')?;
    Some(
        inner[..close]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect(),
    )
}

fn locate(lines: &[&str]) -> Result<Found, String> {
    let header = lines
        .iter()
        .position(|l| table_name(l).as_deref() == Some(TABLE))
        .ok_or_else(|| "Cargo.toml has no [workspace.package] table.".to_string())?;
    for (offset, line) in lines[header + 1..].iter().enumerate() {
        if is_table_header(line) {
            break;
        }
        let trimmed = line.trim_start();
        let Some(after_key) = trimmed.strip_prefix("version") else {
            continue;
        };
        let after_key = after_key.trim_start();
        let Some(value) = after_key.strip_prefix('=') else {
            // `versioning = ...` or `version.workspace = ...`, not ours.
            continue;
        };
        let value = value.trim_start();
        let Some(quoted) = value.strip_prefix('"') else {
            return Err(format!(
                "Cargo.toml's [workspace.package] version is not a quoted string: {}",
                line.trim()
            ));
        };
        let Some(close) = quoted.find('"') else {
            return Err(format!(
                "Cargo.toml's version line has no closing quote: {}",
                line.trim()
            ));
        };
        let start = line.len() - quoted.len();
        return Ok(Found {
            line: header + 1 + offset,
            start,
            end: start + close,
        });
    }
    Err("Cargo.toml's [workspace.package] table has no version.".to_string())
}

/// The version `[workspace.package]` holds.
pub fn workspace_version(text: &str) -> Result<Version, String> {
    let lines: Vec<&str> = text.lines().collect();
    let found = locate(&lines)?;
    Version::parse(&lines[found.line][found.start..found.end])
}

/// The same file with `[workspace.package] version` set to `version`, and
/// every other byte as it was.
pub fn set_workspace_version(text: &str, version: Version) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let found = locate(&lines)?;
    let line = lines[found.line];
    let replaced = format!("{}{version}{}", &line[..found.start], &line[found.end..]);
    let mut out: Vec<&str> = lines.clone();
    out[found.line] = &replaced;
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const TOML: &str = r#"[workspace]
resolver = "2"
members = ["crates/*"]

# The one place the version lives.
[workspace.package]
edition = "2021"
version   =   "0.2.0"   # set by cargo xtask release
rust-version = "1.95"

[workspace.dependencies]
version = "9.9.9"
serde = { version = "1" }
"#;

    #[test]
    fn reads_the_workspace_version() {
        assert_eq!(workspace_version(TOML).unwrap(), Version::new(0, 2, 0));
    }

    #[test]
    fn sets_only_that_string_and_keeps_everything_else() {
        let out = set_workspace_version(TOML, Version::new(0, 3, 0)).unwrap();
        assert_eq!(out, TOML.replace("\"0.2.0\"", "\"0.3.0\""));
        assert!(out.contains("version   =   \"0.3.0\"   # set by cargo xtask release\n"));
        assert!(out.contains("version = \"9.9.9\""));
        assert_eq!(workspace_version(&out).unwrap(), Version::new(0, 3, 0));
    }

    #[test]
    fn keeps_a_missing_final_newline_missing() {
        let text = "[workspace.package]\nversion = \"0.1.0\"";
        assert_eq!(
            set_workspace_version(text, Version::new(1, 0, 0)).unwrap(),
            "[workspace.package]\nversion = \"1.0.0\""
        );
    }

    #[test]
    fn accepts_spaces_inside_the_table_header() {
        let text = "[ workspace.package ] # comment\nversion=\"0.1.0\"\n";
        assert_eq!(
            set_workspace_version(text, Version::new(0, 1, 1)).unwrap(),
            "[ workspace.package ] # comment\nversion=\"0.1.1\"\n"
        );
    }

    #[test]
    fn ignores_keys_that_only_start_with_version() {
        let text = "[workspace.package]\nversioning = \"x\"\nversion.workspace = true\nversion = \"0.1.0\"\n";
        assert_eq!(workspace_version(text).unwrap(), Version::new(0, 1, 0));
    }

    #[test]
    fn explains_what_is_missing() {
        assert!(workspace_version("[package]\nversion = \"0.1.0\"\n")
            .unwrap_err()
            .contains("no [workspace.package]"));
        assert!(workspace_version("[[workspace.package]]\n")
            .unwrap_err()
            .contains("no [workspace.package]"));
        assert!(workspace_version(
            "[workspace.package]\nedition = \"2021\"\n[x]\nversion = \"1.0.0\"\n"
        )
        .unwrap_err()
        .contains("has no version"));
        assert!(workspace_version("[workspace.package]\nversion = 1\n")
            .unwrap_err()
            .contains("not a quoted string"));
        assert!(
            workspace_version("[workspace.package]\nversion = \"0.1.0\n")
                .unwrap_err()
                .contains("no closing quote")
        );
        assert!(
            workspace_version("[workspace.package]\nversion = \"next\"\n")
                .unwrap_err()
                .contains("is not a version")
        );
        assert!(set_workspace_version("", Version::new(0, 1, 0)).is_err());
    }
}
