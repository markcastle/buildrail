//! [`GitHubActions`] as a [`RunDetailsSource`]: one run attempt's title,
//! trigger, timings and jobs.
//!
//! A panel open costs two requests, made concurrently:
//!
//! * `GET /repos/{owner}/{repo}/actions/runs/{id}/attempts/{n}` – the run
//!   as it was for that attempt (`display_title`, `event`, `actor`,
//!   `triggering_actor`, `run_number`, `run_started_at`,
//!   `head_commit.message`);
//! * `GET /repos/{owner}/{repo}/actions/runs/{id}/attempts/{n}/jobs` –
//!   the attempt's jobs with their steps, 100 a page, following `Link:
//!   rel="next"` for at most [`MAX_JOB_PAGES`] pages.
//!
//! Errors are mapped like every other call (see the crate docs); a run the
//! token cannot see, or one GitHub has deleted, is `NotFound`.

use async_trait::async_trait;
use buildrail_core::details::{JobSummary, RunDetails, RunRef};
use buildrail_core::domain::{BuildStatus, ProviderKind, WatchTarget};
use buildrail_core::ports::{ProviderError, RunDetailsSource, Secret};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::runs::GitHubActions;
use super::status::{is_failed_job, map_event, map_run_status};
use crate::http::{decode, endpoint, next_link, parse_base};

/// Jobs asked for per page (GitHub's maximum).
pub const JOBS_PER_PAGE: usize = 100;

/// At most this many pages of jobs are read for one attempt (300 jobs: a
/// matrix larger than that is summarised by its count).
pub const MAX_JOB_PAGES: usize = 3;

impl GitHubActions {
    async fn attempt(
        &self,
        owner: &str,
        repo: &str,
        run: &RunRef,
        credential: &Secret,
    ) -> Result<WireAttempt, ProviderError> {
        let attempt = run.attempt.to_string();
        let url = endpoint(
            self.base_url(),
            &[
                "repos",
                owner,
                repo,
                "actions",
                "runs",
                run.run_id.as_str(),
                "attempts",
                &attempt,
            ],
        )?;
        let (bytes, _) = self.transport().get_body(url, credential).await?;
        decode(&bytes)
    }

    async fn jobs(
        &self,
        owner: &str,
        repo: &str,
        run: &RunRef,
        credential: &Secret,
    ) -> Result<(Vec<JobSummary>, usize), ProviderError> {
        let attempt = run.attempt.to_string();
        let mut url = endpoint(
            self.base_url(),
            &[
                "repos",
                owner,
                repo,
                "actions",
                "runs",
                run.run_id.as_str(),
                "attempts",
                &attempt,
                "jobs",
            ],
        )?;
        url.query_pairs_mut()
            .append_pair("per_page", &JOBS_PER_PAGE.to_string());
        let base = parse_base(self.base_url())?;
        let mut jobs = Vec::new();
        let mut total = None;
        let mut next = Some(url);
        let mut pages = 0;
        while let Some(url) = next.take() {
            let (bytes, headers) = self.transport().get_body(url, credential).await?;
            let page: WireJobsPage = decode(&bytes)?;
            total = total.or(page.total_count);
            jobs.extend(page.jobs.into_iter().map(WireJob::into_summary));
            pages += 1;
            if pages < MAX_JOB_PAGES {
                next = next_link(&headers, &base);
            }
        }
        let total = total.unwrap_or(jobs.len()).max(jobs.len());
        Ok((jobs, total))
    }
}

#[async_trait]
impl RunDetailsSource for GitHubActions {
    fn provider(&self) -> ProviderKind {
        ProviderKind::GitHub
    }

    async fn run_details(
        &self,
        target: &WatchTarget,
        run: &RunRef,
        credential: &Secret,
    ) -> Result<RunDetails, ProviderError> {
        let WatchTarget::GitHub { owner, repo, .. } = target else {
            return Err(ProviderError::NotFound {
                provider: ProviderKind::GitHub,
            });
        };
        let (attempt, jobs) = tokio::join!(
            self.attempt(owner, repo, run, credential),
            self.jobs(owner, repo, run, credential)
        );
        let (jobs, total) = jobs?;
        Ok(attempt?.into_details(run, jobs, total))
    }
}

