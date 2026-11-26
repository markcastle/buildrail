//! The details of one run: what the details panel shows beyond the strip's
//! [`BuildRecord`].
//!
//! Plain, provider-neutral, serialisable data. Providers fill in what they
//! know and leave the rest `None`; the app decides the wording. Details are
//! fetched on demand (a person opened the panel or pressed Refresh), never
//! by the refresh engine, through the
//! [`RunDetailsSource`](crate::ports::RunDetailsSource) port.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::{BuildRecord, BuildStatus, RunId};

/// Which run (and which attempt of it) details are asked for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunRef {
    /// Provider's run identifier.
    pub run_id: RunId,
    /// Attempt number, starting at 1.
    pub attempt: u32,
}

impl From<&BuildRecord> for RunRef {
    fn from(record: &BuildRecord) -> Self {
        Self {
            run_id: record.run_id.clone(),
            attempt: record.attempt.max(1),
        }
    }
}

/// What started a run, in provider-neutral terms.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Trigger {
    /// A push to the repository.
    Push,
    /// A pull request was opened or updated.
    PullRequest,
    /// A schedule (cron).
    Schedule,
    /// Someone started it by hand (GitHub's `workflow_dispatch`, a
    /// dashboard button).
    Manual,
    /// Something called the provider's API or a deploy hook.
    Api,
    /// Another workflow finishing (GitHub's `workflow_run`).
    Workflow,
    /// A trigger with no neutral name; the provider's own word.
    Other(String),
    /// The provider did not say.
    Unknown,
}

/// One job of a run (GitHub), with the step that failed when one did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSummary {
    /// The job's name as the provider shows it (`test (ubuntu-latest)`).
    pub name: String,
    /// Normalised status.
    pub status: BuildStatus,
    /// When the job started, if it has.
    pub started_at: Option<DateTime<Utc>>,
    /// When the job finished, if it has.
    pub finished_at: Option<DateTime<Utc>>,
    /// The job's own page, when the provider has one.
    pub url: Option<String>,
    /// Name of the first failed step, for a failed job, when known.
    pub failing_step: Option<String>,
}

/// Everything known about one run attempt beyond its [`BuildRecord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDetails {
    /// The run (and attempt) these details describe.
    pub run: RunRef,
    /// The run's title, when the provider gives one (GitHub's
    /// `display_title`: usually the commit or pull request title).
    pub title: Option<String>,
    /// The full commit message, when the provider gives one. The app shows
    /// its first line.
    pub commit_message: Option<String>,
    /// Who started the run (the original author of the event).
    pub actor: Option<String>,
    /// Who started *this attempt*, when it differs (a re-run).
    pub triggered_by: Option<String>,
    /// What started it.
    pub trigger: Trigger,
    /// The provider's human-facing run number (`#562`).
    pub run_number: Option<u64>,
    /// When this attempt started running.
    pub started_at: Option<DateTime<Utc>>,
    /// When this attempt finished, if it has.
    pub finished_at: Option<DateTime<Utc>>,
    /// The jobs of this attempt, in the provider's order. `None` when the
    /// provider has no jobs (Cloudflare Workers Builds); `Some(empty)` when
    /// it has them but listed none yet.
    pub jobs: Option<Vec<JobSummary>>,
    /// How many jobs the provider says there are, which may be more than
    /// `jobs` holds when the list was capped.
    pub jobs_total: usize,
}

impl RunDetails {
    /// Details with nothing but the run reference, for adapters to fill.
    pub fn new(run: RunRef) -> Self {
        Self {
            run,
            title: None,
            commit_message: None,
            actor: None,
            triggered_by: None,
            trigger: Trigger::Unknown,
            run_number: None,
            started_at: None,
            finished_at: None,
            jobs: None,
            jobs_total: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::TimeZone;

    use super::*;

    #[test]
    fn run_ref_from_record_and_serialised_shapes() {
        let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let record = BuildRecord {
            run_id: RunId::new("7"),
            attempt: 0,
            status: BuildStatus::Failed,
            url: String::new(),
            branch: None,
            commit: None,
            failing_job: None,
            created_at: at,
            updated_at: at,
            observed_at: at,
        };
        let run = RunRef::from(&record);
        assert_eq!(run.attempt, 1, "attempts start at 1");
        let mut details = RunDetails::new(run.clone());
        details.trigger = Trigger::Other("release".into());
        details.jobs = Some(vec![JobSummary {
            name: "test".into(),
            status: BuildStatus::Failed,
            started_at: Some(at),
            finished_at: None,
            url: None,
            failing_step: Some("cargo test".into()),
        }]);
        let json = serde_json::to_value(&details).unwrap();
        assert_eq!(json["trigger"]["kind"], "other");
        assert_eq!(json["trigger"]["name"], "release");
        assert_eq!(json["jobs"][0]["status"], "failed");
        let back: RunDetails = serde_json::from_value(json).unwrap();
        assert_eq!(back, details);
        let push = serde_json::to_value(Trigger::Push).unwrap();
        assert_eq!(push["kind"], "push");
    }
}
