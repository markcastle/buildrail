//! Per-device preferences: where and how the strip is shown, and which local
//! connections exist on this machine. Never exported, never shared.

use std::collections::BTreeMap;

use buildrail_core::domain::{ConnectionAlias, ProviderKind};
use serde::{Deserialize, Serialize};

use crate::format::{read_versioned, DocumentError};
use crate::store::{DocumentStore, StoredDocument};
use crate::validate::{
    check_label, validate_alias, Field, IssueLocation, IssueSink, ValidationErrors,
};

/// Device-preferences format version this build writes and reads.
pub const DEVICE_PREFS_VERSION: u32 = 1;

/// Smallest accepted scale override.
pub const MIN_SCALE: f64 = 0.5;
/// Largest accepted scale override.
pub const MAX_SCALE: f64 = 4.0;

/// Preferences that belong to this machine only.
///
/// Stored as TOML next to (but separate from) the profile; see the crate
/// docs for an example. Unknown keys are ignored so a file touched by a
/// newer build of the same format version still loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevicePrefs {
    /// Start BuildRail when the user logs in.
    pub launch_at_startup: bool,
    /// Colour theme.
    pub theme: Theme,
    /// Overrides the monitor's scale factor (for example `1.25`). `None`
    /// follows the OS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// The chosen monitor. `None` means the primary monitor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor: Option<MonitorChoice>,
    /// Docked or floating.
    pub placement: Placement,
    /// Local provider connections by alias. Secrets are *not* here: they
    /// live in the OS credential store under the same alias.
    pub connections: BTreeMap<ConnectionAlias, LocalConnection>,
}

impl Default for DevicePrefs {
    fn default() -> Self {
        Self {
            launch_at_startup: false,
            theme: Theme::System,
            scale: None,
            monitor: None,
            placement: Placement::DockedBottom,
            connections: BTreeMap::new(),
        }
    }
}

/// Colour theme preference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the OS setting.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

/// Which monitor to show the strip on.
///
/// Monitor IDs are not always stable (docks, driver updates), so the
/// platform layer should match `id` first, then `name`, and finally fall
/// back to the primary monitor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorChoice {
    /// Platform monitor identifier.
    pub id: String,
    /// Human-readable monitor name, used as the fallback match and in the
    /// settings UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// How the strip is placed on its monitor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum Placement {
    /// Docked along the bottom edge of the work area.
    #[default]
    DockedBottom,
    /// A free-floating window.
    Floating {
        /// Last position, in logical pixels relative to the monitor's
        /// top-left corner. `None` until the user has moved it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<Position>,
    },
}

/// A point in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// Horizontal offset.
    pub x: i32,
    /// Vertical offset.
    pub y: i32,
}

/// A provider connection set up on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalConnection {
    /// Which provider the stored credential is for.
    pub provider: ProviderKind,
    /// Who the credential belongs to, for display only (GitHub login,
    /// Cloudflare account email).
    pub display_name: String,
}

impl DevicePrefs {
    /// The connection registered under `alias`.
    pub fn connection(&self, alias: &ConnectionAlias) -> Option<&LocalConnection> {
        self.connections.get(alias)
    }

    /// Local aliases for `provider`, for a "choose connection" list.
    pub fn connections_for(&self, provider: ProviderKind) -> Vec<&ConnectionAlias> {
        self.connections
            .iter()
            .filter(|(_, connection)| connection.provider == provider)
            .map(|(alias, _)| alias)
            .collect()
    }

    /// Checks every rule; all problems are reported together.
    pub fn validate(&self) -> Result<(), ValidationErrors> {
        let mut issues = Vec::new();
        if let Some(scale) = self.scale {
            if !(MIN_SCALE..=MAX_SCALE).contains(&scale) {
                let mut sink = IssueSink::new(IssueLocation::Document);
                sink.push(
                    Field::Scale,
                    format!("scale must be between {MIN_SCALE} and {MAX_SCALE}"),
                );
                issues.append(&mut sink.issues);
            }
        }
        for (alias, connection) in &self.connections {
            let mut sink = IssueSink::new(IssueLocation::Connection {
                alias: alias.as_str().to_owned(),
            });
            match validate_alias(alias.as_str()) {
                Ok(clean) if &clean == alias => {}
                Ok(_) => sink.push(
                    Field::Connection,
                    "connection alias must not have surrounding spaces",
                ),
                Err(message) => sink.push(Field::Connection, message),
            }
            let name = connection.display_name.trim();
            if name.is_empty() {
                sink.push(Field::DisplayName, "display_name must not be empty");
            } else if let Err(message) = check_label(name) {
                sink.push(Field::DisplayName, message.replace("label", "display_name"));
            }
            issues.append(&mut sink.issues);
        }
        ValidationErrors::new(issues).into_result()
    }
}

impl StoredDocument for DevicePrefs {
    fn initial() -> Self {
        Self::default()
    }

    fn decode(text: &str) -> Result<Self, DocumentError> {
        let (mut table, version) = read_versioned(text, DEVICE_PREFS_VERSION)?;
        table.remove("version");
        // Migration seam, as for the profile.
        let prefs: DevicePrefs = match version {
            1 => toml::Value::Table(table)
                .try_into()
                .map_err(|err: toml::de::Error| {
                    DocumentError::Syntax(err.to_string().trim().to_owned())
                })?,
            _ => {
                return Err(DocumentError::UnsupportedVersion {
                    found: i64::from(version),
                    supported: DEVICE_PREFS_VERSION,
                })
            }
        };
        prefs.validate()?;
        Ok(prefs)
    }

