//! BuildRail provider adapters.
//!
//! Implements core's [`RunSource`] port for
//!
//! * GitHub Actions – [`github::GitHubActions`], and
//! * Cloudflare Workers Builds – [`cloudflare::CloudflareBuilds`],
//!
//! plus small catalogue traits ([`catalog`]) the "Add build" picker uses to
//! browse repositories, workflows, accounts and Workers.
//!
//! # Composition
//!
//! ```no_run
//! # fn main() -> Result<(), buildrail_core::ports::ProviderError> {
//! use std::sync::Arc;
//! use buildrail_core::ports::SystemClock;
//!
//! let client = buildrail_providers::default_http_client()?;
//! let providers = buildrail_providers::ProviderSet::new(client, Arc::new(SystemClock));
//! let sources = providers.run_sources(); // hand these to the refresh engine
//! # let _ = sources;
//! # Ok(()) }
//! ```
//!
//! # Conventions shared by every adapter
//!
//! * One shared `reqwest::Client` (rustls, gzip, timeouts) is injected; see
//!   [`default_http_client`].
//! * Base URLs are injectable (`with_base_url`) for tests and GitHub
//!   Enterprise Server.
//! * `observed_at` comes from the injected [`Clock`].
//! * Errors are mapped uniformly: 429 (or GitHub's rate-limit 403) →
//!   `RateLimited`, other 401/403 → `AccessDenied`, 404 → `NotFound`, other
//!   statuses → `Server`, transport → `Network`, undecodable → `Malformed`.
//! * The credential only ever goes into the `Authorization` header; it
//!   never appears in errors, logs or `Debug` output.

use std::sync::Arc;

use buildrail_core::domain::ProviderKind;
use buildrail_core::ports::{Clock, RunSource};

mod cache;
pub mod catalog;
pub mod cloudflare;
pub mod github;
mod http;

pub use http::{default_http_client, CONNECT_TIMEOUT, REQUEST_TIMEOUT, USER_AGENT};

use cloudflare::{CloudflareBuilds, CloudflareCatalog};
use github::{GitHubActions, GitHubCatalog};

/// All adapters, built over one shared client and clock.
///
/// Fields are `Arc`s so the app can hand the same adapter (and therefore
/// the same `ETag` / tag caches) to the refresh engine and elsewhere.
#[derive(Debug, Clone)]
pub struct ProviderSet {
    /// GitHub Actions run source.
    pub github: Arc<GitHubActions>,
    /// Cloudflare Workers Builds run source.
    pub cloudflare: Arc<CloudflareBuilds>,
    /// GitHub catalogue (identity, repositories, workflows, branches).
    pub github_catalog: Arc<GitHubCatalog>,
    /// Cloudflare catalogue (token check, accounts, Workers).
    pub cloudflare_catalog: Arc<CloudflareCatalog>,
}

impl ProviderSet {
    /// Builds every adapter against the production APIs.
    pub fn new(client: reqwest::Client, clock: Arc<dyn Clock>) -> Self {
        Self {
            github: Arc::new(GitHubActions::new(client.clone(), clock.clone())),
            cloudflare: Arc::new(CloudflareBuilds::new(client.clone(), clock.clone())),
            github_catalog: Arc::new(GitHubCatalog::new(client.clone(), clock.clone())),
            cloudflare_catalog: Arc::new(CloudflareCatalog::new(client, clock)),
        }
    }

    /// Every run source, one per [`ProviderKind`].
    pub fn run_sources(&self) -> Vec<Arc<dyn RunSource>> {
        vec![self.github.clone(), self.cloudflare.clone()]
    }

    /// The run source serving `kind`.
    pub fn run_source(&self, kind: ProviderKind) -> Arc<dyn RunSource> {
        match kind {
            ProviderKind::GitHub => self.github.clone(),
            ProviderKind::Cloudflare => self.cloudflare.clone(),
        }
    }
}

/// Shorthand for [`ProviderSet::new`]`(..).run_sources()`.
pub fn registry(client: reqwest::Client, clock: Arc<dyn Clock>) -> Vec<Arc<dyn RunSource>> {
    ProviderSet::new(client, clock).run_sources()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use buildrail_core::ports::SystemClock;

    #[test]
    fn registry_has_one_source_per_provider() {
        let client = default_http_client().unwrap();
        let set = ProviderSet::new(client.clone(), Arc::new(SystemClock));
        let kinds: Vec<_> = set.run_sources().iter().map(|s| s.provider()).collect();
        assert_eq!(kinds, vec![ProviderKind::GitHub, ProviderKind::Cloudflare]);
        assert_eq!(
            set.run_source(ProviderKind::Cloudflare).provider(),
            ProviderKind::Cloudflare
        );
        assert_eq!(
            set.run_source(ProviderKind::GitHub).provider(),
            ProviderKind::GitHub
        );
        assert_eq!(registry(client, Arc::new(SystemClock)).len(), 2);
        assert!(USER_AGENT.starts_with("buildrail/"));
    }
}
