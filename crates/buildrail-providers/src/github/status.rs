//! Pure mapping from GitHub's `status` / `conclusion` pair to
//! [`BuildStatus`].
//!
//! GitHub reports a workflow run with two fields
//! (<https://docs.github.com/en/rest/actions/workflow-runs>):
//!
//! * `status`: `requested`, `queued`, `pending`, `waiting` (all "not started
//!   yet": `waiting` is a deployment-protection wait, `pending` is a
//!   concurrency-group wait), `in_progress`, `completed`.
//! * `conclusion` (only meaningful once `completed`): `success`, `failure`,
//!   `timed_out`, `startup_failure`, `cancelled`, `skipped`, `neutral`,
//!   `action_required`, `stale`, or `null`.

use buildrail_core::details::Trigger;
use buildrail_core::domain::BuildStatus;

/// Maps a workflow run's `status` and `conclusion` to a [`BuildStatus`].
///
/// Unknown values map to [`BuildStatus::Unavailable`] rather than guessing,
/// so a new GitHub state shows as neutral instead of falsely green or red.
pub fn map_run_status(status: Option<&str>, conclusion: Option<&str>) -> BuildStatus {
    match status {
        Some("requested" | "queued" | "pending" | "waiting") => BuildStatus::Queued,
        Some("in_progress") => BuildStatus::Running,
        Some("completed") => map_conclusion(conclusion),
        // No/unknown status: trust a conclusion if GitHub sent one.
        _ if conclusion.is_some() => map_conclusion(conclusion),
        _ => BuildStatus::Unavailable,
    }
}

/// Maps a completed run's `conclusion`.
pub fn map_conclusion(conclusion: Option<&str>) -> BuildStatus {
    match conclusion {
        Some("success") => BuildStatus::Passed,
        Some("failure" | "timed_out" | "startup_failure") => BuildStatus::Failed,
        Some("cancelled") => BuildStatus::Cancelled,
        Some("skipped") => BuildStatus::Skipped,
        // `neutral`, `action_required` (needs approval), `stale` (GitHub gave
        // up on it) and anything new: not a verdict we can colour.
        _ => BuildStatus::Unavailable,
    }
}

/// Maps a workflow run's `event` to a provider-neutral [`Trigger`].
///
/// The events are GitHub's documented workflow triggers
/// (<https://docs.github.com/en/actions/writing-workflows/choosing-when-your-workflow-runs/events-that-trigger-workflows>);
/// the common ones get a neutral name and the rest keep GitHub's word.
pub fn map_event(event: Option<&str>) -> Trigger {
    match event {
        None | Some("") => Trigger::Unknown,
        Some("push") => Trigger::Push,
        Some("pull_request" | "pull_request_target" | "pull_request_review") => {
            Trigger::PullRequest
        }
        Some("schedule") => Trigger::Schedule,
        Some("workflow_dispatch") => Trigger::Manual,
        Some("repository_dispatch") => Trigger::Api,
        Some("workflow_run" | "workflow_call") => Trigger::Workflow,
        Some(other) => Trigger::Other(other.to_owned()),
    }
}

/// `true` for a *job* conclusion that should be named as the failing job.
pub(crate) fn is_failed_job(conclusion: Option<&str>) -> bool {
    matches!(
        conclusion,
        Some("failure" | "timed_out" | "startup_failure")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use BuildStatus::*;

    #[test]
    fn not_started_states_are_queued() {
        for s in ["requested", "queued", "pending", "waiting"] {
            assert_eq!(map_run_status(Some(s), None), Queued, "{s}");
        }
    }

    #[test]
    fn in_progress_is_running_even_with_stale_conclusion() {
        assert_eq!(map_run_status(Some("in_progress"), None), Running);
        assert_eq!(
            map_run_status(Some("in_progress"), Some("failure")),
            Running
        );
    }

    #[test]
    fn every_conclusion() {
        let cases = [
            ("success", Passed),
            ("failure", Failed),
            ("timed_out", Failed),
            ("startup_failure", Failed),
            ("cancelled", Cancelled),
            ("skipped", Skipped),
            ("neutral", Unavailable),
            ("action_required", Unavailable),
            ("stale", Unavailable),
            ("something_new", Unavailable),
        ];
        for (conclusion, expected) in cases {
            assert_eq!(
                map_run_status(Some("completed"), Some(conclusion)),
                expected,
                "{conclusion}"
            );
        }
        assert_eq!(map_run_status(Some("completed"), None), Unavailable);
    }

    #[test]
    fn unknown_or_missing_status() {
        assert_eq!(map_run_status(None, None), Unavailable);
        assert_eq!(map_run_status(Some("mystery"), None), Unavailable);
        assert_eq!(map_run_status(None, Some("success")), Passed);
    }

    #[test]
    fn events_map_to_triggers() {
        let cases = [
            (None, Trigger::Unknown),
            (Some(""), Trigger::Unknown),
            (Some("push"), Trigger::Push),
            (Some("pull_request"), Trigger::PullRequest),
            (Some("pull_request_target"), Trigger::PullRequest),
            (Some("schedule"), Trigger::Schedule),
            (Some("workflow_dispatch"), Trigger::Manual),
            (Some("repository_dispatch"), Trigger::Api),
            (Some("workflow_run"), Trigger::Workflow),
            (Some("release"), Trigger::Other("release".into())),
        ];
        for (event, expected) in cases {
            assert_eq!(map_event(event), expected, "{event:?}");
        }
    }

    #[test]
    fn failed_job_detection() {
        assert!(is_failed_job(Some("failure")));
        assert!(is_failed_job(Some("timed_out")));
        assert!(!is_failed_job(Some("cancelled")));
        assert!(!is_failed_job(None));
    }
}
