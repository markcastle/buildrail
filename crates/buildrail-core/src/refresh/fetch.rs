//! One provider request: credential lookup, source lookup, timeout, and the
//! mapping of every way it can fail onto what the UI and the schedule need.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use super::schedule::Outcome;
use crate::domain::{BuildRecord, ProviderKind, RequestKey, StaleReason};
use crate::ports::{CredentialError, CredentialReader, ProviderError, RunSource};

/// The [`RunSource`]s available, one per provider.
///
/// The engine looks sources up here by the target's provider, so adding a
/// provider means registering one more source – no engine change (open /
/// closed). A target whose provider has no source shows as
/// [`StaleReason::ProviderError`].
#[derive(Clone, Default)]
pub struct SourceRegistry {
    sources: HashMap<ProviderKind, Arc<dyn RunSource>>,
}

impl SourceRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `source` under the provider it reports, replacing and
    /// returning any previous source for that provider.
    pub fn register(&mut self, source: Arc<dyn RunSource>) -> Option<Arc<dyn RunSource>> {
        self.sources.insert(source.provider(), source)
    }

    /// Builder-style [`register`](Self::register).
    #[must_use]
    pub fn with(mut self, source: Arc<dyn RunSource>) -> Self {
        self.register(source);
        self
    }

    /// The source for `provider`, if registered.
    pub fn get(&self, provider: ProviderKind) -> Option<&Arc<dyn RunSource>> {
        self.sources.get(&provider)
    }

    /// Providers with a registered source, sorted.
    pub fn providers(&self) -> Vec<ProviderKind> {
        let mut providers: Vec<ProviderKind> = self.sources.keys().copied().collect();
        providers.sort();
        providers
    }
}

impl FromIterator<Arc<dyn RunSource>> for SourceRegistry {
    fn from_iter<I: IntoIterator<Item = Arc<dyn RunSource>>>(iter: I) -> Self {
        iter.into_iter().fold(Self::new(), Self::with)
    }
}

impl fmt::Debug for SourceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceRegistry")
            .field("providers", &self.providers())
            .finish()
    }
}

/// Why a refresh of one request produced no data.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FetchError {
    /// The provider call failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// No credential is stored for the connection; the provider was not
    /// called.
    #[error("no credential stored for this connection")]
    NoCredential,
    /// The credential store itself failed; the provider was not called.
    #[error(transparent)]
    CredentialStore(#[from] CredentialError),
    /// No [`RunSource`] is registered for the target's provider.
    #[error("no source registered for {0}")]
    NoSource(ProviderKind),
    /// The provider did not answer within the policy's request timeout.
    #[error("request timed out")]
    TimedOut,
    /// The provider adapter panicked. Isolated so one buggy adapter cannot
    /// take the engine down.
    #[error("provider task failed")]
    Crashed,
}

impl FetchError {
    /// What the UI should say about this failure.
    pub fn stale_reason(&self) -> StaleReason {
        match self {
            Self::Provider(error) => stale_reason_for(error),
            Self::NoCredential | Self::CredentialStore(_) => StaleReason::AccessDenied,
            Self::NoSource(_) | Self::Crashed => StaleReason::ProviderError,
            Self::TimedOut => StaleReason::Offline,
        }
    }

    /// How the schedule should react: rate limits close the gate, all other
    /// failures back off.
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Provider(ProviderError::RateLimited { retry_after, .. }) => {
                Outcome::RateLimited {
                    retry_after: *retry_after,
                }
            }
            _ => Outcome::Failure,
        }
    }
}

/// Maps a provider error onto the reason shown next to stale data.
pub fn stale_reason_for(error: &ProviderError) -> StaleReason {
    match error {
        ProviderError::AccessDenied { .. } => StaleReason::AccessDenied,
        ProviderError::NotFound { .. } => StaleReason::NotFound,
        ProviderError::RateLimited { .. } => StaleReason::RateLimited,
        ProviderError::Network(_) => StaleReason::Offline,
        ProviderError::Server { .. } | ProviderError::Malformed(_) => StaleReason::ProviderError,
    }
}

/// Performs single requests. Cheap to clone into spawned tasks.
#[derive(Clone)]
pub(crate) struct Fetcher {
    sources: SourceRegistry,
    credentials: Arc<dyn CredentialReader>,
    timeout: Duration,
}

impl Fetcher {
    pub(crate) fn new(
        sources: SourceRegistry,
        credentials: Arc<dyn CredentialReader>,
        timeout: Duration,
    ) -> Self {
        Self {
            sources,
            credentials,
            timeout,
        }
    }

    /// Fetches the candidate runs for `key`. The credential is checked
    /// before the provider is called, so a missing credential costs no
    /// rate limit and is reported as access denied.
    pub(crate) async fn fetch(&self, key: &RequestKey) -> Result<Vec<BuildRecord>, FetchError> {
        let provider = key.target.provider();
        let source = self
            .sources
            .get(provider)
            .ok_or(FetchError::NoSource(provider))?;
        let secret = self
            .credentials
            .read(&key.connection)?
            .ok_or(FetchError::NoCredential)?;
        match tokio::time::timeout(self.timeout, source.recent_runs(&key.target, &secret)).await {
            Ok(result) => Ok(result?),
            Err(_) => Err(FetchError::TimedOut),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_errors_map_to_stale_reasons() {
        let p = ProviderKind::GitHub;
        let cases = [
            (
                ProviderError::AccessDenied { provider: p },
                StaleReason::AccessDenied,
            ),
            (
                ProviderError::NotFound { provider: p },
                StaleReason::NotFound,
            ),
            (
                ProviderError::RateLimited {
                    provider: p,
                    retry_after: None,
                },
                StaleReason::RateLimited,
            ),
            (ProviderError::Network("dns".into()), StaleReason::Offline),
            (
                ProviderError::Server {
                    provider: p,
                    status: 502,
                },
                StaleReason::ProviderError,
            ),
            (
                ProviderError::Malformed("json".into()),
                StaleReason::ProviderError,
            ),
        ];
        for (error, reason) in cases {
            assert_eq!(FetchError::from(error).stale_reason(), reason);
        }
    }

    #[test]
    fn fetch_errors_map_to_reasons_and_outcomes() {
        assert_eq!(
            FetchError::NoCredential.stale_reason(),
            StaleReason::AccessDenied
        );
        assert_eq!(
            FetchError::from(CredentialError("locked".into())).stale_reason(),
            StaleReason::AccessDenied
        );
        assert_eq!(
            FetchError::NoSource(ProviderKind::Cloudflare).stale_reason(),
            StaleReason::ProviderError
        );
        assert_eq!(
            FetchError::Crashed.stale_reason(),
            StaleReason::ProviderError
        );
        assert_eq!(FetchError::TimedOut.stale_reason(), StaleReason::Offline);
        assert_eq!(FetchError::TimedOut.outcome(), Outcome::Failure);
        let limited = FetchError::Provider(ProviderError::RateLimited {
            provider: ProviderKind::GitHub,
            retry_after: Some(Duration::from_secs(7)),
        });
        assert_eq!(
            limited.outcome(),
            Outcome::RateLimited {
                retry_after: Some(Duration::from_secs(7))
            }
        );
        assert_eq!(
            FetchError::NoSource(ProviderKind::Cloudflare).to_string(),
            "no source registered for Cloudflare"
        );
    }

    #[test]
    fn empty_registry_debug_lists_no_providers() {
        let registry = SourceRegistry::new();
        assert!(registry.providers().is_empty());
        assert_eq!(format!("{registry:?}"), "SourceRegistry { providers: [] }");
    }
}
