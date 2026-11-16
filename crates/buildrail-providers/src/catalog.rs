//! Catalogue ports for the "Add build" picker.
//!
//! The picker walks the user from a credential to a concrete
//! [`WatchTarget`](buildrail_core::domain::WatchTarget): verify the token,
//! pick a repository / account, then a workflow / Worker and optionally a
//! branch. Each step is its own small trait (interface segregation) so the
//! UI and its tests depend only on what they use. Pagination is handled
//! inside the implementations; every list is capped (see the constants) so
//! a token with access to thousands of repositories cannot stall the UI.
//!
//! Implementations: [`crate::github::GitHubCatalog`] and
//! [`crate::cloudflare::CloudflareCatalog`].

use async_trait::async_trait;
use buildrail_core::ports::{ProviderError, Secret};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Upper bound on repositories returned by [`GitHubRepositories`].
pub const MAX_REPOSITORIES: usize = 1000;
/// Upper bound on workflows returned by [`GitHubWorkflows`].
pub const MAX_WORKFLOWS: usize = 500;
/// Upper bound on branches returned by [`GitHubBranches`].
pub const MAX_BRANCHES: usize = 1000;
/// Upper bound on accounts returned by [`CloudflareAccounts`].
pub const MAX_ACCOUNTS: usize = 500;

// ---------------------------------------------------------------- GitHub ---

/// The account a GitHub token belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubUser {
    /// Login handle, e.g. `octocat`.
    pub login: String,
    /// Display name, when the user set one.
    pub name: Option<String>,
}

/// A repository the token can see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubRepository {
    /// Owner login (user or organisation).
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// `owner/name`.
    pub full_name: String,
    /// Whether the repository is private.
    pub private: bool,
    /// Archived repositories no longer run workflows.
    pub archived: bool,
    /// Default branch, a sensible pre-selection for the branch filter.
    pub default_branch: Option<String>,
}

/// A workflow defined in a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubWorkflow {
    /// Numeric workflow ID.
    pub id: u64,
    /// Display name (the workflow's `name:`).
    pub name: String,
    /// Path, normally `.github/workflows/<file>.yml`.
    pub path: String,
    /// `active`, `disabled_manually`, `disabled_inactivity`, …
    pub state: String,
}

impl GitHubWorkflow {
    /// The value to store in `WatchTarget::GitHub::workflow`.
    ///
    /// The file name (`ci.yml`) is preferred because it is readable and
    /// survives export/import between forks. GitHub-managed "dynamic"
    /// workflows (Pages, Dependabot, CodeQL default setup) have paths like
    /// `dynamic/pages/pages-build-deployment` that the API does not accept
    /// as an identifier, so the numeric ID is used for those.
    pub fn watch_key(&self) -> String {
        match self.path.strip_prefix(".github/workflows/") {
            Some(file) if !file.is_empty() && !file.contains('/') => file.to_owned(),
            _ => self.id.to_string(),
        }
    }
}

/// A branch of a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubBranch {
    /// Branch name.
    pub name: String,
    /// Whether branch protection is enabled.
    pub protected: bool,
}

/// "Who am I?" – verifies a GitHub token.
#[async_trait]
pub trait GitHubIdentity: Send + Sync {
    /// The user owning `credential` (`GET /user`). An invalid token yields
    /// [`ProviderError::AccessDenied`].
    async fn current_user(&self, credential: &Secret) -> Result<GitHubUser, ProviderError>;
}

/// Lists repositories accessible to a token.
#[async_trait]
pub trait GitHubRepositories: Send + Sync {
    /// Repositories the token can access, most recently pushed first, at
    /// most [`MAX_REPOSITORIES`]. For fine-grained tokens this is exactly
    /// the set of repositories the token was granted.
    async fn repositories(
        &self,
        credential: &Secret,
    ) -> Result<Vec<GitHubRepository>, ProviderError>;
}

/// Lists the workflows of a repository.
#[async_trait]
pub trait GitHubWorkflows: Send + Sync {
    /// Workflows of `owner/repo`, at most [`MAX_WORKFLOWS`].
    async fn workflows(
        &self,
        owner: &str,
        repo: &str,
        credential: &Secret,
    ) -> Result<Vec<GitHubWorkflow>, ProviderError>;
}

/// Lists the branches of a repository.
#[async_trait]
pub trait GitHubBranches: Send + Sync {
    /// Branches of `owner/repo`, at most [`MAX_BRANCHES`].
    async fn branches(
        &self,
        owner: &str,
        repo: &str,
        credential: &Secret,
    ) -> Result<Vec<GitHubBranch>, ProviderError>;
}

// ------------------------------------------------------------ Cloudflare ---

/// Result of verifying a Cloudflare API token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudflareTokenStatus {
    /// Token identifier (not the secret).
    pub id: String,
    /// `active`, `disabled` or `expired`.
    pub status: String,
    /// `true` when `status` is `active`.
    pub active: bool,
    /// When the token expires, if it does.
    pub expires_on: Option<DateTime<Utc>>,
}

/// A Cloudflare account the token can access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudflareAccount {
    /// Account ID (32 hex characters) – goes into
    /// `WatchTarget::Cloudflare::account_id`.
    pub id: String,
    /// Account name as shown in the dashboard.
    pub name: String,
}

/// A Worker script in an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudflareWorker {
    /// Script name – goes into `WatchTarget::Cloudflare::worker`.
    pub name: String,
    /// Immutable script tag, which the Builds API uses instead of the name.
    pub tag: Option<String>,
    /// When the script was last modified.
    pub modified_on: Option<DateTime<Utc>>,
}

/// Verifies a Cloudflare API token.
#[async_trait]
pub trait CloudflareTokenCheck: Send + Sync {
    /// Verifies `credential`. Workers Builds only accepts **user** API
    /// tokens, so this uses the user-token endpoint; an account-owned token
    /// is reported as [`ProviderError::AccessDenied`].
    async fn verify_token(
        &self,
        credential: &Secret,
    ) -> Result<CloudflareTokenStatus, ProviderError>;
}

/// Lists Cloudflare accounts.
#[async_trait]
pub trait CloudflareAccounts: Send + Sync {
    /// Accounts the token can access, at most [`MAX_ACCOUNTS`].
    async fn accounts(&self, credential: &Secret) -> Result<Vec<CloudflareAccount>, ProviderError>;
}

/// Lists Workers in an account.
#[async_trait]
pub trait CloudflareWorkers: Send + Sync {
    /// Worker scripts in `account_id`.
    async fn workers(
        &self,
        account_id: &str,
        credential: &Secret,
    ) -> Result<Vec<CloudflareWorker>, ProviderError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn workflow(path: &str) -> GitHubWorkflow {
        GitHubWorkflow {
            id: 161335,
            name: "CI".into(),
            path: path.into(),
            state: "active".into(),
        }
    }

    #[test]
    fn watch_key_prefers_file_name() {
        assert_eq!(workflow(".github/workflows/ci.yml").watch_key(), "ci.yml");
        assert_eq!(
            workflow("dynamic/pages/pages-build-deployment").watch_key(),
            "161335"
        );
        assert_eq!(workflow(".github/workflows/").watch_key(), "161335");
        assert_eq!(workflow(".github/workflows/a/b.yml").watch_key(), "161335");
    }
}