    fn encode(&self) -> Result<String, DocumentError> {
        self.validate()?;
        let body = toml::to_string(self).map_err(|err| DocumentError::Encode(err.to_string()))?;
        Ok(format!(
            "# BuildRail device preferences (this machine only; not exported).\n\
             version = {DEVICE_PREFS_VERSION}\n{body}"
        ))
    }
}

/// The store for this machine's preferences.
pub type DevicePrefsStore = DocumentStore<DevicePrefs>;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::store::StoreError;

    fn full() -> DevicePrefs {
        let mut prefs = DevicePrefs {
            launch_at_startup: true,
            theme: Theme::Dark,
            scale: Some(1.25),
            monitor: Some(MonitorChoice {
                id: r"\\.\DISPLAY2".into(),
                name: Some("DELL U2720Q".into()),
            }),
            placement: Placement::Floating {
                position: Some(Position { x: -1200, y: 40 }),
            },
            connections: BTreeMap::new(),
        };
        prefs.connections.insert(
            ConnectionAlias::new("work-github"),
            LocalConnection {
                provider: ProviderKind::GitHub,
                display_name: "octocat".into(),
            },
        );
        prefs.connections.insert(
            ConnectionAlias::new("acme-cf"),
            LocalConnection {
                provider: ProviderKind::Cloudflare,
                display_name: "ops@acme.example".into(),
            },
        );
        prefs
    }

    #[test]
    fn round_trips() {
        for prefs in [
            DevicePrefs::default(),
            full(),
            DevicePrefs {
                placement: Placement::Floating { position: None },
                ..DevicePrefs::default()
            },
        ] {
            let text = prefs.encode().unwrap();
            assert!(text.contains("version = 1"), "{text}");
            assert_eq!(DevicePrefs::decode(&text).unwrap(), prefs, "{text}");
        }
    }

    #[test]
    fn readable_format() {
        let text = r#"
version = 1
launch_at_startup = true
theme = "light"

[placement]
mode = "floating"
position = { x = 10, y = 20 }

[connections.work-github]
provider = "github"
display_name = "octocat"

[future]
ignored = true
"#;
        let prefs = DevicePrefs::decode(text).unwrap();
        assert_eq!(prefs.theme, Theme::Light);
        assert_eq!(
            prefs.placement,
            Placement::Floating {
                position: Some(Position { x: 10, y: 20 })
            }
        );
        assert_eq!(
            prefs.connections_for(ProviderKind::GitHub),
            [&ConnectionAlias::new("work-github")]
        );
        assert!(prefs.connections_for(ProviderKind::Cloudflare).is_empty());
        assert!(prefs
            .connection(&ConnectionAlias::new("work-github"))
            .is_some());
    }

    #[test]
    fn minimal_file_uses_defaults() {
        assert_eq!(
            DevicePrefs::decode("version = 1").unwrap(),
            DevicePrefs::default()
        );
    }

    #[test]
    fn rejects_bad_files() {
        assert!(matches!(
            DevicePrefs::decode("version = 1\ntheme = \"neon\""),
            Err(DocumentError::Syntax(_))
        ));
        assert!(matches!(
            DevicePrefs::decode("version = 9"),
            Err(DocumentError::UnsupportedVersion { found: 9, .. })
        ));
        assert!(matches!(
            DevicePrefs::decode("version = 1\nscale = 9.0"),
            Err(DocumentError::Invalid(_))
        ));
    }

    #[test]
    fn validation_collects_all() {
        let mut prefs = full();
        prefs.scale = Some(f64::NAN);
        prefs.connections.insert(
            ConnectionAlias::new("bad alias"),
            LocalConnection {
                provider: ProviderKind::GitHub,
                display_name: " ".into(),
            },
        );
        prefs.connections.insert(
            ConnectionAlias::new(" padded"),
            LocalConnection {
                provider: ProviderKind::GitHub,
                display_name: "x\u{0}".into(),
            },
        );
        let err = prefs.validate().unwrap_err();
        let lines: Vec<String> = err.issues().iter().map(ToString::to_string).collect();
        assert_eq!(
            lines,
            vec![
                "scale must be between 0.5 and 4",
                "connection \" padded\": connection alias must not have surrounding spaces",
                "connection \" padded\": display_name must not contain control characters",
                "connection \"bad alias\": connection \"bad alias\" must start with a letter or digit and contain only letters, digits, '.', '-' and '_'",
                "connection \"bad alias\": display_name must not be empty",
            ]
        );
    }

    #[test]
    fn invalid_prefs_are_not_saved() {
        let dir = tempfile::tempdir().unwrap();
        let store = DevicePrefsStore::new(dir.path().join("device.toml"));
        assert_eq!(store.load().unwrap(), DevicePrefs::default());
        store.save(&full()).unwrap();
        let before = std::fs::read(store.path()).unwrap();
        let mut bad = full();
        bad.scale = Some(0.1);
        let err = store.save(&bad).unwrap_err();
        assert!(matches!(err, StoreError::Document { .. }));
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert_eq!(store.load().unwrap(), full());
    }
}
