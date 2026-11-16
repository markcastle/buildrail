//! The pure model of what the strip shows.
//!
//! [`Board`] holds the pinned watches, one state per distinct
//! [`RequestKey`] (so watches sharing a request always agree) and the
//! snapshots loaded from the cache at startup. It turns fetch results into
//! [`WatchSnapshot`]s in pinned order. No clock, no I/O: the engine passes
//! the wall-clock time of each result in.
//!
//! # Cached snapshots
//!
//! Snapshots loaded from the [`SnapshotCache`](crate::ports::SnapshotCache)
//! are shown with their record and `last_success` (so the UI can show the
//! age) but with [`Freshness::Pending`], whatever freshness was saved:
//! nothing has been checked *this session* yet, and claiming `Stale` would
//! invent a failure reason. If the first fetch fails, the cached record
//! stays visible and the freshness becomes `Stale { reason }`.
//!
//! The cache does not record which target a snapshot was for, so a cached
//! record is display-only: it is never used as the baseline for
//! [`selection::merge`]. The first successful fetch for a key is
//! authoritative and replaces it. A cached snapshot is dropped as soon as
//! its watch is edited to point at a different request.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::domain::{
    BuildRecord, Freshness, RequestKey, StaleReason, Watch, WatchId, WatchSnapshot,
};
use crate::selection;

/// What is known about one shared request.
#[derive(Debug, Clone)]
struct KeyState {
    record: Option<BuildRecord>,
    freshness: Freshness,
    last_success: Option<DateTime<Utc>>,
    /// At least one fetch succeeded this session, so `record` is ours and
    /// not the cache's.
    confirmed: bool,
}

impl KeyState {
    fn pending() -> Self {
        Self {
            record: None,
            freshness: Freshness::Pending,
            last_success: None,
            confirmed: false,
        }
    }
}

/// The strip's state: pinned watches and what is known about each.
#[derive(Debug, Clone, Default)]
pub struct Board {
    watches: Vec<Watch>,
    keys: HashMap<RequestKey, KeyState>,
    seeds: HashMap<WatchId, WatchSnapshot>,
}

impl Board {
    /// A board with no watches yet, remembering `cached` snapshots to show
    /// for matching watch IDs once watches are set.
    pub fn new(cached: impl IntoIterator<Item = WatchSnapshot>) -> Self {
        Self {
            watches: Vec::new(),
            keys: HashMap::new(),
            seeds: cached
                .into_iter()
                .map(|s| (s.watch_id.clone(), s))
                .collect(),
        }
    }

    /// Replaces the pinned watches (profile loaded or edited).
    ///
    /// State for requests still in use is kept, so reordering, renaming or
    /// adding a watch does not blank the others. State for requests no
    /// longer used is dropped. A watch whose target or connection changed
    /// loses its cached snapshot.
    pub fn set_watches(&mut self, watches: Vec<Watch>) {
        let previous: HashMap<&WatchId, RequestKey> = self
            .watches
            .iter()
            .map(|w| (&w.id, RequestKey::from(w)))
            .collect();
        let mut seeds = std::mem::take(&mut self.seeds);
        seeds.retain(|id, _| {
            watches.iter().any(|w| {
                &w.id == id
                    && previous
                        .get(id)
                        .is_none_or(|old_key| *old_key == RequestKey::from(w))
            })
        });
        self.seeds = seeds;

        let mut old_keys = std::mem::take(&mut self.keys);
        for key in watches.iter().map(RequestKey::from) {
            let state = old_keys.remove(&key);
            self.keys
                .entry(key)
                .or_insert_with(|| state.unwrap_or_else(KeyState::pending));
        }
        self.watches = watches;
    }

    /// The pinned watches, in order.
    pub fn watches(&self) -> &[Watch] {
        &self.watches
    }

    /// Distinct requests needed to serve the watches, in pinned order of
    /// first use.
    pub fn request_keys(&self) -> Vec<RequestKey> {
        let mut keys: Vec<RequestKey> = Vec::with_capacity(self.watches.len());
        for key in self.watches.iter().map(RequestKey::from) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys
    }

    /// Applies a successful fetch for `key` at wall time `now`.
    ///
    /// Returns whether the selected run is active (queued / running), which
    /// picks the poll cadence, or `None` if no watch uses `key` any more
    /// (a late result for a removed watch is ignored).
    pub fn apply_success(
        &mut self,
        key: &RequestKey,
        candidates: Vec<BuildRecord>,
        now: DateTime<Utc>,
    ) -> Option<bool> {
        let state = self.keys.get_mut(key)?;
        let incoming = selection::select_latest(candidates);
        let baseline = if state.confirmed {
            state.record.as_ref()
        } else {
            None
        };
        state.record = selection::merge(baseline, incoming);
        state.freshness = Freshness::Fresh;
        state.last_success = Some(now);
        state.confirmed = true;
        let active = state.record.as_ref().is_some_and(|r| r.status.is_active());
        let seeds = &mut self.seeds;
        for watch in self.watches.iter().filter(|w| RequestKey::from(*w) == *key) {
            seeds.remove(&watch.id);
        }
        Some(active)
    }

