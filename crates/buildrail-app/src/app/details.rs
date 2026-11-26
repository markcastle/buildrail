//! The details use-case's state, decided without a runtime: which build's
//! details panel is open, what was fetched for each build this session, and
//! when to fetch again.
//!
//! [`DetailsBook`] is pure: the controller feeds it what the strip shows
//! for the open build ([`DetailsBook::plan`]) and the answers of the
//! fetches it started ([`DetailsBook::accept`]); the book says what to
//! fetch. The rules:
//!
//! * details are fetched only for the open panel, never in the
//!   background, so polling costs exactly what it did;
//! * each build keeps its last details for the session, keyed by run,
//!   attempt and status: a finished run is never fetched twice, and
//!   reopening a panel shows what was fetched before at once;
//! * a running (or queued) run is fetched again whenever what the strip
//!   shows for it changes (the refresh engine saw progress);
//! * "Refresh now" and "Try again" force a fetch;
//! * every fetch has a request number and only the latest answer for a
//!   build is kept, so a slow answer never overwrites a newer one, and an
//!   answer for a build whose panel was closed or switched away from
//!   changes nothing on screen.

use std::collections::HashMap;

use buildrail_core::details::{RunDetails, RunRef};
use buildrail_core::domain::{
    BuildRecord, BuildStatus, ConnectionAlias, ProviderKind, RunId, WatchId,
};
use buildrail_core::ports::ProviderError;

/// Why a build's details could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailsError {
    /// The provider call failed.
    Provider(ProviderKind, ProviderError),
    /// No token is stored for the build's connection on this device.
    NoCredential(ConnectionAlias),
    /// The operating system's credential store failed.
    CredentialStore,
    /// This build of BuildRail has no details source for the provider.
    NoSource(ProviderKind),
}

/// Where loading a build's details stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DetailsLoad {
    /// Nothing asked for (no run to ask about).
    #[default]
    Idle,
    /// A fetch is on its way. Earlier details, if any, are still shown.
    Loading,
    /// The latest fetch answered.
    Loaded,
    /// The latest fetch failed. Earlier details, if any, are still shown.
    Failed(DetailsError),
}

/// A fetch the controller should start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailsFetch {
    /// Identifies this fetch's answer.
    pub request: u64,
    /// The build it is for.
    pub watch: WatchId,
    /// The run to ask about.
    pub run: RunRef,
}

/// What a fetch brought back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailsAnswer {
    /// The run's details.
    pub details: Result<RunDetails, DetailsError>,
    /// The watch's recent runs, when they could be read.
    pub recent: Option<Vec<BuildRecord>>,
}

/// What the book holds for one build, for the panel to show.
#[derive(Debug, Clone, Copy)]
pub struct DetailsView<'a> {
    /// The latest details fetched for the run on the strip.
    pub details: Option<&'a RunDetails>,
    /// The latest recent runs fetched.
    pub recent: Option<&'a [BuildRecord]>,
    /// Where loading stands.
    pub load: &'a DetailsLoad,
}

/// Run, attempt and status: when any of them changes, the run on the strip
/// is a different thing to describe.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RunKey {
    run_id: RunId,
    attempt: u32,
    status: BuildStatus,
}

impl RunKey {
    fn of(record: &BuildRecord) -> Self {
        Self {
            run_id: record.run_id.clone(),
            attempt: record.attempt,
            status: record.status,
        }
    }

    fn same_attempt(&self, other: &Self) -> bool {
        self.run_id == other.run_id && self.attempt == other.attempt
    }
}

/// A record with the time we last saw it taken out: confirming unchanged
/// data is not a change.
fn seen(record: &BuildRecord) -> BuildRecord {
    let mut seen = record.clone();
    seen.observed_at = seen.updated_at;
    seen
}

#[derive(Debug, Clone)]
struct Entry {
    key: RunKey,
    seen: BuildRecord,
    details: Option<RunDetails>,
    recent: Option<Vec<BuildRecord>>,
    load: DetailsLoad,
    request: Option<u64>,
}

const IDLE: DetailsLoad = DetailsLoad::Idle;

/// See the module docs.
#[derive(Debug, Clone, Default)]
pub struct DetailsBook {
    open: Option<WatchId>,
    entries: HashMap<WatchId, Entry>,
    next_request: u64,
}

impl DetailsBook {
    /// A book with no panel open and nothing fetched.
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the panel for `watch` (closing any other).
    pub fn open(&mut self, watch: WatchId) {
        self.open = Some(watch);
    }

    /// Closes the panel. What was fetched is kept for the session.
    pub fn close(&mut self) {
        self.open = None;
    }

