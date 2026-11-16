//! Reading and writing the profile TOML (`version = 1`, ordered `[[watch]]`
//! tables). See the crate docs for the format itself.

use thiserror::Error;
use toml::{Table, Value};

use crate::profile::Profile;
use crate::validate::{Field, IssueLocation, IssueSink, ValidationErrors, WatchDraft};

/// The profile format version this build writes and the newest it reads.
pub const PROFILE_VERSION: u32 = 1;

/// Why a TOML document (profile or device preferences) could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DocumentError {
    /// Not TOML at all, or the wrong shape (for example `watch` that is not
    /// a list of tables). The message includes line and column when known.
    #[error("not a valid BuildRail file: {0}")]
    Syntax(String),
    /// The top-level `version` key is absent.
    #[error("the file has no `version` key")]
    MissingVersion,
    /// `version` is not a positive integer.
    #[error("`version` must be a positive whole number")]
    InvalidVersion,
    /// Written by a newer BuildRail. Never guessed at: update the app.
    #[error("this file uses format version {found}, but this BuildRail only understands up to version {supported}; please update BuildRail")]
    UnsupportedVersion {
        /// Version in the file.
        found: i64,
        /// Newest version this build reads.
        supported: u32,
    },
    /// Well-formed TOML whose contents failed validation.
    #[error("{0}")]
    Invalid(#[from] ValidationErrors),
    /// The value could not be encoded (not expected in practice).
    #[error("could not encode: {0}")]
    Encode(String),
}

/// Parses TOML into a table and returns it with its declared version
/// (checked against `supported`). Shared by every versioned document.
pub(crate) fn read_versioned(text: &str, supported: u32) -> Result<(Table, u32), DocumentError> {
    let table: Table = text
        .parse()
        .map_err(|err: toml::de::Error| DocumentError::Syntax(err.to_string().trim().to_owned()))?;
    let version = match table.get("version") {
        None => return Err(DocumentError::MissingVersion),
        Some(value) => value.as_integer().ok_or(DocumentError::InvalidVersion)?,
    };
    if version > i64::from(supported) {
        return Err(DocumentError::UnsupportedVersion {
            found: version,
            supported,
        });
    }
    let version = u32::try_from(version)
        .ok()
        .filter(|v| *v >= 1)
        .ok_or(DocumentError::InvalidVersion)?;
    Ok((table, version))
}

/// Parses and validates profile TOML.
///
/// All validation problems are reported together in
/// [`DocumentError::Invalid`]; unknown keys count as problems so typos such
/// as `lable = "x"` do not silently drop data.
pub fn parse_profile(text: &str) -> Result<Profile, DocumentError> {
    let (table, version) = read_versioned(text, PROFILE_VERSION)?;
    // Migration seam: when version 2 exists, convert older tables here
    // before reading them as the current shape.
    let drafts = match version {
        1 => drafts_v1(&table)?,
        _ => {
            return Err(DocumentError::UnsupportedVersion {
                found: i64::from(version),
                supported: PROFILE_VERSION,
            })
        }
    };
    Ok(Profile::from_drafts(&drafts)?)
}

/// Reads the raw drafts of a version-1 profile. Structural problems
/// (unknown keys, non-string values) are returned together with any
/// validation problems of the same document.
fn drafts_v1(table: &Table) -> Result<Vec<WatchDraft>, DocumentError> {
    let mut document = IssueSink::new(IssueLocation::Document);
    for key in table.keys() {
        if key != "version" && key != "watch" {
            document.push(
                Field::Key(key.clone()),
                format!(
                    "unknown top-level key {key:?} (expected `version` and `[[watch]]` tables)"
                ),
            );
        }
    }
    let entries: &[Value] = match table.get("watch") {
        None => &[],
        Some(Value::Array(entries)) => entries,
        Some(_) => {
            return Err(DocumentError::Syntax(
                "`watch` must be written as [[watch]] tables".into(),
            ))
        }
    };
    let mut issues = document.issues;
    let mut drafts = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let Value::Table(entry) = entry else {
            return Err(DocumentError::Syntax(
                "`watch` must be written as [[watch]] tables".into(),
            ));
        };
        let label = entry
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_owned);
        let mut sink = IssueSink::new(IssueLocation::Watch { index, label });
        drafts.push(draft_from_table(entry, &mut sink));
        issues.append(&mut sink.issues);
    }
    if issues.is_empty() {
        Ok(drafts)
    } else {
        // Structural problems are reported together with any validation
        // problems in the well-formed watches, so one pass shows everything.
        if let Err(more) = Profile::from_drafts(&drafts) {
            issues.extend(more.issues().iter().cloned());
        }
        Err(ValidationErrors::new(issues).into())
    }
}