    /// Applies a failed fetch for `key`: whatever is shown stays, marked
    /// stale with `reason`. Ignored if no watch uses `key` any more.
    pub fn apply_failure(&mut self, key: &RequestKey, reason: StaleReason) {
        if let Some(state) = self.keys.get_mut(key) {
            state.freshness = Freshness::Stale { reason };
        }
    }

    /// One snapshot per watch, in pinned order.
    pub fn snapshots(&self) -> Vec<WatchSnapshot> {
        self.watches
            .iter()
            .map(|watch| self.snapshot_for(watch))
            .collect()
    }

    fn snapshot_for(&self, watch: &Watch) -> WatchSnapshot {
        let state = self.keys.get(&RequestKey::from(watch));
        let seed = self.seeds.get(&watch.id);
        match state {
            Some(state) if state.confirmed => WatchSnapshot {
                watch_id: watch.id.clone(),
                record: state.record.clone(),
                freshness: state.freshness.clone(),
                last_success: state.last_success,
            },
            _ => WatchSnapshot {
                watch_id: watch.id.clone(),
                record: seed.and_then(|s| s.record.clone()),
                freshness: state.map_or(Freshness::Pending, |s| s.freshness.clone()),
                last_success: seed.and_then(|s| s.last_success),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use chrono::TimeZone;

    use super::*;
    use crate::domain::{BuildStatus, ConnectionAlias, RunId, WatchTarget};

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn watch(id: &str, repo: &str) -> Watch {
        Watch {
            id: WatchId::new(id),
            label: id.to_uppercase(),
            connection: ConnectionAlias::new("work"),
            target: WatchTarget::GitHub {
                owner: "o".into(),
                repo: repo.into(),
                workflow: "ci.yml".into(),
                branch: None,
            },
        }
    }

    fn rec(run: &str, created: i64, status: BuildStatus) -> BuildRecord {
        BuildRecord {
            run_id: RunId::new(run),
            attempt: 1,
            status,
            url: String::new(),
            branch: None,
            commit: None,
            failing_job: None,
            created_at: t(created),
            updated_at: t(created),
            observed_at: t(created),
        }
    }

    fn cached(id: &str, run: &str) -> WatchSnapshot {
        WatchSnapshot {
            watch_id: WatchId::new(id),
            record: Some(rec(run, 0, BuildStatus::Passed)),
            freshness: Freshness::Fresh,
            last_success: Some(t(-100)),
        }
    }

    #[test]
    fn empty_board_has_no_snapshots() {
        let board = Board::default();
        assert!(board.snapshots().is_empty());
        assert!(board.watches().is_empty());
        assert!(board.request_keys().is_empty());
    }

    #[test]
    fn watches_start_pending_in_pinned_order() {
        let mut board = Board::new(Vec::new());
        board.set_watches(vec![watch("b", "rb"), watch("a", "ra")]);
        let snaps = board.snapshots();
        assert_eq!(
            snaps,
            vec![
                WatchSnapshot::pending(WatchId::new("b")),
                WatchSnapshot::pending(WatchId::new("a")),
            ]
        );
        assert_eq!(board.watches().len(), 2);
    }

    #[test]
    fn shared_requests_are_deduplicated_and_ordered() {
        let mut board = Board::new(Vec::new());
        board.set_watches(vec![watch("1", "x"), watch("2", "y"), watch("3", "x")]);
        let keys = board.request_keys();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0], RequestKey::from(&watch("1", "x")));
        assert_eq!(keys[1], RequestKey::from(&watch("2", "y")));
    }

    #[test]
    fn success_updates_every_watch_sharing_the_key() {
        let mut board = Board::new(Vec::new());
        board.set_watches(vec![watch("1", "x"), watch("2", "y"), watch("3", "x")]);
        let key = RequestKey::from(&watch("1", "x"));
        let active = board.apply_success(
            &key,
            vec![
                rec("old", 0, BuildStatus::Passed),
                rec("new", 10, BuildStatus::Running),
            ],
            t(20),
        );
        assert_eq!(active, Some(true));
        let snaps = board.snapshots();
        for i in [0, 2] {
            assert_eq!(snaps[i].record.as_ref().unwrap().run_id, RunId::new("new"));
            assert_eq!(snaps[i].freshness, Freshness::Fresh);
            assert_eq!(snaps[i].last_success, Some(t(20)));
        }
        assert_eq!(snaps[1].freshness, Freshness::Pending);
    }

    #[test]
    fn empty_history_is_fresh_with_no_record() {
        let mut board = Board::new(Vec::new());
        board.set_watches(vec![watch("1", "x")]);
        let key = RequestKey::from(&watch("1", "x"));
        assert_eq!(board.apply_success(&key, Vec::new(), t(5)), Some(false));
        let snap = &board.snapshots()[0];
        assert_eq!(snap.record, None);
        assert_eq!(snap.freshness, Freshness::Fresh);
        assert_eq!(snap.last_success, Some(t(5)));
    }

    #[test]
    fn later_success_merges_so_older_runs_never_win() {
        let mut board = Board::new(Vec::new());
        board.set_watches(vec![watch("1", "x")]);
        let key = RequestKey::from(&watch("1", "x"));
        board.apply_success(&key, vec![rec("new", 10, BuildStatus::Running)], t(10));
        // Provider list momentarily lacks the newest run.
        let active = board.apply_success(&key, vec![rec("old", 0, BuildStatus::Failed)], t(20));
        assert_eq!(active, Some(true));
        let snap = &board.snapshots()[0];
        assert_eq!(snap.record.as_ref().unwrap().run_id, RunId::new("new"));
        assert_eq!(snap.last_success, Some(t(20)));
    }

    #[test]
    fn failure_keeps_record_and_last_success() {
        let mut board = Board::new(Vec::new());
        board.set_watches(vec![watch("1", "x")]);
        let key = RequestKey::from(&watch("1", "x"));
        board.apply_success(&key, vec![rec("r", 0, BuildStatus::Passed)], t(10));
        board.apply_failure(&key, StaleReason::Offline);
        let snap = &board.snapshots()[0];
        assert_eq!(snap.record.as_ref().unwrap().run_id, RunId::new("r"));
        assert_eq!(
            snap.freshness,
            Freshness::Stale {
                reason: StaleReason::Offline
            }
        );
        assert_eq!(snap.last_success, Some(t(10)));
    }

    #[test]
    fn results_for_unknown_keys_are_ignored() {
        let mut board = Board::new(Vec::new());
        board.set_watches(vec![watch("1", "x")]);
        let gone = RequestKey::from(&watch("9", "gone"));
        assert_eq!(board.apply_success(&gone, Vec::new(), t(0)), None);
        board.apply_failure(&gone, StaleReason::NotFound);
        assert_eq!(
            board.snapshots(),
            vec![WatchSnapshot::pending(WatchId::new("1"))]
        );
    }

    #[test]
    fn cached_snapshots_show_as_pending_then_stale_then_replaced() {
        let mut board = Board::new(vec![cached("1", "cached"), cached("other", "zzz")]);
        board.set_watches(vec![watch("1", "x")]);
        let snap = &board.snapshots()[0];
        assert_eq!(snap.record.as_ref().unwrap().run_id, RunId::new("cached"));
        assert_eq!(
            snap.freshness,
            Freshness::Pending,
            "saved Fresh is not trusted"
        );
        assert_eq!(snap.last_success, Some(t(-100)));

        let key = RequestKey::from(&watch("1", "x"));
        board.apply_failure(&key, StaleReason::Offline);
        let snap = &board.snapshots()[0];
        assert_eq!(snap.record.as_ref().unwrap().run_id, RunId::new("cached"));
        assert_eq!(
            snap.freshness,
            Freshness::Stale {
                reason: StaleReason::Offline
            }
        );
        assert_eq!(snap.last_success, Some(t(-100)));

        // A first success is authoritative even if the cached run looks newer
        // (cached run was created at t(0), the real one at t(-50)).
        board.apply_success(&key, vec![rec("real", -50, BuildStatus::Failed)], t(50));
        let snap = &board.snapshots()[0];
        assert_eq!(snap.record.as_ref().unwrap().run_id, RunId::new("real"));
        assert_eq!(snap.freshness, Freshness::Fresh);
        assert_eq!(snap.last_success, Some(t(50)));
    }

    #[test]
    fn cached_snapshot_dropped_when_watch_retargeted_or_removed() {
        let mut board = Board::new(vec![cached("1", "c1"), cached("2", "c2")]);
        board.set_watches(vec![watch("1", "x"), watch("2", "y")]);
        // Rename only (same key) keeps the seed; retarget drops it.
        let mut renamed = watch("1", "x");
        renamed.label = "Renamed".into();
        board.set_watches(vec![watch("2", "other"), renamed]);
        let snaps = board.snapshots();
        assert_eq!(snaps[0].watch_id, WatchId::new("2"));
        assert_eq!(snaps[0].record, None);
        assert_eq!(snaps[0].last_success, None);
        assert_eq!(snaps[1].record.as_ref().unwrap().run_id, RunId::new("c1"));
        // Removing and re-adding loses the seed too.
        board.set_watches(Vec::new());
        board.set_watches(vec![watch("1", "x")]);
        assert_eq!(board.snapshots()[0].record, None);
    }

    #[test]
    fn set_watches_keeps_state_for_unchanged_keys() {
        let mut board = Board::new(Vec::new());
        board.set_watches(vec![watch("1", "x"), watch("2", "y")]);
        let key = RequestKey::from(&watch("1", "x"));
        board.apply_success(&key, vec![rec("r", 0, BuildStatus::Passed)], t(1));
        board.set_watches(vec![watch("3", "z"), watch("1", "x")]);
        let snaps = board.snapshots();
        assert_eq!(snaps[0], WatchSnapshot::pending(WatchId::new("3")));
        assert_eq!(snaps[1].freshness, Freshness::Fresh);
        // A newly added watch on an already-known key shows its data at once.
        board.set_watches(vec![watch("1", "x"), watch("4", "x")]);
        assert_eq!(board.snapshots()[1].freshness, Freshness::Fresh);
    }
}
