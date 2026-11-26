//! [`CloudflareBuilds`]: the Workers Builds [`RunSource`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use buildrail_core::domain::{BuildRecord, ProviderKind, RunId, WatchTarget};
use buildrail_core::ports::{Clock, ProviderError, RunSource, Secret};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::status::{dashboard_build_url, is_production_deploy, map_build_status};
use super::{open_envelope, CLOUDFLARE_API_URL, HEADERS};
use crate::http::{endpoint, fingerprint, Transport};

/// Default number of recent builds returned per target.
pub const DEFAULT_BUILD_LIMIT: usize = 10;

/// Page size used when a branch filter is set: the API cannot filter by
/// branch, so fetch more and filter locally.
pub(super) const FILTERED_PAGE_SIZE: usize = 50;

/// How long a listed page of builds may be reused by the details panel
/// (which asks for the run history and then one build's details in the
/// same breath: one list request serves both).
pub(super) const LIST_REUSE: chrono::TimeDelta = chrono::TimeDelta::seconds(10);

/// Reads recent Workers Builds for a Worker.
///
/// A poll normally costs one request (list builds). The Worker name → tag
/// lookup happens once per (account, Worker, credential) and is cached;
/// if the builds call later answers 404 for a cached tag (the Worker was
/// deleted and re-created under the same name) the tag is resolved again
/// once.
///
/// Workers Builds has no re-run concept, so `attempt` is always 1 and every
/// build has its own `build_uuid`. It has no jobs either, so `failing_job`
/// stays `None`.
pub struct CloudflareBuilds {
    transport: Transport,
    base_url: String,
    limit: usize,
    tags: Mutex<HashMap<(String, String, u64), String>>,
    /// The last list per (account, Worker, credential), with when it came.
    lists: Mutex<BuildLists>,
}

type BuildLists = HashMap<(String, String, u64), (DateTime<Utc>, Vec<WireBuild>)>;

impl std::fmt::Debug for CloudflareBuilds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudflareBuilds")
            .field("base_url", &self.base_url)
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

impl CloudflareBuilds {
    /// An adapter for `api.cloudflare.com` using the shared `client` and
    /// `clock` for `observed_at`.
    pub fn new(client: reqwest::Client, clock: Arc<dyn Clock>) -> Self {
        Self {
            transport: Transport::new(client, ProviderKind::Cloudflare, clock, HEADERS),
            base_url: CLOUDFLARE_API_URL.to_owned(),
            limit: DEFAULT_BUILD_LIMIT,
            tags: Mutex::new(HashMap::new()),
            lists: Mutex::new(HashMap::new()),
        }
    }

    /// Points the adapter at another API root (a mock server in tests). The
    /// root includes the version path, e.g. `http://127.0.0.1:1234/client/v4`.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// How many recent builds to return (1–200, default
    /// [`DEFAULT_BUILD_LIMIT`]).
    pub fn with_build_limit(mut self, limit: usize) -> Self {
        self.limit = limit.clamp(1, 200);
        self
    }

    fn cached_tag(&self, key: &(String, String, u64)) -> Option<String> {
        self.tags
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .cloned()
    }

    fn store_tag(&self, key: (String, String, u64), tag: Option<String>) {
        let mut tags = self.tags.lock().unwrap_or_else(|p| p.into_inner());
        match tag {
            Some(tag) => tags.insert(key, tag),
            None => tags.remove(&key),
        };
    }

    async fn resolve_tag(
        &self,
        account_id: &str,
        worker: &str,
        credential: &Secret,
    ) -> Result<String, ProviderError> {
        let url = endpoint(
            &self.base_url,
            &["accounts", account_id, "workers", "scripts"],
        )?;
        let (bytes, _) = self.transport.get_body(url, credential).await?;
        let (scripts, _) = open_envelope::<Vec<WireScript>>(&bytes)?;
        scripts
            .unwrap_or_default()
            .into_iter()
            .find(|script| script.id == worker)
            .and_then(|script| script.tag)
            .ok_or(ProviderError::NotFound {
                provider: ProviderKind::Cloudflare,
            })
    }