fn draft_from_table(entry: &Table, sink: &mut IssueSink) -> WatchDraft {
    let mut draft = WatchDraft::default();
    for (key, value) in entry {
        let slot = match key.as_str() {
            "id" => &mut draft.id,
            "label" => &mut draft.label,
            "provider" => &mut draft.provider,
            "connection" => &mut draft.connection,
            "owner" => &mut draft.owner,
            "repo" => &mut draft.repo,
            "workflow" => &mut draft.workflow,
            "account_id" => &mut draft.account_id,
            "worker" => &mut draft.worker,
            "branch" => &mut draft.branch,
            _ => {
                sink.push(Field::Key(key.clone()), format!("unknown key {key:?}"));
                continue;
            }
        };
        match value {
            Value::String(text) => *slot = Some(text.clone()),
            // A numeric workflow ID is naturally written without quotes.
            Value::Integer(number) if key == "workflow" => *slot = Some(number.to_string()),
            other => sink.push(
                Field::Key(key.clone()),
                format!("{key} must be a string, not {}", other.type_str()),
            ),
        }
    }
    draft
}

/// Serialises a profile as TOML: a short header comment, `version`, then one
/// `[[watch]]` table per watch in order.
///
/// The output contains exactly the keys listed in the crate docs. There is
/// no secret anywhere in [`Profile`], so an export cannot carry credentials.
pub fn profile_to_toml(profile: &Profile) -> String {
    let mut out = String::from(
        "# BuildRail profile. Watches appear on the strip in this order.\n\
         # Credentials are never stored here; `connection` names a local\n\
         # connection that each machine maps to its own credential.\n",
    );
    out.push_str(&format!("version = {PROFILE_VERSION}\n"));
    for watch in profile.watches() {
        let draft = WatchDraft::from(watch);
        out.push_str("\n[[watch]]\n");
        let fields = [
            ("id", &draft.id),
            ("label", &draft.label),
            ("provider", &draft.provider),
            ("connection", &draft.connection),
            ("owner", &draft.owner),
            ("repo", &draft.repo),
            ("workflow", &draft.workflow),
            ("account_id", &draft.account_id),
            ("worker", &draft.worker),
            ("branch", &draft.branch),
        ];
        for (key, value) in fields {
            if let Some(value) = value {
                out.push_str(&format!("{key} = {}\n", Value::String(value.clone())));
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::BTreeSet;

    use buildrail_core::domain::WatchTarget;

    use super::*;

    const SAMPLE: &str = r#"
version = 1

[[watch]]
id = "api-ci"
label = "API CI"
provider = "github"
connection = "work-github"
owner = "acme"
repo = "api"
workflow = "ci.yml"
branch = "main"

[[watch]]
id = "site"
label = "Marketing site"
provider = "cloudflare"
connection = "acme-cf"
account_id = "0123456789abcdef0123456789abcdef"
worker = "marketing-site"

[[watch]]
label = "Nightly"
provider = "github"
connection = "work-github"
owner = "acme"
repo = "api"
workflow = 1234
"#;

    fn invalid(text: &str) -> ValidationErrors {
        match parse_profile(text) {
            Err(DocumentError::Invalid(errors)) => errors,
            other => panic!("expected validation errors, got {other:?}"),
        }
    }

    #[test]
    fn parses_sample_in_order() {
        let profile = parse_profile(SAMPLE).unwrap();
        let labels: Vec<&str> = profile.watches().iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["API CI", "Marketing site", "Nightly"]);
        assert_eq!(profile.watches()[0].target.branch(), Some("main"));
        assert!(matches!(
            &profile.watches()[2].target,
            WatchTarget::GitHub { workflow, .. } if workflow == "1234"
        ));
        assert!(uuid::Uuid::parse_str(profile.watches()[2].id.as_str()).is_ok());
    }

    #[test]
    fn empty_profile_document() {
        assert!(parse_profile("version = 1").unwrap().is_empty());
    }

    #[test]
    fn round_trip() {
        let profile = parse_profile(SAMPLE).unwrap();
        let text = profile_to_toml(&profile);
        assert_eq!(parse_profile(&text).unwrap(), profile);
        assert!(text.starts_with("# BuildRail profile"));
    }

    #[test]
    fn export_contains_only_expected_keys() {
        let text = profile_to_toml(&parse_profile(SAMPLE).unwrap());
        let table: Table = text.parse().unwrap();
        let top: BTreeSet<&str> = table.keys().map(String::as_str).collect();
        assert_eq!(top, BTreeSet::from(["version", "watch"]));
        let allowed = BTreeSet::from([
            "id",
            "label",
            "provider",
            "connection",
            "owner",
            "repo",
            "workflow",
            "account_id",
            "worker",
            "branch",
        ]);
        for watch in table["watch"].as_array().unwrap() {
            for (key, value) in watch.as_table().unwrap() {
                assert!(allowed.contains(key.as_str()), "unexpected key {key}");
                assert!(value.is_str());
            }
        }
        for forbidden in ["token", "secret", "password", "credential"] {
            assert!(!text.to_lowercase().contains(&format!("{forbidden} =")));
        }
    }

    #[test]
    fn escapes_awkward_labels() {
        let text = SAMPLE.replace("\"API CI\"", r#""Quote \" and \\ back""#);
        let profile = parse_profile(&text).unwrap();
        let again = parse_profile(&profile_to_toml(&profile)).unwrap();
        assert_eq!(again.watches()[0].label, "Quote \" and \\ back");
    }

    #[test]
    fn malformed_toml() {
        let err = parse_profile("version = 1\n[[watch]\nid = ").unwrap_err();
        let DocumentError::Syntax(message) = &err else {
            panic!("{err:?}")
        };
        assert!(message.contains("line"), "{message}");
        assert!(err.to_string().starts_with("not a valid BuildRail file"));
    }

    #[test]
    fn version_handling() {
        assert_eq!(
            parse_profile("").unwrap_err(),
            DocumentError::MissingVersion
        );
        assert_eq!(
            parse_profile("version = \"1\"").unwrap_err(),
            DocumentError::InvalidVersion
        );
        assert_eq!(
            parse_profile("version = 0").unwrap_err(),
            DocumentError::InvalidVersion
        );
        assert_eq!(
            parse_profile("version = -3").unwrap_err(),
            DocumentError::InvalidVersion
        );
        let err = parse_profile("version = 2\n[[watch]]\nshiny = true").unwrap_err();
        assert_eq!(
            err,
            DocumentError::UnsupportedVersion {
                found: 2,
                supported: 1
            }
        );
        assert!(err.to_string().contains("please update BuildRail"));
    }

    #[test]
    fn wrong_shapes() {
        for text in ["version = 1\nwatch = 3", "version = 1\nwatch = [1, 2]"] {
            assert!(matches!(parse_profile(text), Err(DocumentError::Syntax(_))));
        }
    }

    #[test]
    fn unknown_keys_and_types_are_collected_with_validation_errors() {
        let text = r#"
version = 1
watches = []

[[watch]]
id = "a"
lable = "typo"
provider = "github"
connection = "work"
owner = "acme"
repo = ""
workflow = "ci.yml"
branch = 7

[[watch]]
id = "a"
label = "Dup"
provider = "github"
connection = "work"
owner = "acme"
repo = "api"
workflow = "ci.yml"
"#;
        let errors = invalid(text);
        let lines: Vec<String> = errors.issues().iter().map(ToString::to_string).collect();
        assert_eq!(
            lines,
            vec![
                "unknown top-level key \"watches\" (expected `version` and `[[watch]]` tables)",
                "watch 1: branch must be a string, not integer",
                "watch 1: unknown key \"lable\"",
                "watch 1: label is required",
                "watch 1: repo must not be empty",
            ]
        );
        assert_eq!(errors.issues()[2].field, Field::Key("lable".into()));
    }

    #[test]
    fn duplicate_ids_rejected() {
        let text = SAMPLE.replace("id = \"site\"", "id = \"api-ci\"");
        let errors = invalid(&text);
        assert_eq!(
            errors.to_string(),
            "watch 2 (\"Marketing site\"): id \"api-ci\" is already used by watch 1"
        );
    }
}
