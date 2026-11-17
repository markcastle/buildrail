//! When the strip must be painted again, decided without a window.
//!
//! An idle strip paints nothing. It paints again only when something it
//! shows changes:
//!
//! * new snapshots that change what an entry says
//!   ([`snapshots_differ_visibly`]; a refresh that only moved "last
//!   confirmed" forward on fresh data changes nothing on screen);
//! * the clock passing a point where a relative age changes its words
//!   ("just now" → "1 min ago", "2 min ago" → "3 min ago", …), which
//!   [`RepaintPlanner`] works out from each entry's [`StripEntry::ages`];
//! * the in-progress glyph's next animation step, only when the user
//!   turned Animation on, the desktop does not ask for less motion and the
//!   strip is on screen ([`Motion`]).
//!
//! The UI asks the planner once per frame and schedules exactly one
//! `request_repaint_after` for the answer.

use std::time::Duration;

use buildrail_core::domain::{Freshness, WatchSnapshot};
use chrono::{DateTime, TimeDelta, Utc};

use super::strip::{StripEntry, StripModel};

/// Time between animation steps of the in-progress glyph (4 frames a
/// second at most).
pub const ANIMATION_STEP: Duration = Duration::from_millis(250);

/// A little slack after a text change, so a timer that fires a moment
/// early does not paint the old words and then paint again.
pub const CHANGE_SLACK: Duration = Duration::from_millis(20);

/// Whether the strip may animate this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Motion {
    /// The user's Animation preference.
    pub enabled: bool,
    /// The desktop asked for less motion.
    pub reduced: bool,
    /// The strip is on screen (not hidden for a fullscreen app).
    pub visible: bool,
}

impl Motion {
    /// `true` when the in-progress glyph should move.
    pub fn animates(self) -> bool {
        self.enabled && !self.reduced && self.visible
    }
}

/// Plans the strip's next paint. See the module docs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepaintPlanner {
    /// Whether and how the strip may animate.
    pub motion: Motion,
}

impl RepaintPlanner {
    /// A planner for `motion`.
    pub fn new(motion: Motion) -> Self {
        Self { motion }
    }

    /// `true` when `model` has something that animates and motion allows
    /// it.
    pub fn animating(&self, model: &StripModel) -> bool {
        self.motion.animates() && model.entries.iter().any(|e| e.icon.is_animated())
    }

    /// The next instant at which the displayed text of `model` changes on
    /// its own (an age crossing into new words), if any.
    pub fn next_text_change(model: &StripModel, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        model
            .entries
            .iter()
            .flat_map(|entry: &StripEntry| entry.ages.iter())
            .map(|&anchor| next_age_change(anchor, now))
            .min()
    }