    /// Lists builds for a Worker by name: resolves (or reuses) its tag and
    /// lists its builds, resolving the tag again once if a cached one is
    /// answered with 404. The list is remembered briefly so the details
    /// panel can read a build from it without asking again
    /// ([`Self::recent_list`]).
    pub(super) async fn fetch_builds(
        &self,
        account_id: &str,
        worker: &str,
        page_size: usize,
        credential: &Secret,
    ) -> Result<Vec<WireBuild>, ProviderError> {
        let key = (
            account_id.to_owned(),
            worker.to_owned(),
            fingerprint(credential),
        );
        let builds = match self.cached_tag(&key) {
            Some(tag) => match self
                .list_builds(account_id, &tag, page_size, credential)
                .await
            {
                Err(ProviderError::NotFound { .. }) => {
                    self.store_tag(key.clone(), None);
                    None
                }
                other => Some(other?),
            },
            None => None,
        };
        let builds = match builds {
            Some(builds) => builds,
            None => {
                let tag = self.resolve_tag(account_id, worker, credential).await?;
                self.store_tag(key.clone(), Some(tag.clone()));
                self.list_builds(account_id, &tag, page_size, credential)
                    .await?
            }
        };
        let now = self.transport.now();
        self.lists
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, (now, builds.clone()));
        Ok(builds)
    }

    /// The builds listed for this Worker and credential within the last
    /// [`LIST_REUSE`], if any.
    pub(super) fn recent_list(
        &self,
        account_id: &str,
        worker: &str,
        credential: &Secret,
    ) -> Option<Vec<WireBuild>> {
        let key = (
            account_id.to_owned(),
            worker.to_owned(),
            fingerprint(credential),
        );
        let now = self.transport.now();
        let lists = self.lists.lock().unwrap_or_else(|p| p.into_inner());
        lists
            .get(&key)
            .filter(|(at, _)| now - *at <= LIST_REUSE && *at <= now)
            .map(|(_, builds)| builds.clone())
    }

    async fn list_builds(
        &self,
        account_id: &str,
        tag: &str,
        page_size: usize,
        credential: &Secret,
    ) -> Result<Vec<WireBuild>, ProviderError> {
        let mut url = endpoint(
            &self.base_url,
            &["accounts", account_id, "builds", "workers", tag, "builds"],
        )?;
        url.query_pairs_mut()
            .append_pair("page", "1")
            .append_pair("per_page", &page_size.to_string());
        let (bytes, _) = self.transport.get_body(url, credential).await?;
        let (builds, _) = open_envelope::<Vec<WireBuild>>(&bytes)?;
        Ok(builds.unwrap_or_default())
    }
}

#[async_trait]
impl RunSource for CloudflareBuilds {
    fn provider(&self) -> ProviderKind {
        ProviderKind::Cloudflare
    }

    async fn recent_runs(
        &self,
        target: &WatchTarget,
        credential: &Secret,
    ) -> Result<Vec<BuildRecord>, ProviderError> {
        let WatchTarget::Cloudflare {
            account_id,
            worker,
            branch,
        } = target
        else {
            return Err(ProviderError::NotFound {
                provider: ProviderKind::Cloudflare,
            });
        };
        let page_size = if branch.is_some() {
            FILTERED_PAGE_SIZE.max(self.limit)
        } else {
            self.limit
        };

        let builds = self
            .fetch_builds(account_id, worker, page_size, credential)
            .await?;
        let now = self.transport.now();

        let mut records: Vec<BuildRecord> = builds
            .into_iter()
            .filter(|build| match branch {
                Some(wanted) => build.branch() == Some(wanted.as_str()),
                None => true,
            })
            .map(|build| build.into_record(account_id, worker, now))
            .collect();
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        records.truncate(self.limit);
        Ok(records)
    }
}

#[derive(Debug, Deserialize)]
struct WireScript {
    id: String,
    tag: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct WireBuild {
    pub(super) build_uuid: String,
    pub(super) status: String,
    pub(super) build_outcome: Option<String>,
    pub(super) created_on: DateTime<Utc>,
    pub(super) initializing_on: Option<DateTime<Utc>>,
    pub(super) running_on: Option<DateTime<Utc>>,
    pub(super) modified_on: Option<DateTime<Utc>>,
    pub(super) stopped_on: Option<DateTime<Utc>>,
    pub(super) build_trigger_metadata: Option<WireTriggerMetadata>,
    trigger: Option<WireTrigger>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct WireTriggerMetadata {
    pub(super) build_trigger_source: Option<String>,
    branch: Option<String>,
    commit_hash: Option<String>,
    pub(super) commit_message: Option<String>,
    pub(super) author: Option<String>,
    deploy_command: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct WireTrigger {
    branch_includes: Option<Vec<String>>,
    #[serde(default)]
    branch_excludes: Vec<String>,
    deploy_command: Option<String>,
}

impl WireBuild {
    pub(super) fn branch(&self) -> Option<&str> {
        self.build_trigger_metadata
            .as_ref()
            .and_then(|m| m.branch.as_deref())
    }

    pub(super) fn production_deploy(&self) -> bool {
        let Some(trigger) = &self.trigger else {
            return false;
        };
        let command = self
            .build_trigger_metadata
            .as_ref()
            .and_then(|m| m.deploy_command.as_deref())
            .or(trigger.deploy_command.as_deref());
        is_production_deploy(
            trigger.branch_includes.as_deref(),
            &trigger.branch_excludes,
            command,
        )
    }

    fn into_record(
        self,
        account_id: &str,
        worker: &str,
        observed_at: DateTime<Utc>,
    ) -> BuildRecord {
        let status = map_build_status(
            &self.status,
            self.build_outcome.as_deref(),
            self.production_deploy(),
        );
        let updated_at = self
            .modified_on
            .or(self.stopped_on)
            .unwrap_or(self.created_on);
        let url = dashboard_build_url(account_id, worker, &self.build_uuid);
        let (branch, commit) = match self.build_trigger_metadata {
            Some(meta) => (meta.branch, meta.commit_hash),
            None => (None, None),
        };
        BuildRecord {
            run_id: RunId::new(self.build_uuid),
            attempt: 1,
            status,
            url,
            branch,
            commit,
            failing_job: None,
            created_at: self.created_on,
            updated_at,
            observed_at,
        }
    }
}
