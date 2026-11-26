//! [`GitHubActions`]: the GitHub [`RunSource`].

use std::sync::Arc;

use async_trait::async_trait;
use buildrail_core::domain::{BuildRecord, BuildStatus, ProviderKind, RunId, WatchTarget};
use buildrail_core::ports::{Clock, ProviderError, RunSource, Secret};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::status::{is_failed_job, map_run_status};
use super::{GITHUB_API_URL, HEADERS};
use crate::cache::EtagCache;
use crate::http::{decode, endpoint, fingerprint, Fetched, Transport};

/// Default number of recent runs returned per target.
pub const DEFAULT_RUN_LIMIT: u8 = 10;

/// Number of distinct (target, credential) responses kept for `ETag` reuse.
const CACHE_CAPACITY: usize = 256;

/// Reads recent GitHub Actions workflow runs.
///
/// One request per poll (`GET …/actions/workflows/{workflow}/runs`), made
/// conditional with `If-None-Match` so unchanged polls are answered with a
/// free 304 and the previously parsed records are reused (with a fresh
/// `observed_at`). One extra request (the jobs of that attempt) is made only
/// when the newest run failed, to name the failing job; its result is cached
/// with the run so it is not repeated while nothing changes.
#[derive(Debug)]
pub struct GitHubActions {
    transport: Transport,
    base_url: String,
    run_limit: u8,
    cache: EtagCache<(String, u64), CachedRuns>,
}

#[derive(Debug, Clone)]
struct CachedRuns {
    records: Vec<BuildRecord>,
    /// `false` once the failing-job lookup succeeded (or was not needed).
    /// A failed lookup is retried on the next poll.
    failing_job_pending: bool,
}

impl GitHubActions {
    /// An adapter for `api.github.com` using the shared `client` (see
    /// [`crate::default_http_client`]) and `clock` for `observed_at`.
    pub fn new(client: reqwest::Client, clock: Arc<dyn Clock>) -> Self {
        Self {
            transport: Transport::new(client, ProviderKind::GitHub, clock, HEADERS),
            base_url: GITHUB_API_URL.to_owned(),
            run_limit: DEFAULT_RUN_LIMIT,
            cache: EtagCache::new(CACHE_CAPACITY),
        }
    }

    /// Points the adapter at another API root: a GitHub Enterprise Server
    /// (`https://ghe.example.com/api/v3`) or a mock server in tests.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// How many recent runs to return (1–100, default
    /// [`DEFAULT_RUN_LIMIT`]).
    pub fn with_run_limit(mut self, limit: u8) -> Self {
        self.run_limit = limit.clamp(1, 100);
        self
    }

    pub(super) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(super) fn transport(&self) -> &Transport {
        &self.transport
    }

    async fn first_failed_job(
        &self,
        owner: &str,
        repo: &str,
        record: &BuildRecord,
        credential: &Secret,
    ) -> Result<Option<String>, ProviderError> {
        let attempt = record.attempt.to_string();
        let mut url = endpoint(
            &self.base_url,
            &[
                "repos",
                owner,
                repo,
                "actions",
                "runs",
                record.run_id.as_str(),
                "attempts",
                &attempt,
                "jobs",
            ],
        )?;
        url.query_pairs_mut().append_pair("per_page", "100");
        let (bytes, _) = self.transport.get_body(url, credential).await?;
        let page: JobsPage = decode(&bytes)?;
        Ok(page
            .jobs
            .into_iter()
            .find(|job| is_failed_job(job.conclusion.as_deref()))
            .map(|job| job.name))
    }

    /// Fills `failing_job` of the newest run when it failed. Returns `true`
    /// when nothing is left to look up.
    async fn fill_failing_job(
        &self,
        owner: &str,
        repo: &str,
        records: &mut [BuildRecord],
        credential: &Secret,
    ) -> bool {
        let Some(newest) = newest_mut(records) else {
            return true;
        };
        if newest.status != BuildStatus::Failed || newest.failing_job.is_some() {
            return true;
        }
        match self.first_failed_job(owner, repo, newest, credential).await {
            Ok(name) => {
                newest.failing_job = name;
                true
            }
            Err(error) => {
                // Best effort: the run list is still worth showing.
                tracing::debug!(%error, run = %newest.run_id, "failing job lookup failed");
                false
            }
        }
    }
}

#[async_trait]
impl RunSource for GitHubActions {
    fn provider(&self) -> ProviderKind {
        ProviderKind::GitHub
    }

    async fn recent_runs(
        &self,
        target: &WatchTarget,
        credential: &Secret,
    ) -> Result<Vec<BuildRecord>, ProviderError> {
        let WatchTarget::GitHub {
            owner,
            repo,
            workflow,
            branch,
        } = target
        else {
            // A Cloudflare target routed here is a wiring bug; report it as
            // "not found on GitHub" rather than panic.
            return Err(ProviderError::NotFound {
                provider: ProviderKind::GitHub,
            });
        };

        let mut url = endpoint(
            &self.base_url,
            &[
                "repos",
                owner,
                repo,
                "actions",
                "workflows",
                workflow,
                "runs",
            ],
        )?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("per_page", &self.run_limit.to_string());
            query.append_pair("exclude_pull_requests", "true");
            if let Some(branch) = branch {
                query.append_pair("branch", branch);
            }
        }