#[derive(Debug, Deserialize)]
struct WireUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct WireCommit {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireAttempt {
    run_number: Option<u64>,
    run_attempt: Option<u32>,
    event: Option<String>,
    display_title: Option<String>,
    status: Option<String>,
    conclusion: Option<String>,
    actor: Option<WireUser>,
    triggering_actor: Option<WireUser>,
    run_started_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    head_commit: Option<WireCommit>,
}

impl WireAttempt {
    fn into_details(self, run: &RunRef, jobs: Vec<JobSummary>, total: usize) -> RunDetails {
        let status = map_run_status(self.status.as_deref(), self.conclusion.as_deref());
        let finished = !status.is_active();
        let actor = self.actor.map(|u| u.login);
        // `triggering_actor` is who started *this* attempt; say it only
        // when it adds something.
        let triggered_by = self
            .triggering_actor
            .map(|u| u.login)
            .filter(|login| Some(login) != actor.as_ref() || run.attempt > 1);
        let mut details = RunDetails::new(RunRef {
            run_id: run.run_id.clone(),
            attempt: self.run_attempt.unwrap_or(run.attempt).max(1),
        });
        details.title = self.display_title;
        details.commit_message = self.head_commit.and_then(|c| c.message);
        details.actor = actor;
        details.triggered_by = triggered_by;
        details.trigger = map_event(self.event.as_deref());
        details.run_number = self.run_number;
        // Queued runs have no start yet; GitHub sends the creation time
        // in `run_started_at` for them, which would read as "running".
        details.started_at =
            (status != BuildStatus::Queued).then(|| self.run_started_at.unwrap_or(self.created_at));
        details.finished_at = finished.then_some(self.updated_at);
        details.jobs_total = total;
        details.jobs = Some(jobs);
        details
    }
}

#[derive(Debug, Deserialize)]
struct WireJobsPage {
    total_count: Option<usize>,
    #[serde(default)]
    jobs: Vec<WireJob>,
}

#[derive(Debug, Deserialize)]
struct WireStep {
    name: String,
    conclusion: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireJob {
    name: String,
    status: Option<String>,
    conclusion: Option<String>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    html_url: Option<String>,
    #[serde(default)]
    steps: Vec<WireStep>,
}

impl WireJob {
    fn into_summary(self) -> JobSummary {
        let status = map_run_status(self.status.as_deref(), self.conclusion.as_deref());
        let failing_step = (status == BuildStatus::Failed)
            .then(|| {
                self.steps
                    .into_iter()
                    .find(|step| is_failed_job(step.conclusion.as_deref()))
                    .map(|step| step.name)
            })
            .flatten();
        JobSummary {
            name: self.name,
            status,
            // A queued job's `started_at` is when it was queued.
            started_at: (status != BuildStatus::Queued)
                .then_some(self.started_at)
                .flatten(),
            finished_at: (!status.is_active()).then_some(self.completed_at).flatten(),
            url: self.html_url,
            failing_step,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use buildrail_core::details::Trigger;
    use buildrail_core::domain::RunId;

    use super::*;

    fn run(attempt: u32) -> RunRef {
        RunRef {
            run_id: RunId::new("1"),
            attempt,
        }
    }

    fn attempt(json: &str) -> WireAttempt {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn queued_runs_have_no_start_and_running_ones_no_finish() {
        let queued = attempt(
            r#"{"status":"queued","event":"schedule","created_at":"2026-11-16T10:00:00Z","updated_at":"2026-11-16T10:00:00Z","run_started_at":"2026-11-16T10:00:00Z"}"#,
        )
        .into_details(&run(1), Vec::new(), 0);
        assert_eq!(queued.started_at, None);
        assert_eq!(queued.finished_at, None);
        assert_eq!(queued.trigger, Trigger::Schedule);
        assert_eq!(queued.actor, None);

        let running = attempt(
            r#"{"status":"in_progress","created_at":"2026-11-16T10:00:00Z","updated_at":"2026-11-16T10:03:00Z","actor":{"login":"a"},"triggering_actor":{"login":"a"}}"#,
        )
        .into_details(&run(1), Vec::new(), 0);
        assert_eq!(
            running.started_at.unwrap().to_rfc3339(),
            "2026-11-16T10:00:00+00:00",
            "falls back to created_at"
        );
        assert_eq!(running.finished_at, None);
        assert_eq!(running.triggered_by, None, "same person, first attempt");
        assert_eq!(running.trigger, Trigger::Unknown);
        assert_eq!(running.run.attempt, 1);
    }

    #[test]
    fn a_rerun_names_who_reran_it() {
        let rerun = attempt(
            r#"{"status":"completed","conclusion":"success","run_attempt":3,"created_at":"2026-11-16T10:00:00Z","updated_at":"2026-11-16T10:03:00Z","actor":{"login":"a"},"triggering_actor":{"login":"a"}}"#,
        )
        .into_details(&run(3), Vec::new(), 0);
        assert_eq!(rerun.triggered_by.as_deref(), Some("a"));
        assert_eq!(rerun.run.attempt, 3);
        assert!(rerun.finished_at.is_some());
    }

    #[test]
    fn failing_step_only_for_failed_jobs_and_queued_jobs_have_no_start() {
        let job = |json: &str| {
            serde_json::from_str::<WireJob>(json)
                .unwrap()
                .into_summary()
        };
        let failed = job(
            r#"{"name":"t","status":"completed","conclusion":"failure","started_at":"2026-11-16T10:00:00Z","completed_at":"2026-11-16T10:01:00Z","steps":[{"name":"ok","conclusion":"success"},{"name":"boom","conclusion":"timed_out"},{"name":"later","conclusion":"failure"}]}"#,
        );
        assert_eq!(failed.failing_step.as_deref(), Some("boom"));
        assert!(failed.finished_at.is_some());
        let cancelled = job(
            r#"{"name":"t","status":"completed","conclusion":"cancelled","steps":[{"name":"x","conclusion":"failure"}]}"#,
        );
        assert_eq!(cancelled.failing_step, None);
        let queued = job(r#"{"name":"t","status":"queued","started_at":"2026-11-16T10:00:00Z"}"#);
        assert_eq!(queued.status, BuildStatus::Queued);
        assert_eq!(queued.started_at, None);
        let running = job(
            r#"{"name":"t","status":"in_progress","started_at":"2026-11-16T10:00:00Z","completed_at":"2026-11-16T10:00:00Z"}"#,
        );
        assert!(running.started_at.is_some());
        assert_eq!(running.finished_at, None, "a stray completed_at is ignored");
    }
}
