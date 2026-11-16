//! Run selection: which run "wins" for a watch, and when a new observation
//! may replace the one already on screen.
//!
//! Everything here is a pure function over [`BuildRecord`]s so the rules can
//! be tested exhaustively without a provider, a clock or a runtime.
//!
//! # The rules
//!
//! * **Different runs** are ordered by the provider's `created_at`. A run
//!   created later is newer, however late an older run finishes. This is
//!   what stops an old, slow build from overwriting the result of a newer
//!   one that already finished.
//! * **The same run** (equal `run_id`) is ordered by `attempt` first: a
//!   re-run keeps the run ID and bumps the attempt, so attempt 2 replaces
//!   attempt 1, never the reverse. Within one attempt the observation with
//!   the later `updated_at` wins, and an *equal* `updated_at` lets the
//!   incoming observation through (it is at least as new, and it carries a
//!   fresher `observed_at`).
//! * Ties between different runs with an equal `created_at` are broken by
//!   `attempt`, then `updated_at`, then `run_id`, purely so the choice is
//!   deterministic across polls (no flicker between two equal candidates).

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::domain::{BuildRecord, RunId};

/// Orders two observations of the *same* run (`run_id` equal): attempt
/// first, then `updated_at`.
fn same_run_cmp(a: &BuildRecord, b: &BuildRecord) -> Ordering {
    a.attempt
        .cmp(&b.attempt)
        .then_with(|| a.updated_at.cmp(&b.updated_at))
}

/// Orders two *different* runs by recency, with the deterministic
/// tie-breakers described in the module docs.
fn different_run_cmp(a: &BuildRecord, b: &BuildRecord) -> Ordering {
    a.created_at
        .cmp(&b.created_at)
        .then_with(|| a.attempt.cmp(&b.attempt))
        .then_with(|| a.updated_at.cmp(&b.updated_at))
        .then_with(|| a.run_id.cmp(&b.run_id))
}

/// `true` when `incoming` may replace `current` on screen.
///
/// This is the single place that encodes "an older run finishing late never
/// replaces a newer one" and "a higher attempt replaces a lower one, never
/// the reverse". See the module docs for the full rules.
pub fn supersedes(incoming: &BuildRecord, current: &BuildRecord) -> bool {
    if incoming.run_id == current.run_id {
        same_run_cmp(incoming, current) != Ordering::Less
    } else {
        different_run_cmp(incoming, current) == Ordering::Greater
    }
}

/// Picks the newest run from a provider's candidate list, or `None` when the
/// list is empty (the target has no matching runs).
///
/// Providers may return several observations of the same run (for example
/// one per attempt); those are first collapsed to the newest attempt, so the
/// result is independent of the order the provider used.
pub fn select_latest(candidates: impl IntoIterator<Item = BuildRecord>) -> Option<BuildRecord> {
    let mut per_run: HashMap<RunId, BuildRecord> = HashMap::new();
    for candidate in candidates {
        match per_run.get(&candidate.run_id) {
            Some(existing) if same_run_cmp(&candidate, existing) != Ordering::Greater => {}
            _ => {
                per_run.insert(candidate.run_id.clone(), candidate);
            }
        }
    }
    per_run.into_values().max_by(different_run_cmp)
}

