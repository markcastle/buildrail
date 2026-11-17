//! When the snapshot cache is worth writing.
//!
//! Every successful poll moves two timestamps (`last_success` and the
//! record's `observed_at`) even when nothing else changed, so writing the
//! cache on every published change would rewrite the file every 15–60 s
//! for as long as the app runs. A restart only needs what the strip would
//! *say* from the cache: which run, its status and details, and whether
//! the data was fresh, pending or stale (and why). So:
//!
//! * a **material** change (a watch added, removed or reordered; another
//!   run or attempt; a status, branch, commit, failing job or link; a
//!   different freshness) is written straight away;
//! * a change of timestamps only is written at most once every
//!   [`SavePolicy::drift_interval`], so the cached ages stay roughly right;
//! * whatever was held back is written when the engine stops
//!   ([`SaveTracker::flush`]).
//!
//! Pure: the caller passes the instant in.

use std::time::Duration;

use tokio::time::Instant;

use crate::domain::{BuildRecord, WatchSnapshot};

/// How often timestamp-only changes reach the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavePolicy {
    /// Timestamp-only changes are written at most this often.
    pub drift_interval: Duration,
}

impl Default for SavePolicy {
    /// Five minutes: ages in a cache read after a crash are at most that
    /// much off, and the file is written a dozen times an hour, not 240.
    fn default() -> Self {
        Self {
            drift_interval: Duration::from_secs(5 * 60),
        }
    }
}

/// Remembers what was written last and decides whether new snapshots
/// should be.
#[derive(Debug, Clone)]
pub struct SaveTracker {
    policy: SavePolicy,
    saved: Option<Vec<WatchSnapshot>>,
    saved_at: Option<Instant>,
    held_back: bool,
}

impl SaveTracker {
    /// Nothing written yet: the first snapshots are always written.
    pub fn new(policy: SavePolicy) -> Self {
        Self {
            policy,
            saved: None,
            saved_at: None,
            held_back: false,
        }
    }

    /// Whether `snapshots`, published at `now`, should be written. When it
    /// says yes it assumes the caller writes them.
    pub fn should_save(&mut self, snapshots: &[WatchSnapshot], now: Instant) -> bool {
        let save = match &self.saved {
            None => true,
            Some(saved) if saved.as_slice() == snapshots => false,
            Some(saved) if differs_materially(saved, snapshots) => true,
            Some(_) => self
                .saved_at
                .is_none_or(|at| now.saturating_duration_since(at) >= self.policy.drift_interval),
        };
        if save {
            self.saved = Some(snapshots.to_vec());
            self.saved_at = Some(now);
            self.held_back = false;
        } else if self.saved.as_deref() != Some(snapshots) {
            self.held_back = true;
        }
        save
    }

    /// Whether something newer than the file was held back (and should be
    /// written on the way out).
    pub fn has_unsaved(&self) -> bool {
        self.held_back
    }

    /// Call when the engine stops with the latest snapshots: `true` when
    /// they should be written now because a timestamp-only change was
    /// held back.
    pub fn flush(&mut self, snapshots: &[WatchSnapshot], now: Instant) -> bool {
        if !self.held_back {
            return false;
        }
        self.saved = Some(snapshots.to_vec());
        self.saved_at = Some(now);
        self.held_back = false;
        true
    }
}

/// Whether two snapshot lists differ in anything a restarted app would
/// show differently, ignoring `last_success` and `observed_at`.
pub fn differs_materially(a: &[WatchSnapshot], b: &[WatchSnapshot]) -> bool {
    a.len() != b.len()
        || a.iter().zip(b).any(|(a, b)| {
            a.watch_id != b.watch_id
                || a.freshness != b.freshness
                || !same_record(a.record.as_ref(), b.record.as_ref())
        })
}

