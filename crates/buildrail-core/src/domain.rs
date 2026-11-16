//! The shared vocabulary of BuildRail.
//!
//! Everything here is plain data. Behaviour that needs judgement (which run
//! wins, when to poll) lives in [`crate::selection`] and [`crate::refresh`].

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The CI services BuildRail can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// GitHub Actions workflow runs.
    #[serde(rename = "github")]
    GitHub,
    /// Cloudflare Workers Builds.
    Cloudflare,
}

impl ProviderKind {
    /// Human readable provider name, used in labels and accessible text.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::Cloudflare => "Cloudflare",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps a raw string. Validation of user-supplied values happens
            /// in the profile crate, not here.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// The raw string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(
    /// Stable identifier of a pinned watch. Survives rename, reorder and
    /// export/import, so cached state and UI positions can follow the watch.
    WatchId
);
string_id!(
    /// Name of a *local* provider connection ("work-github"). Profiles carry
    /// the alias only; each machine maps it to its own stored credential.
    ConnectionAlias
);
string_id!(
    /// Provider-assigned identifier of one run / build. Opaque to core.
    RunId
);

/// What a watch points at, in the provider's own terms.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum WatchTarget {
    /// One GitHub Actions workflow in one repository.
    #[serde(rename = "github")]
    GitHub {
        /// Repository owner (user or organisation).
        owner: String,
        /// Repository name.
        repo: String,
        /// Workflow file name (`ci.yml`) or numeric workflow ID.
        workflow: String,
        /// Only consider runs on this branch. `None` means any branch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
    /// One Worker's builds in one Cloudflare account.
    Cloudflare {
        /// Cloudflare account ID.
        account_id: String,
        /// Worker script name as shown in the dashboard.
        worker: String,
        /// Only consider builds of this branch. `None` means any branch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
}

impl WatchTarget {
    /// Which provider serves this target.
    pub fn provider(&self) -> ProviderKind {
        match self {
            Self::GitHub { .. } => ProviderKind::GitHub,
            Self::Cloudflare { .. } => ProviderKind::Cloudflare,
        }
    }

    /// The optional branch filter.
    pub fn branch(&self) -> Option<&str> {
        match self {
            Self::GitHub { branch, .. } | Self::Cloudflare { branch, .. } => branch.as_deref(),
        }
    }
}

/// One pinned entry on the strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watch {
    /// Stable identity.
    pub id: WatchId,
    /// Short name shown on the strip.
    pub label: String,
    /// Which local connection supplies the credential.
    pub connection: ConnectionAlias,
    /// What to watch.
    pub target: WatchTarget,
}

/// Identifies one distinct provider request. Two watches with an equal key
/// are served by a single request (see [`crate::refresh`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestKey {
    /// Connection whose credential is used.
    pub connection: ConnectionAlias,
    /// Target being asked about.
    pub target: WatchTarget,
}

impl From<&Watch> for RequestKey {
    fn from(watch: &Watch) -> Self {
        Self {
            connection: watch.connection.clone(),
            target: watch.target.clone(),
        }
    }
}

/// Provider-neutral status of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildStatus {
    /// Accepted by the provider, not started.
    Queued,
    /// In progress.
    Running,
    /// Finished successfully (CI).
    Passed,
    /// Finished successfully and the provider confirmed the deployment.
    Deployed,
    /// Finished unsuccessfully.
    Failed,
    /// Stopped before completion by a person or a newer run.
    Cancelled,
    /// The provider decided not to run it.
    Skipped,
    /// The provider reported something we cannot map to the above.
    Unavailable,
}

/// The colour family a status belongs to. The UI maps this to actual colours;
/// core only decides the meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    /// Passed or confirmed deployed.
    Green,
    /// Queued or running.
    Amber,
    /// Failed.
    Red,
    /// Cancelled, skipped, unavailable.
    Neutral,
}

impl BuildStatus {
    /// Colour family for this status.
    pub fn tone(self) -> Tone {
        match self {
            Self::Passed | Self::Deployed => Tone::Green,
            Self::Queued | Self::Running => Tone::Amber,
            Self::Failed => Tone::Red,
            Self::Cancelled | Self::Skipped | Self::Unavailable => Tone::Neutral,
        }
    }

    /// `true` while the run can still change state. Drives the faster
    /// polling cadence.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    /// The word shown next to the label. Each neutral status has its own
    /// word so they are distinguishable without colour.
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Building",
            Self::Passed => "Passed",
            Self::Deployed => "Deployed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Skipped => "Skipped",
            Self::Unavailable => "Unavailable",
        }
    }
}

/// The common record every provider returns for one run attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRecord {
    /// Provider's stable run identifier.
    pub run_id: RunId,
    /// Attempt number, starting at 1. A re-run keeps `run_id` and bumps this.
    pub attempt: u32,
    /// Normalised status.
    pub status: BuildStatus,
    /// Page to open when the entry is activated.
    pub url: String,
    /// Branch the run was for, when the provider says.
    pub branch: Option<String>,
    /// Commit SHA (full or short) when the provider says.
    pub commit: Option<String>,
    /// Name of the first failing job / step, when known.
    pub failing_job: Option<String>,
    /// When the provider created the run (source timestamp).
    pub created_at: DateTime<Utc>,
    /// When the provider last changed the run (source timestamp).
    pub updated_at: DateTime<Utc>,
    /// When *we* last successfully saw this record. Set by the provider
    /// adapter from the injected clock at the moment the response arrived.
    pub observed_at: DateTime<Utc>,
}

/// How much to trust what is on screen for a watch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Freshness {
    /// Nothing fetched yet.
    Pending,
    /// The last refresh succeeded.
    Fresh,
    /// The last refresh failed; any record shown is the cached one.
    Stale {
        /// Why the refresh failed.
        reason: StaleReason,
    },
}

/// Why a watch is showing stale (or no) data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleReason {
    /// Network unreachable or timed out.
    Offline,
    /// The credential is missing, expired or lacks permission. Needs
    /// "Reconnect" in settings.
    AccessDenied,
    /// The target no longer exists (or is hidden from this credential).
    NotFound,
    /// The provider asked us to slow down.
    RateLimited,
    /// The provider failed or sent something we could not read.
    ProviderError,
}

/// Everything the UI needs to draw one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchSnapshot {
    /// Which watch this describes.
    pub watch_id: WatchId,
    /// Latest selected run. `None` with [`Freshness::Fresh`] means the
    /// target exists but has no matching runs (empty history) – this is
    /// deliberately different from an access error.
    pub record: Option<BuildRecord>,
    /// Whether `record` is current.
    pub freshness: Freshness,
    /// Last time a refresh for this watch succeeded, if ever.
    pub last_success: Option<DateTime<Utc>>,
}

impl WatchSnapshot {
    /// The snapshot for a watch nothing is known about yet.
    pub fn pending(watch_id: WatchId) -> Self {
        Self {
            watch_id,
            record: None,
            freshness: Freshness::Pending,
            last_success: None,
        }
    }
}
