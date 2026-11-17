//! `latest.json`: the update feed's one file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The manifest schema this crate writes and understands.
pub const SCHEMA_VERSION: u32 = 1;

/// The whole of `latest.json`. Unknown fields are ignored when reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Format version; see [`SCHEMA_VERSION`].
    pub schema: u32,
    /// The release's semantic version, without a leading `v`.
    pub version: String,
    /// The release page, which carries the changelog section.
    pub notes_url: String,
    /// Id of the key that signed the payloads ([`crate::PublicKey::key_id`]).
    pub key_id: String,
    /// One entry per platform, keyed by target name ([`crate::TARGETS`]).
    pub targets: BTreeMap<String, TargetEntry>,
}

/// One platform's payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetEntry {
    /// Release asset address (must pass [`crate::validate_asset_url`]).
    pub url: String,
    /// Lowercase hex SHA-256 of the `.gz` asset.
    pub sha256: String,
    /// Size of the `.gz` asset in bytes.
    pub size: u64,
    /// Base64 Ed25519 signature of [`crate::signed_message`].
    pub signature: String,
}

/// Why `latest.json` could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// Written for a newer updater than this one.
    #[error("the update feed uses format {0}, which needs a newer BuildRail; download the latest release by hand")]
    UnsupportedSchema(u64),
    /// Not JSON, or missing required fields.
    #[error("the update feed is not valid ({0})")]
    Malformed(String),
}

#[derive(Deserialize)]
struct SchemaProbe {
    schema: u64,
}

impl Manifest {
    /// Parses `latest.json`, checking the schema before anything else so a
    /// future format is reported as "needs a newer updater" rather than as
    /// a parse error.
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        let malformed = |e: serde_json::Error| ManifestError::Malformed(e.to_string());
        let probe: SchemaProbe = serde_json::from_slice(bytes).map_err(malformed)?;
        if probe.schema > u64::from(SCHEMA_VERSION) {
            return Err(ManifestError::UnsupportedSchema(probe.schema));
        }
        if probe.schema < u64::from(SCHEMA_VERSION) {
            return Err(ManifestError::Malformed(format!(
                "unknown format {}",
                probe.schema
            )));
        }
        serde_json::from_slice(bytes).map_err(malformed)
    }

    /// Pretty JSON with a trailing newline, as `buildrail-sign manifest` prints it.
    pub fn to_json(&self) -> String {
        // Serialising plain strings, numbers and maps cannot fail.
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "schema": 1,
        "version": "0.3.0",
        "notes_url": "https://github.com/markcastle/buildrail/releases/tag/v0.3.0",
        "key_id": "0123456789abcdef",
        "future_field": {"anything": true},
        "targets": {
            "windows-x86_64": {
                "url": "https://github.com/markcastle/buildrail/releases/download/v0.3.0/buildrail-v0.3.0-windows-x86_64.gz",
                "sha256": "00", "size": 1234567, "signature": "c2ln", "extra": 1
            }
        }
    }"#;

    #[test]
    fn parses_the_documented_shape_and_ignores_unknown_fields() {
        let manifest = Manifest::parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(manifest.version, "0.3.0");
        assert_eq!(manifest.targets["windows-x86_64"].size, 1_234_567);
        let again = Manifest::parse(manifest.to_json().as_bytes()).unwrap();
        assert_eq!(again, manifest);
        assert!(manifest.to_json().ends_with("}\n"));
    }

    #[test]
    fn newer_schema_is_its_own_error() {
        let text = SAMPLE.replace("\"schema\": 1", "\"schema\": 2");
        let err = Manifest::parse(text.as_bytes()).unwrap_err();
        assert_eq!(err, ManifestError::UnsupportedSchema(2));
        assert!(err.to_string().contains("needs a newer BuildRail"));
        // Even if the rest of a future format is unreadable to us.
        assert_eq!(
            Manifest::parse(br#"{"schema": 7, "totally": "different"}"#),
            Err(ManifestError::UnsupportedSchema(7))
        );
    }

    #[test]
    fn malformed_inputs() {
        for bad in [
            &b"not json"[..],
            br#"{"version": "1.0.0"}"#,
            br#"{"schema": 0}"#,
            br#"{"schema": 1}"#,
        ] {
            assert!(matches!(
                Manifest::parse(bad),
                Err(ManifestError::Malformed(_))
            ));
        }
    }
}