fn same_record(a: Option<&BuildRecord>, b: Option<&BuildRecord>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.run_id == b.run_id
                && a.attempt == b.attempt
                && a.status == b.status
                && a.url == b.url
                && a.branch == b.branch
                && a.commit == b.commit
                && a.failing_job == b.failing_job
                && a.created_at == b.created_at
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::{TimeDelta, TimeZone, Utc};

    use super::*;
    use crate::domain::{BuildStatus, Freshness, RunId, StaleReason, WatchId};

    fn snap(id: &str, status: BuildStatus, seen: i64) -> WatchSnapshot {
        let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let seen_at = at + TimeDelta::seconds(seen);
        WatchSnapshot {
            watch_id: WatchId::new(id),
            record: Some(BuildRecord {
                run_id: RunId::new("1"),
                attempt: 1,
                status,
                url: "https://example.com/1".into(),
                branch: Some("main".into()),
                commit: None,
                failing_job: None,
                created_at: at,
                updated_at: at,
                observed_at: seen_at,
            }),
            freshness: Freshness::Fresh,
            last_success: Some(seen_at),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn first_and_material_changes_save_at_once_drift_waits() {
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);
        let mut tracker = SaveTracker::new(SavePolicy::default());
        assert!(tracker.should_save(&[snap("w", BuildStatus::Passed, 0)], at(0)));
        // Same snapshots: nothing to do.
        assert!(!tracker.should_save(&[snap("w", BuildStatus::Passed, 0)], at(1)));
        assert!(!tracker.has_unsaved());
        // Only the timestamps moved: held back until five minutes passed.
        for secs in [60, 120, 299] {
            let s = [snap("w", BuildStatus::Passed, secs)];
            assert!(!tracker.should_save(&s, at(secs as u64)), "{secs}");
            assert!(tracker.has_unsaved());
        }
        assert!(tracker.should_save(&[snap("w", BuildStatus::Passed, 300)], at(300)));
        assert!(!tracker.has_unsaved());
        // A status change is written straight away.
        assert!(tracker.should_save(&[snap("w", BuildStatus::Running, 310)], at(310)));
        // As is a stale marker, a new watch, a removed record.
        let mut stale = snap("w", BuildStatus::Running, 311);
        stale.freshness = Freshness::Stale {
            reason: StaleReason::Offline,
        };
        assert!(tracker.should_save(&[stale.clone()], at(311)));
        let two = [stale.clone(), snap("x", BuildStatus::Passed, 311)];
        assert!(tracker.should_save(&two, at(312)));
        let mut empty = stale;
        empty.record = None;
        assert!(tracker.should_save(&[empty], at(313)));
    }

    #[tokio::test(start_paused = true)]
    async fn flush_writes_only_what_was_held_back() {
        let t0 = Instant::now();
        let mut tracker = SaveTracker::new(SavePolicy {
            drift_interval: Duration::from_secs(60),
        });
        let first = [snap("w", BuildStatus::Passed, 0)];
        assert!(!tracker.flush(&first, t0), "nothing held back yet");
        assert!(tracker.should_save(&first, t0));
        let later = [snap("w", BuildStatus::Passed, 15)];
        assert!(!tracker.should_save(&later, t0 + Duration::from_secs(15)));
        assert!(tracker.flush(&later, t0 + Duration::from_secs(16)));
        assert!(!tracker.flush(&later, t0 + Duration::from_secs(17)));
        assert!(!tracker.has_unsaved());
    }

    #[test]
    fn material_comparison_ignores_only_timestamps_we_set() {
        let a = vec![snap("w", BuildStatus::Passed, 0)];
        let mut b = vec![snap("w", BuildStatus::Passed, 99)];
        assert!(!differs_materially(&a, &b));
        b[0].record.as_mut().unwrap().attempt = 2;
        assert!(differs_materially(&a, &b));
        let c = vec![snap("v", BuildStatus::Passed, 0)];
        assert!(differs_materially(&a, &c));
        assert!(differs_materially(&a, &[]));
        let mut none = a.clone();
        none[0].record = None;
        assert!(!differs_materially(&none, &none));
        assert!(differs_materially(&none, &a));
        assert_eq!(SavePolicy::default().drift_interval.as_secs(), 300);
    }
}
