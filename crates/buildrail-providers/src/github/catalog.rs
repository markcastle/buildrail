//! [`GitHubCatalog`]: GitHub catalogue for the "Add build" picker.

use std::sync::Arc;

use async_trait::async_trait;
use buildrail_core::domain::ProviderKind;
use buildrail_core::ports::{Clock, ProviderError, Secret};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use url::Url;

use super::{GITHUB_API_URL, HEADERS};
use crate::catalog::{
    GitHubBranch, GitHubBranches, GitHubIdentity, GitHubRepositories, GitHubRepository, GitHubUser,
    GitHubWorkflow, GitHubWorkflows, MAX_BRANCHES, MAX_REPOSITORIES, MAX_WORKFLOWS,
};
use crate::http::{decode, endpoint, next_link, parse_base, Transport};

/// GitHub's maximum page size.
const PAGE_SIZE: &str = "100";
/// Hard stop for pagination loops, whatever the `Link` headers say.
const MAX_PAGES: usize = 20;

/// GitHub catalogue: identity, repositories, workflows and branches.
///
/// Pagination follows the `Link: …; rel="next"` header as GitHub
/// recommends, but only to URLs on the configured API host.
#[derive(Debug)]
pub struct GitHubCatalog {
    transport: Transport,
    base_url: String,
}

impl GitHubCatalog {
    /// A catalogue for `api.github.com`.
    pub fn new(client: reqwest::Client, clock: Arc<dyn Clock>) -> Self {
        Self {
            transport: Transport::new(client, ProviderKind::GitHub, clock, HEADERS),
            base_url: GITHUB_API_URL.to_owned(),
        }
    }

    /// Points the catalogue at another API root (GHES or a mock server).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn url(&self, segments: &[&str], query: &[(&str, &str)]) -> Result<Url, ProviderError> {
        let mut url = endpoint(&self.base_url, segments)?;
        url.query_pairs_mut().extend_pairs(query);
        Ok(url)
    }

    /// Fetches `first` and every `next` page, decoding each page with `T`
    /// and flattening it with `items`, up to `cap` items.
    async fn paginate<T, I>(
        &self,
        first: Url,
        credential: &Secret,
        cap: usize,
        items: impl Fn(T) -> Vec<I>,
    ) -> Result<Vec<I>, ProviderError>
    where
        T: DeserializeOwned,
    {
        let base = parse_base(&self.base_url)?;
        let mut out = Vec::new();
        let mut next = Some(first);
        let mut pages = 0;
        while let Some(url) = next.take() {
            let (bytes, headers) = self.transport.get_body(url, credential).await?;
            out.extend(items(decode::<T>(&bytes)?));
            pages += 1;
            if out.len() >= cap || pages >= MAX_PAGES {
                break;
            }
            next = next_link(&headers, &base);
        }
        out.truncate(cap);
        Ok(out)
    }
}

#[async_trait]
impl GitHubIdentity for GitHubCatalog {
    async fn current_user(&self, credential: &Secret) -> Result<GitHubUser, ProviderError> {
        let (bytes, _) = self
            .transport
            .get_body(self.url(&["user"], &[])?, credential)
            .await?;
        decode(&bytes)
    }
}

#[async_trait]
impl GitHubRepositories for GitHubCatalog {
    async fn repositories(
        &self,
        credential: &Secret,
    ) -> Result<Vec<GitHubRepository>, ProviderError> {
        let url = self.url(
            &["user", "repos"],
            &[("per_page", PAGE_SIZE), ("sort", "pushed")],
        )?;
        self.paginate(url, credential, MAX_REPOSITORIES, |page: Vec<WireRepo>| {
            page.into_iter().map(WireRepo::into_repository).collect()
        })
        .await
    }
}

#[async_trait]
impl GitHubWorkflows for GitHubCatalog {
    async fn workflows(
        &self,
        owner: &str,
        repo: &str,
        credential: &Secret,
    ) -> Result<Vec<GitHubWorkflow>, ProviderError> {
        let url = self.url(
            &["repos", owner, repo, "actions", "workflows"],
            &[("per_page", PAGE_SIZE)],
        )?;
        self.paginate(url, credential, MAX_WORKFLOWS, |page: WorkflowsPage| {
            page.workflows
        })
        .await
    }
}

#[async_trait]
impl GitHubBranches for GitHubCatalog {
    async fn branches(
        &self,
        owner: &str,
        repo: &str,
        credential: &Secret,
    ) -> Result<Vec<GitHubBranch>, ProviderError> {
        let url = self.url(
            &["repos", owner, repo, "branches"],
            &[("per_page", PAGE_SIZE)],
        )?;
        self.paginate(url, credential, MAX_BRANCHES, |page: Vec<GitHubBranch>| {
            page
        })
        .await
    }
}

#[derive(Debug, Deserialize)]
struct WireRepo {
    name: String,
    full_name: String,
    owner: WireOwner,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    archived: bool,
    default_branch: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireOwner {
    login: String,
}

impl WireRepo {
    fn into_repository(self) -> GitHubRepository {
        GitHubRepository {
            owner: self.owner.login,
            name: self.name,
            full_name: self.full_name,
            private: self.private,
            archived: self.archived,
            default_branch: self.default_branch,
        }
    }
}

#[derive(Debug, Deserialize)]
struct WorkflowsPage {
    #[serde(default)]
    workflows: Vec<GitHubWorkflow>,
}