    /// How long until the strip must be painted again, or `None` when
    /// nothing changes until new data arrives.
    pub fn next_repaint(&self, model: &StripModel, now: DateTime<Utc>) -> Option<Duration> {
        let text = Self::next_text_change(model, now).map(|at| {
            (at - now)
                .to_std()
                .unwrap_or(Duration::ZERO)
                .saturating_add(CHANGE_SLACK)
        });
        let animation = self.animating(model).then_some(ANIMATION_STEP);
        match (text, animation) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// The animation step at `time` seconds: the glyph advances one step
/// every [`ANIMATION_STEP`], so frames in between would look identical.
pub fn animation_phase(time: f64) -> u32 {
    let steps = (time.max(0.0) / ANIMATION_STEP.as_secs_f64()).floor();
    // Wraps after ~34 years of uptime; the glyph only uses it modulo 8.
    (steps % f64::from(u32::MAX)) as u32
}

/// The first instant after `now` at which the age of `anchor` reads
/// differently in [`super::age_short`] and [`super::age_long`] (both use
/// the same boundaries: under a minute, whole minutes, whole hours, whole
/// days).
pub fn next_age_change(anchor: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
    let secs = (now - anchor).num_seconds().max(0);
    let unit = match secs {
        0..3_600 => 60,
        3_600..86_400 => 3_600,
        _ => 86_400,
    };
    let next = (secs / unit + 1) * unit;
    anchor + TimeDelta::seconds(next)
}

/// `true` when the strip would say something different for `after` than
/// for `before`, at the same instant.
///
/// A refresh that confirms unchanged data still moves `last_success` (and
/// the record's `observed_at`) forward; on fresh data neither is shown, so
/// such a refresh is not a visible change. On stale or cached data "last
/// confirmed" is part of the note, so every field counts.
pub fn snapshots_differ_visibly(before: &[WatchSnapshot], after: &[WatchSnapshot]) -> bool {
    before.len() != after.len()
        || before
            .iter()
            .zip(after)
            .any(|(a, b)| visible_key(a) != visible_key(b))
}

fn visible_key(snapshot: &WatchSnapshot) -> WatchSnapshot {
    let mut key = snapshot.clone();
    if matches!(key.freshness, Freshness::Fresh) {
        key.last_success = None;
        if let Some(record) = &mut key.record {
            record.observed_at = record.updated_at;
        }
    }
    key
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use buildrail_core::domain::{
        BuildRecord, BuildStatus, ConnectionAlias, RunId, StaleReason, Watch, WatchId, WatchTarget,
    };
    use chrono::TimeZone;

    use super::*;
    use crate::presentation::{age_long, age_short};

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn watch(id: &str) -> Watch {
        Watch {
            id: WatchId::new(id),
            label: id.into(),
            connection: ConnectionAlias::new("work"),
            target: WatchTarget::GitHub {
                owner: "acme".into(),
                repo: id.into(),
                workflow: "ci.yml".into(),
                branch: None,
            },
        }
    }

    fn record(status: BuildStatus, at: DateTime<Utc>) -> BuildRecord {
        BuildRecord {
            run_id: RunId::new("1"),
            attempt: 1,
            status,
            url: "https://github.com/acme/x/actions/runs/1".into(),
            branch: None,
            commit: None,
            failing_job: None,
            created_at: at,
            updated_at: at,
            observed_at: at,
        }
    }

    fn snap(id: &str, record: Option<BuildRecord>, freshness: Freshness) -> WatchSnapshot {
        WatchSnapshot {
            watch_id: WatchId::new(id),
            last_success: record.as_ref().map(|r| r.observed_at),
            record,
            freshness,
        }
    }

    #[test]
    fn age_boundaries_match_the_words() {
        let anchor = t(0);
        for secs in [-90, -1, 0, 1, 59, 60, 61, 119, 3_599, 3_600, 7_000, 86_399, 86_400, 200_000] {
            let now = anchor + TimeDelta::seconds(secs);
            let next = next_age_change(anchor, now);
            assert!(next > now, "{secs}: {next} after {now}");
            let words = |at: DateTime<Utc>| (age_short(at - anchor), age_long(at - anchor));
            // Nothing changes before the boundary…
            assert_eq!(words(now), words(next - TimeDelta::milliseconds(1)), "{secs}");
            // …and it does change at it.
            assert_ne!(words(now), words(next), "{secs}");
        }
        assert_eq!(next_age_change(anchor, t(30)), t(60));
        assert_eq!(next_age_change(anchor, t(150)), t(180));
        assert_eq!(next_age_change(anchor, t(5_000)), t(7_200));
        assert_eq!(next_age_change(anchor, t(90_000)), t(172_800));
    }

    #[test]
    fn plans_the_earliest_text_change() {
        let now = t(0);
        let model = StripModel::build(
            &[watch("a"), watch("b"), watch("c")],
            &[
                // Updated 150 s ago: "2 min ago" until 180 s.
                snap(
                    "a",
                    Some(record(BuildStatus::Passed, t(-150))),
                    Freshness::Fresh,
                ),
                // Two hours old: changes at three hours.
                snap(
                    "b",
                    Some(record(BuildStatus::Failed, t(-7_300))),
                    Freshness::Fresh,
                ),
                // Never fetched: nothing ages.
                WatchSnapshot::pending(WatchId::new("c")),
            ],
            now,
        );
        assert_eq!(RepaintPlanner::next_text_change(&model, now), Some(t(30)));
        let planner = RepaintPlanner::default();
        assert_eq!(
            planner.next_repaint(&model, now),
            Some(Duration::from_secs(30) + CHANGE_SLACK)
        );
        // Just after it, the next one is a minute on ("3 min ago" until 240 s).
        assert_eq!(
            planner.next_repaint(&model, t(31)),
            Some(Duration::from_secs(59) + CHANGE_SLACK)
        );
    }

    #[test]
    fn nothing_to_plan_without_ages() {
        let now = t(0);
        let model = StripModel::build(&[watch("a")], &[], now);
        assert_eq!(RepaintPlanner::next_text_change(&model, now), None);
        assert_eq!(RepaintPlanner::default().next_repaint(&model, now), None);
        assert_eq!(
            RepaintPlanner::default().next_repaint(&StripModel::default(), now),
            None
        );
    }

    #[test]
    fn stale_entries_also_age_from_last_confirmation() {
        let now = t(0);
        let mut stale = snap(
            "a",
            Some(record(BuildStatus::Passed, t(-7_300))),
            Freshness::Stale {
                reason: StaleReason::Offline,
            },
        );
        stale.last_success = Some(t(-90));
        let model = StripModel::build(&[watch("a")], &[stale], now);
        assert_eq!(model.entries[0].ages, vec![t(-7_300), t(-90)]);
        assert_eq!(RepaintPlanner::next_text_change(&model, now), Some(t(30)));
    }

    #[test]
    fn animation_only_when_asked_allowed_and_visible() {
        let now = t(0);
        let building = StripModel::build(
            &[watch("a")],
            &[snap(
                "a",
                Some(record(BuildStatus::Running, t(-10))),
                Freshness::Fresh,
            )],
            now,
        );
        let still = RepaintPlanner::default();
        assert!(!still.animating(&building));
        assert_eq!(
            still.next_repaint(&building, now),
            Some(Duration::from_secs(50) + CHANGE_SLACK)
        );
        let on = Motion {
            enabled: true,
            reduced: false,
            visible: true,
        };
        assert!(on.animates());
        let planner = RepaintPlanner::new(on);
        assert!(planner.animating(&building));
        assert_eq!(planner.next_repaint(&building, now), Some(ANIMATION_STEP));
        for off in [
            Motion { enabled: false, ..on },
            Motion { reduced: true, ..on },
            Motion { visible: false, ..on },
        ] {
            assert!(!RepaintPlanner::new(off).animating(&building), "{off:?}");
        }
        // Nothing in progress: nothing animates even when allowed.
        assert_eq!(
            planner.next_repaint(&StripModel::build(&[watch("a")], &[], now), now),
            None
        );
        let empty = StripModel::default();
        assert!(!planner.animating(&empty));
    }

    #[test]
    fn animation_phase_steps_four_times_a_second() {
        assert_eq!(animation_phase(0.0), 0);
        assert_eq!(animation_phase(0.24), 0);
        assert_eq!(animation_phase(0.25), 1);
        assert_eq!(animation_phase(1.0), 4);
        assert_eq!(animation_phase(-3.0), 0);
    }

    #[test]
    fn confirming_unchanged_fresh_data_is_not_a_visible_change() {
        let first = vec![snap(
            "a",
            Some(record(BuildStatus::Passed, t(-60))),
            Freshness::Fresh,
        )];
        let mut confirmed = first.clone();
        confirmed[0].last_success = Some(t(0));
        if let Some(r) = &mut confirmed[0].record {
            r.observed_at = t(0);
        }
        assert!(!snapshots_differ_visibly(&first, &confirmed));

        // A new status is.
        let mut failed = first.clone();
        if let Some(r) = &mut failed[0].record {
            r.status = BuildStatus::Failed;
        }
        assert!(snapshots_differ_visibly(&first, &failed));
        // So is going stale, and a different number of watches.
        let mut stale = first.clone();
        stale[0].freshness = Freshness::Stale {
            reason: StaleReason::Offline,
        };
        assert!(snapshots_differ_visibly(&first, &stale));
        assert!(snapshots_differ_visibly(&first, &[]));
        // On stale data, last confirmation is shown, so it counts.
        let mut stale_later = stale.clone();
        stale_later[0].last_success = Some(t(5));
        assert!(snapshots_differ_visibly(&stale, &stale_later));
    }
}