        let key = (url.to_string(), fingerprint(credential));
        let cached = self.cache.get(&key);
        let fetched = self
            .transport
            .get(
                url,
                credential,
                cached.as_ref().map(|(etag, _)| etag.as_str()),
            )
            .await?;
        let now = self.transport.now();

        let (mut runs, etag) = match (fetched, cached) {
            (Fetched::NotModified, Some((etag, runs))) => (runs, Some(etag)),
            (Fetched::NotModified, None) => {
                return Err(ProviderError::Server {
                    provider: ProviderKind::GitHub,
                    status: 304,
                })
            }
            (Fetched::Body { bytes, etag, .. }, previous) => {
                let page: RunsPage = decode(&bytes)?;
                let mut records: Vec<BuildRecord> = page
                    .workflow_runs
                    .into_iter()
                    .map(|run| run.into_record(now))
                    .collect();
                if let Some((_, previous)) = previous {
                    carry_failing_jobs(&previous.records, &mut records);
                }
                (
                    CachedRuns {
                        records,
                        failing_job_pending: true,
                    },
                    etag,
                )
            }
        };

        if runs.failing_job_pending {
            runs.failing_job_pending = !self
                .fill_failing_job(owner, repo, &mut runs.records, credential)
                .await;
        }
        for record in &mut runs.records {
            record.observed_at = now;
        }
        match etag {
            Some(etag) => self.cache.put(key, etag, runs.clone()),
            None => self.cache.remove(&key),
        }
        Ok(runs.records)
    }
}

/// The run that core will most likely show: latest `created_at`, ties
/// broken by the larger run ID.
fn newest_mut(records: &mut [BuildRecord]) -> Option<&mut BuildRecord> {
    records
        .iter_mut()
        .max_by(|a, b| (a.created_at, &a.run_id).cmp(&(b.created_at, &b.run_id)))
}

/// Copies already known failing-job names onto freshly decoded records of
/// the same run attempt, so a changed list does not trigger a new jobs call
/// for a run we already inspected.
fn carry_failing_jobs(previous: &[BuildRecord], fresh: &mut [BuildRecord]) {
    for record in fresh.iter_mut() {
        if let Some(known) = previous.iter().find(|p| {
            p.run_id == record.run_id && p.attempt == record.attempt && p.status == record.status
        }) {
            record.failing_job.clone_from(&known.failing_job);
        }
    }
}

#[derive(Debug, Deserialize)]
struct RunsPage {
    #[serde(default)]
    workflow_runs: Vec<WireRun>,
}

#[derive(Debug, Deserialize)]
struct WireRun {
    id: u64,
    run_attempt: Option<u32>,
    status: Option<String>,
    conclusion: Option<String>,
    html_url: String,
    head_branch: Option<String>,
    head_sha: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl WireRun {
    fn into_record(self, observed_at: DateTime<Utc>) -> BuildRecord {
        BuildRecord {
            run_id: RunId::new(self.id.to_string()),
            attempt: self.run_attempt.unwrap_or(1).max(1),
            status: map_run_status(self.status.as_deref(), self.conclusion.as_deref()),
            url: self.html_url,
            branch: self.head_branch,
            commit: self.head_sha,
            failing_job: None,
            created_at: self.created_at,
            updated_at: self.updated_at,
            observed_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct JobsPage {
    #[serde(default)]
    jobs: Vec<WireJob>,
}

#[derive(Debug, Deserialize)]
struct WireJob {
    name: String,
    conclusion: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn record(id: &str, minute: u32, status: BuildStatus) -> BuildRecord {
        let at = Utc.with_ymd_and_hms(2026, 1, 1, 0, minute, 0).unwrap();
        BuildRecord {
            run_id: RunId::new(id),
            attempt: 1,
            status,
            url: String::new(),
            branch: None,
            commit: None,
            failing_job: None,
            created_at: at,
            updated_at: at,
            observed_at: at,
        }
    }

    #[test]
    fn newest_is_latest_created() {
        let mut records = vec![
            record("1", 5, BuildStatus::Passed),
            record("2", 9, BuildStatus::Failed),
            record("3", 1, BuildStatus::Passed),
        ];
        assert_eq!(newest_mut(&mut records).unwrap().run_id.as_str(), "2");
        assert!(newest_mut(&mut []).is_none());
    }

    #[test]
    fn carries_known_failing_job_for_same_attempt_only() {
        let mut old = record("2", 9, BuildStatus::Failed);
        old.failing_job = Some("test".into());
        let mut fresh = vec![record("2", 9, BuildStatus::Failed)];
        carry_failing_jobs(std::slice::from_ref(&old), &mut fresh);
        assert_eq!(fresh[0].failing_job.as_deref(), Some("test"));

        let mut rerun = vec![record("2", 9, BuildStatus::Failed)];
        rerun[0].attempt = 2;
        carry_failing_jobs(&[old], &mut rerun);
        assert_eq!(rerun[0].failing_job, None);
    }
}