    /// The build whose panel is open.
    pub fn open_watch(&self) -> Option<&WatchId> {
        self.open.as_ref()
    }

    /// Forgets builds for which `keep` is false (removed from the
    /// profile), closing the panel if it showed one.
    pub fn retain(&mut self, keep: impl Fn(&WatchId) -> bool) {
        self.entries.retain(|watch, _| keep(watch));
        if self.open.as_ref().is_some_and(|watch| !keep(watch)) {
            self.open = None;
        }
    }

    fn request(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }

    /// What to fetch now for the open panel, given the run the strip shows
    /// for that build (`None`: no run to ask about, or none that can be
    /// asked about, such as while it needs reconnecting). `force` is
    /// "Refresh now" or "Try again".
    pub fn plan(&mut self, record: Option<&BuildRecord>, force: bool) -> Option<DetailsFetch> {
        let watch = self.open.clone()?;
        let record = record?;
        let key = RunKey::of(record);
        let now_seen = seen(record);
        let fetch = match self.entries.get_mut(&watch) {
            None => true,
            Some(entry) if entry.key != key => {
                if !entry.key.same_attempt(&key) {
                    // Another run: its details would mislead.
                    entry.details = None;
                }
                true
            }
            Some(entry) => force || (record.status.is_active() && entry.seen != now_seen),
        };
        if !fetch {
            return None;
        }
        let request = self.request();
        let entry = self.entries.entry(watch.clone()).or_insert_with(|| Entry {
            key: key.clone(),
            seen: now_seen.clone(),
            details: None,
            recent: None,
            load: DetailsLoad::Loading,
            request: None,
        });
        entry.key = key;
        entry.seen = now_seen;
        entry.load = DetailsLoad::Loading;
        entry.request = Some(request);
        Some(DetailsFetch {
            request,
            watch,
            run: RunRef::from(record),
        })
    }

    /// Takes the answer to fetch `request` for `watch`. Answers that are
    /// not the latest for that build are ignored (returns `false`).
    pub fn accept(&mut self, request: u64, watch: &WatchId, answer: DetailsAnswer) -> bool {
        let Some(entry) = self.entries.get_mut(watch) else {
            return false;
        };
        if entry.request != Some(request) {
            return false;
        }
        entry.request = None;
        if let Some(recent) = answer.recent {
            entry.recent = Some(recent);
        }
        match answer.details {
            Ok(details) => {
                entry.details = Some(details);
                entry.load = DetailsLoad::Loaded;
            }
            Err(error) => entry.load = DetailsLoad::Failed(error),
        }
        true
    }