/// Combines the record currently held for a watch with the newest record
/// from a fresh, *successful* fetch.
///
/// * No current record: take whatever arrived.
/// * Incoming `None` (the provider returned an empty history although we
///   held a record): the result is `None`. An empty list is an authoritative
///   answer from the provider ("no matching runs", e.g. the runs were
///   deleted or the branch was removed), and continuing to show a run that
///   no longer exists as *fresh* would be misleading. Failed fetches never
///   reach this function, so connectivity problems cannot blank the strip.
/// * Both present: keep the current one unless the incoming one
///   [`supersedes`] it. This is what protects against an older run that
///   finishes late, or a provider whose list briefly omits the newest run.
pub fn merge(current: Option<&BuildRecord>, incoming: Option<BuildRecord>) -> Option<BuildRecord> {
    match (current, incoming) {
        (Some(current), Some(incoming)) => {
            if supersedes(&incoming, current) {
                Some(incoming)
            } else {
                Some(current.clone())
            }
        }
        (_, incoming) => incoming,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use chrono::{DateTime, TimeZone, Utc};

    use super::*;
    use crate::domain::BuildStatus;

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn rec(run: &str, attempt: u32, created: i64, updated: i64) -> BuildRecord {
        BuildRecord {
            run_id: RunId::new(run),
            attempt,
            status: BuildStatus::Passed,
            url: format!("https://example.test/{run}/{attempt}"),
            branch: Some("main".into()),
            commit: None,
            failing_job: None,
            created_at: t(created),
            updated_at: t(updated),
            observed_at: t(updated),
        }
    }

    #[test]
    fn select_latest_of_empty_is_none() {
        assert_eq!(select_latest(Vec::new()), None);
    }

    #[test]
    fn select_latest_picks_newest_created_regardless_of_order() {
        let a = rec("1", 1, 0, 500); // old run that finished late
        let b = rec("2", 1, 10, 20);
        let c = rec("3", 1, 5, 30);
        for order in [
            vec![a.clone(), b.clone(), c.clone()],
            vec![c.clone(), b.clone(), a.clone()],
            vec![b.clone(), a.clone(), c.clone()],
        ] {
            assert_eq!(select_latest(order).unwrap().run_id, RunId::new("2"));
        }
    }

    #[test]
    fn select_latest_prefers_highest_attempt_of_same_run() {
        let first = rec("7", 1, 0, 100);
        let rerun = rec("7", 2, 0, 50);
        assert_eq!(
            select_latest(vec![rerun.clone(), first.clone()]).unwrap(),
            rerun
        );
        assert_eq!(select_latest(vec![first, rerun.clone()]).unwrap(), rerun);
    }

    #[test]
    fn select_latest_same_run_same_attempt_takes_later_update() {
        let older = rec("7", 1, 0, 10);
        let newer = rec("7", 1, 0, 20);
        assert_eq!(
            select_latest(vec![newer.clone(), older.clone()]).unwrap(),
            newer
        );
        assert_eq!(select_latest(vec![older, newer.clone()]).unwrap(), newer);
    }

    #[test]
    fn select_latest_ties_are_deterministic() {
        // Same created_at, different runs: attempt, then updated_at, then id.
        let a = rec("a", 1, 0, 10);
        let b = rec("b", 2, 0, 5);
        assert_eq!(select_latest(vec![a.clone(), b.clone()]).unwrap(), b);
        let c = rec("c", 1, 0, 20);
        assert_eq!(select_latest(vec![c.clone(), a.clone()]).unwrap(), c);
        let d = rec("d", 1, 0, 10);
        assert_eq!(select_latest(vec![d.clone(), a.clone()]).unwrap(), d);
        assert_eq!(select_latest(vec![a, d.clone()]).unwrap(), d);
    }

    #[test]
    fn older_run_finishing_late_does_not_supersede_newer_run() {
        let newer = rec("2", 1, 10, 20);
        let late_old = rec("1", 1, 0, 999);
        assert!(!supersedes(&late_old, &newer));
        assert!(supersedes(&newer, &late_old));
    }

    #[test]
    fn higher_attempt_supersedes_lower_never_reverse() {
        let a1 = rec("1", 1, 0, 100);
        let a2 = rec("1", 2, 0, 50);
        assert!(supersedes(&a2, &a1));
        assert!(!supersedes(&a1, &a2));
    }

    #[test]
    fn same_attempt_takes_incoming_unless_older_update() {
        let cur = rec("1", 1, 0, 10);
        assert!(supersedes(&rec("1", 1, 0, 10), &cur), "equal is accepted");
        assert!(supersedes(&rec("1", 1, 0, 11), &cur));
        assert!(!supersedes(&rec("1", 1, 0, 9), &cur));
    }

    #[test]
    fn identical_different_run_does_not_supersede_itself_by_tie() {
        let a = rec("a", 1, 0, 0);
        let b = rec("b", 1, 0, 0);
        assert!(supersedes(&b, &a));
        assert!(!supersedes(&a, &b));
    }

    #[test]
    fn merge_without_current_takes_incoming() {
        let r = rec("1", 1, 0, 0);
        assert_eq!(merge(None, Some(r.clone())), Some(r));
        assert_eq!(merge(None, None), None);
    }

    #[test]
    fn merge_with_empty_history_clears_record() {
        let r = rec("1", 1, 0, 0);
        assert_eq!(merge(Some(&r), None), None);
    }

    #[test]
    fn merge_keeps_current_when_incoming_is_older() {
        let current = rec("2", 1, 10, 10);
        let incoming = rec("1", 1, 0, 50);
        assert_eq!(merge(Some(&current), Some(incoming)), Some(current));
    }

    #[test]
    fn merge_takes_newer_incoming() {
        let current = rec("1", 1, 0, 0);
        let mut incoming = rec("1", 1, 0, 5);
        incoming.status = BuildStatus::Failed;
        assert_eq!(
            merge(Some(&current), Some(incoming.clone())),
            Some(incoming)
        );
    }
}
