//! Ports: the traits the outside world implements for core.
//!
//! Core code depends on these abstractions only (dependency inversion).
//! Adapters live in `buildrail-providers`, `buildrail-platform` and
//! `buildrail-profile`. Traits are kept small on purpose (interface
//! segregation): a caller that only reads runs never sees catalogue or
//! credential-writing methods.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::domain::{BuildRecord, ConnectionAlias, ProviderKind, WatchSnapshot, WatchTarget};

/// A provider credential. `Debug` and `Display` never print the value, so a
/// secret cannot leak through logs or error messages by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a token.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw token. Call only at the point of use (an HTTP header, the OS
    /// credential store).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Why a provider call failed. Variants are chosen so the caller can decide
/// what to *do* (reconnect, wait, retry) without parsing strings.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderError {
    /// HTTP 401, or 403 that is not a rate limit: the credential is missing,
    /// expired or lacks the needed read scope.
    #[error("access denied by {provider}")]
    AccessDenied {
        /// Provider that refused.
        provider: ProviderKind,
    },
    /// The target does not exist or is not visible to this credential.
    #[error("target not found on {provider}")]
    NotFound {
        /// Provider that answered.
        provider: ProviderKind,
    },
    /// The provider's rate limit was hit.
    #[error("rate limited by {provider}")]
    RateLimited {
        /// Provider that throttled us.
        provider: ProviderKind,
        /// How long the provider asked us to wait, when it said.
        retry_after: Option<Duration>,
    },
    /// Could not reach the provider (DNS, TCP, TLS, timeout).
    #[error("network error: {0}")]
    Network(String),
    /// The provider answered with a 5xx or an unexpected status.
    #[error("{provider} returned HTTP {status}")]
    Server {
        /// Provider that failed.
        provider: ProviderKind,
        /// HTTP status code.
        status: u16,
    },
    /// The response could not be decoded.
    #[error("unreadable response: {0}")]
    Malformed(String),
}

/// Reads recent runs for a target. One implementation per provider.
#[async_trait]
pub trait RunSource: Send + Sync {
    /// The provider this source talks to.
    fn provider(&self) -> ProviderKind;

    /// Recent runs matching `target` (including its branch filter), in any
    /// order. An empty vector means "the target exists and has no matching
    /// runs". Picking the winner is core's job ([`crate::selection`]), so
    /// implementations should return the handful of most recent candidates
    /// rather than guess.
    async fn recent_runs(
        &self,
        target: &WatchTarget,
        credential: &Secret,
    ) -> Result<Vec<BuildRecord>, ProviderError>;
}

/// Read access to stored provider credentials.
pub trait CredentialReader: Send + Sync {
    /// The credential for `alias`, or `Ok(None)` when none is stored.
    fn read(&self, alias: &ConnectionAlias) -> Result<Option<Secret>, CredentialError>;
}

/// Write access to stored provider credentials. Only the settings UI needs
/// this; the refresh engine takes a [`CredentialReader`].
pub trait CredentialWriter: Send + Sync {
    /// Stores or replaces the credential for `alias`.
    fn write(&self, alias: &ConnectionAlias, secret: &Secret) -> Result<(), CredentialError>;
    /// Removes the credential for `alias`. Removing a missing one is `Ok`.
    fn delete(&self, alias: &ConnectionAlias) -> Result<(), CredentialError>;
}

/// The OS credential store failed. The message never contains the secret.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("credential store error: {0}")]
pub struct CredentialError(pub String);

/// Wall-clock time, injectable so tests control it.
pub trait Clock: Send + Sync {
    /// Current UTC time.
    fn now(&self) -> DateTime<Utc>;
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Persists the last known snapshots so the strip can show cached state
/// (with its age) at startup and while offline.
pub trait SnapshotCache: Send + Sync {
    /// Loads whatever was saved last. A missing or unreadable cache is an
    /// empty vector, never an error: the cache is an optimisation.
    fn load(&self) -> Vec<WatchSnapshot>;
    /// Saves the current snapshots. Failures are logged by the
    /// implementation and otherwise ignored.
    fn save(&self, snapshots: &[WatchSnapshot]);
}