    /// What is known about `watch`.
    pub fn view(&self, watch: &WatchId) -> DetailsView<'_> {
        match self.entries.get(watch) {
            Some(entry) => DetailsView {
                details: entry.details.as_ref(),
                recent: entry.recent.as_deref(),
                load: &entry.load,
            },
            None => DetailsView {
                details: None,
                recent: None,
                load: &IDLE,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::{TimeDelta, TimeZone, Utc};

    use super::*;

    fn record(run: &str, attempt: u32, status: BuildStatus, minute: i64) -> BuildRecord {
        let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap() + TimeDelta::minutes(minute);
        BuildRecord {
            run_id: RunId::new(run),
            attempt,
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

    fn answer(run: &str) -> DetailsAnswer {
        DetailsAnswer {
            details: Ok(RunDetails::new(RunRef {
                run_id: RunId::new(run),
                attempt: 1,
            })),
            recent: Some(vec![record(run, 1, BuildStatus::Passed, 0)]),
        }
    }

    fn w(id: &str) -> WatchId {
        WatchId::new(id)
    }

    #[test]
    fn nothing_is_fetched_without_an_open_panel_or_a_run() {
        let mut book = DetailsBook::new();
        let passed = record("1", 1, BuildStatus::Passed, 0);
        assert_eq!(book.plan(Some(&passed), true), None, "no panel open");
        book.open(w("a"));
        assert_eq!(book.plan(None, true), None, "no run");
        assert_eq!(book.view(&w("a")).load, &DetailsLoad::Idle);
        assert!(book.view(&w("a")).details.is_none());
    }

    #[test]
    fn a_finished_run_is_fetched_once_per_session() {
        let mut book = DetailsBook::new();
        let passed = record("1", 1, BuildStatus::Passed, 0);
        book.open(w("a"));
        let fetch = book.plan(Some(&passed), false).unwrap();
        assert_eq!(fetch.watch, w("a"));
        assert_eq!(fetch.run.run_id.as_str(), "1");
        assert_eq!(book.view(&w("a")).load, &DetailsLoad::Loading);
        assert_eq!(book.plan(Some(&passed), false), None, "already on its way");
        assert!(book.accept(fetch.request, &w("a"), answer("1")));
        assert_eq!(book.view(&w("a")).load, &DetailsLoad::Loaded);
        assert_eq!(book.view(&w("a")).recent.unwrap().len(), 1);
        // Closed and opened again, even with a fresher observation: no fetch.
        book.close();
        assert_eq!(book.open_watch(), None);
        book.open(w("a"));
        let mut confirmed = passed.clone();
        confirmed.observed_at += TimeDelta::minutes(5);
        assert_eq!(book.plan(Some(&confirmed), false), None);
        // Refresh now forces one.
        assert!(book.plan(Some(&passed), true).is_some());
    }

    #[test]
    fn a_running_run_is_fetched_again_when_the_strip_changes() {
        let mut book = DetailsBook::new();
        book.open(w("a"));
        let running = record("1", 1, BuildStatus::Running, 0);
        let first = book.plan(Some(&running), false).unwrap();
        book.accept(first.request, &w("a"), answer("1"));
        // Confirming the same state is not a change…
        let mut confirmed = running.clone();
        confirmed.observed_at += TimeDelta::seconds(30);
        assert_eq!(book.plan(Some(&confirmed), false), None);
        // …progress is.
        let mut progressed = running.clone();
        progressed.updated_at += TimeDelta::seconds(30);
        let second = book.plan(Some(&progressed), false).unwrap();
        assert!(second.request > first.request);
        assert!(
            book.view(&w("a")).details.is_some(),
            "earlier details stay while reloading"
        );
        // It finished: same attempt, new status. Details kept until the
        // answer comes, then replaced.
        let done = record("1", 1, BuildStatus::Passed, 0);
        let third = book.plan(Some(&done), false).unwrap();
        assert!(book.view(&w("a")).details.is_some());
        assert!(
            !book.accept(second.request, &w("a"), answer("old")),
            "superseded"
        );
        assert!(book.accept(third.request, &w("a"), answer("1")));
        // A new run drops the old run's details at once.
        let next = record("2", 1, BuildStatus::Queued, 5);
        book.plan(Some(&next), false).unwrap();
        assert!(book.view(&w("a")).details.is_none());
        assert!(book.view(&w("a")).recent.is_some(), "history stays");
    }

    #[test]
    fn failures_keep_what_was_known_and_switching_ignores_the_old_answer() {
        let mut book = DetailsBook::new();
        book.open(w("a"));
        let a = record("1", 1, BuildStatus::Failed, 0);
        let first = book.plan(Some(&a), false).unwrap();
        book.accept(first.request, &w("a"), answer("1"));
        let retry = book.plan(Some(&a), true).unwrap();
        let failed = DetailsAnswer {
            details: Err(DetailsError::NoCredential(ConnectionAlias::new("work"))),
            recent: None,
        };
        assert!(book.accept(retry.request, &w("a"), failed));
        let view = book.view(&w("a"));
        assert!(matches!(
            view.load,
            DetailsLoad::Failed(DetailsError::NoCredential(_))
        ));
        assert!(view.details.is_some() && view.recent.is_some());
        // A failure does not refetch by itself.
        assert_eq!(book.plan(Some(&a), false), None);

        // Switch to b while a is loading again; a's late answer only
        // updates a.
        let again = book.plan(Some(&a), true).unwrap();
        book.open(w("b"));
        let b = record("9", 1, BuildStatus::Passed, 0);
        let fetch_b = book.plan(Some(&b), false).unwrap();
        assert!(book.accept(again.request, &w("a"), answer("1")));
        assert_eq!(book.view(&w("b")).load, &DetailsLoad::Loading);
        assert!(book.view(&w("b")).details.is_none());
        assert!(!book.accept(fetch_b.request, &w("zzz"), answer("9")));
        assert!(!book.accept(999, &w("b"), answer("9")));
        assert!(book.accept(fetch_b.request, &w("b"), answer("9")));
    }

    #[test]
    fn removed_builds_are_forgotten() {
        let mut book = DetailsBook::new();
        book.open(w("a"));
        let fetch = book
            .plan(Some(&record("1", 1, BuildStatus::Passed, 0)), false)
            .unwrap();
        book.accept(fetch.request, &w("a"), answer("1"));
        book.retain(|watch| watch != &w("a"));
        assert_eq!(book.open_watch(), None);
        assert!(book.view(&w("a")).details.is_none());
        book.open(w("b"));
        book.retain(|_| true);
        assert_eq!(book.open_watch(), Some(&w("b")));
    }
}
