//! The strip's view-model: one [`StripEntry`] per pinned watch.
//!
//! [`StripModel::build`] is the single place that decides what an entry
//! *says*: its status word, icon shape, tone, the "cached / offline / no
//! runs / reconnect" note, the detail lines shown on hover or keyboard
//! focus, and the accessible name read by screen readers. The egui layer
//! only draws what it is given, so all of this is tested without a window.
//!
//! # Ordering
//!
//! Entries follow the *watches* (pinned order), never the snapshots, and
//! are matched to snapshots by [`WatchId`]. A rerun that turns an entry from
//! Failed to Running therefore changes that entry in place; nothing moves.
//!
//! # Status without colour
//!
//! Every state has its own [`IconKind`] shape *and* its own word, so tone
//! (colour) is only ever a third, redundant cue.

use std::collections::HashMap;

use buildrail_core::domain::{
    BuildRecord, BuildStatus, Freshness, ProviderKind, StaleReason, Tone, Watch, WatchId,
    WatchSnapshot, WatchTarget,
};
use chrono::{DateTime, Utc};

use super::text::{age_long, age_short, short_commit};

/// The shape drawn before an entry's text. Each one is visually distinct so
/// status never depends on colour alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconKind {
    /// A tick: passed.
    Check,
    /// A tick inside a ring: passed *and* the provider confirmed the
    /// deployment.
    CheckCircle,
    /// A diagonal cross: failed.
    Cross,
    /// A partial ring (animated in the UI): running.
    Spinner,
    /// A hollow ring with a dot: queued, waiting to start.
    Queued,
    /// A horizontal bar: cancelled.
    Dash,
    /// A double chevron: skipped.
    Skip,
    /// A question mark: the provider sent something we cannot map.
    Question,
    /// An empty ring: the target exists but has no runs yet.
    Empty,
    /// Three dots: nothing fetched yet.
    Pending,
    /// A warning triangle: data is missing because of an access, network
    /// or provider problem.
    Warning,
}

impl IconKind {
    /// The icon for a run status.
    pub fn for_status(status: BuildStatus) -> Self {
        match status {
            BuildStatus::Queued => Self::Queued,
            BuildStatus::Running => Self::Spinner,
            BuildStatus::Passed => Self::Check,
            BuildStatus::Deployed => Self::CheckCircle,
            BuildStatus::Failed => Self::Cross,
            BuildStatus::Cancelled => Self::Dash,
            BuildStatus::Skipped => Self::Skip,
            BuildStatus::Unavailable => Self::Question,
        }
    }

    /// `true` for the icons the UI animates.
    pub fn is_animated(self) -> bool {
        self == Self::Spinner
    }
}

/// Two-letter provider mark drawn before an entry. Deliberately plain
/// letters, not the providers' logo artwork.
pub fn provider_mark(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::GitHub => "GH",
        ProviderKind::Cloudflare => "CF",
    }
}

/// Everything the UI needs to draw and announce one pinned watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripEntry {
    /// The watch this entry shows.
    pub watch_id: WatchId,
    /// Which provider serves it.
    pub provider: ProviderKind,
    /// The user's short name.
    pub label: String,
    /// The word after the label: a run status ("Failed"), or why there is
    /// none ("No runs yet", "Reconnect needed", "Checking…").
    pub status_text: String,
    /// Colour family. Redundant with `icon` and `status_text`.
    pub tone: Tone,
    /// Icon shape.
    pub icon: IconKind,
    /// A short note about how current the entry is ("5 min ago · offline",
    /// "Reconnect needed"), shown muted after the status. `None` when the
    /// entry is fresh and has a run.
    pub note: Option<String>,
    /// `true` when what is shown is not confirmed by the latest refresh
    /// (cached at startup, or the last refresh failed).
    pub stale: bool,
    /// `true` when the user must reconnect the watch's connection.
    pub needs_reconnect: bool,
    /// Facts for the hover / keyboard-focus detail popup, one per line.
    pub details: Vec<String>,
    /// The sentence under the facts that says what activating the entry
    /// does ("Click or press Enter to open the latest run."), if anything.
    pub hint: Option<String>,
    /// Full sentence for screen readers, e.g.
    /// "Keevoro, GitHub, Failed, branch main, commit a7c412e, 2 minutes ago".
    pub accessible_name: String,
    /// The provider page to open on activation, when there is a run.
    pub url: Option<String>,
}

impl StripEntry {
    /// The visible entry text, `Keevoro · Failed`.
    pub fn text(&self) -> String {
        format!("{} · {}", self.label, self.status_text)
    }

    /// `true` for a failed run (counted in the overflow badge).
    pub fn is_failure(&self) -> bool {
        self.tone == Tone::Red
    }
}

/// The strip's content, in pinned order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StripModel {
    /// One entry per pinned watch.
    pub entries: Vec<StripEntry>,
}

impl StripModel {
    /// Builds the model. `watches` defines the order; `snapshots` are
    /// matched by watch ID (a watch without a snapshot shows as pending).
    pub fn build(watches: &[Watch], snapshots: &[WatchSnapshot], now: DateTime<Utc>) -> Self {
        let by_id: HashMap<&WatchId, &WatchSnapshot> =
            snapshots.iter().map(|s| (&s.watch_id, s)).collect();
        let entries = watches
            .iter()
            .map(|watch| match by_id.get(&watch.id) {
                Some(snapshot) => entry(watch, snapshot, now),
                None => entry(watch, &WatchSnapshot::pending(watch.id.clone()), now),
            })
            .collect();
        Self { entries }
    }

    /// `true` when nothing is pinned (first launch: show "Add build").
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of entries showing a failed run.
    pub fn failure_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_failure()).count()
    }

    /// The entry for a watch.
    pub fn get(&self, id: &WatchId) -> Option<&StripEntry> {
        self.entries.iter().find(|e| &e.watch_id == id)
    }
}

/// The short word for a stale reason, used in notes.
fn reason_word(reason: StaleReason) -> &'static str {
    match reason {
        StaleReason::Offline => "offline",
        StaleReason::AccessDenied => "reconnect needed",
        StaleReason::NotFound => "not found",
        StaleReason::RateLimited => "rate limited",
        StaleReason::ProviderError => "provider error",
    }
}

/// Capitalises the first letter (`offline` → `Offline`).
fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// One line describing what the watch points at.
fn target_line(target: &WatchTarget) -> String {
    match target {
        WatchTarget::GitHub {
            owner,
            repo,
            workflow,
            ..
        } => format!("GitHub · {owner}/{repo} · {workflow}"),
        WatchTarget::Cloudflare { worker, .. } => format!("Cloudflare · Worker {worker}"),
    }
}

fn entry(watch: &Watch, snapshot: &WatchSnapshot, now: DateTime<Utc>) -> StripEntry {
    let provider = watch.target.provider();
    let record = snapshot.record.as_ref();
    // When the data was last confirmed: the last successful refresh, or for
    // a record without one, when we saw it.
    let confirmed_at = snapshot
        .last_success
        .or_else(|| record.map(|r| r.observed_at));
    let since_confirmed = confirmed_at.map(|at| now - at);

    let stale_reason = match snapshot.freshness {
        Freshness::Stale { reason } => Some(reason),
        _ => None,
    };
    let needs_reconnect = stale_reason == Some(StaleReason::AccessDenied);

    let (status_text, tone, icon) = match (record, &snapshot.freshness) {
        (Some(record), _) => (
            record.status.label().to_owned(),
            record.status.tone(),
            IconKind::for_status(record.status),
        ),
        (None, Freshness::Pending) => ("Checking…".to_owned(), Tone::Neutral, IconKind::Pending),
        (None, Freshness::Fresh) => ("No runs yet".to_owned(), Tone::Neutral, IconKind::Empty),
        (None, Freshness::Stale { reason }) => (
            capitalised(reason_word(*reason)),
            Tone::Neutral,
            IconKind::Warning,
        ),
    };

    // Short note (strip) and its spoken form (accessible name).
    let (note, spoken_note) = match (record, &snapshot.freshness) {
        (Some(_), Freshness::Fresh) | (None, Freshness::Pending) => (None, None),
        (None, Freshness::Fresh) => (Some("No runs yet".to_owned()), None),
        (
            _,
            Freshness::Stale {
                reason: StaleReason::AccessDenied,
            },
        ) => (
            Some("Reconnect needed".to_owned()),
            Some("reconnect needed".to_owned()),
        ),
        (Some(_), Freshness::Pending) => match since_confirmed {
            Some(age) => (
                Some(format!("{} · cached", age_short(age))),
                Some(format!("cached, last updated {}", age_long(age))),
            ),
            None => (Some("cached".to_owned()), Some("cached".to_owned())),
        },
        (_, Freshness::Stale { reason }) => {
            let word = reason_word(*reason);
            match since_confirmed {
                Some(age) => (
                    Some(format!("{} · {word}", age_short(age))),
                    Some(format!("{word}, last updated {}", age_long(age))),
                ),
                None => (Some(capitalised(word)), Some(word.to_owned())),
            }
        }
    };
    let stale = record.is_some() && !matches!(snapshot.freshness, Freshness::Fresh);

    let branch = record
        .and_then(|r| r.branch.clone())
        .or_else(|| watch.target.branch().map(str::to_owned));
    let commit = record.and_then(|r| r.commit.as_deref()).map(short_commit);
    let run_age = record.map(|r| now - r.updated_at);
    let failing_job = record.and_then(failing_job_of);

    let mut details = vec![target_line(&watch.target)];
    if let Some(branch) = &branch {
        details.push(format!("Branch {branch}"));
    }
    if let Some(commit) = &commit {
        details.push(format!("Commit {commit}"));
    }
    if let (Some(record), Some(age)) = (record, run_age) {
        details.push(format!("{} {}", record.status.label(), age_short(age)));
    }
    if let Some(job) = &failing_job {
        details.push(format!("Failing job: {job}"));
    }
    if let Some(note) = &note {
        if record.is_some() || needs_reconnect {
            details.push(note.clone());
        }
    }
    let hint = if needs_reconnect {
        Some("Click or press Enter to reconnect in Settings.".to_owned())
    } else if record.is_some() {
        Some("Click or press Enter to open the latest run.".to_owned())
    } else {
        None
    };

    let mut spoken = vec![
        watch.label.clone(),
        provider.display_name().to_owned(),
        status_text.clone(),
    ];
    if let Some(branch) = &branch {
        spoken.push(format!("branch {branch}"));
    }
    if let Some(commit) = &commit {
        spoken.push(format!("commit {commit}"));
    }
    if let Some(age) = run_age {
        spoken.push(age_long(age));
    }
    if let Some(job) = &failing_job {
        spoken.push(format!("failing job {job}"));
    }
    if let Some(note) = spoken_note {
        if !spoken.iter().any(|part| part.eq_ignore_ascii_case(&note)) {
            spoken.push(note);
        }
    }

    StripEntry {
        watch_id: watch.id.clone(),
        provider,
        label: watch.label.clone(),
        status_text,
        tone,
        icon,
        note,
        stale,
        needs_reconnect,
        details,
        hint,
        accessible_name: spoken.join(", "),
        url: record.map(|r| r.url.clone()),
    }
}

/// The failing job, only for failed runs (a stale name from an earlier
/// attempt would mislead on a passing run).
fn failing_job_of(record: &BuildRecord) -> Option<String> {
    (record.status == BuildStatus::Failed)
        .then(|| record.failing_job.clone())
        .flatten()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use buildrail_core::domain::{ConnectionAlias, RunId};
    use chrono::{TimeDelta, TimeZone};

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).unwrap()
    }

    fn gh(id: &str, label: &str) -> Watch {
        Watch {
            id: WatchId::new(id),
            label: label.into(),
            connection: ConnectionAlias::new("work"),
            target: WatchTarget::GitHub {
                owner: "acme".into(),
                repo: id.into(),
                workflow: "ci.yml".into(),
                branch: Some("main".into()),
            },
        }
    }

    fn cf(id: &str, label: &str) -> Watch {
        Watch {
            id: WatchId::new(id),
            label: label.into(),
            connection: ConnectionAlias::new("cf"),
            target: WatchTarget::Cloudflare {
                account_id: "0123456789abcdef0123456789abcdef".into(),
                worker: id.into(),
                branch: None,
            },
        }
    }

    fn record(status: BuildStatus, minutes_ago: i64) -> BuildRecord {
        let at = now() - TimeDelta::minutes(minutes_ago);
        BuildRecord {
            run_id: RunId::new("1"),
            attempt: 1,
            status,
            url: "https://github.com/acme/keevoro/actions/runs/1".into(),
            branch: Some("main".into()),
            commit: Some("a7c412e9f0b1c2d3".into()),
            failing_job: Some("test (ubuntu)".into()),
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

    fn one(watch: Watch, snapshot: WatchSnapshot) -> StripEntry {
        StripModel::build(&[watch], &[snapshot], now())
            .entries
            .remove(0)
    }

    #[test]
    fn failed_entry_has_text_details_and_accessible_name() {
        let e = one(
            gh("keevoro", "Keevoro"),
            snap(
                "keevoro",
                Some(record(BuildStatus::Failed, 2)),
                Freshness::Fresh,
            ),
        );
        assert_eq!(e.text(), "Keevoro · Failed");
        assert_eq!(e.tone, Tone::Red);
        assert_eq!(e.icon, IconKind::Cross);
        assert_eq!(e.note, None);
        assert!(!e.stale);
        assert!(e.is_failure());
        assert_eq!(
            e.accessible_name,
            "Keevoro, GitHub, Failed, branch main, commit a7c412e, 2 minutes ago, failing job test (ubuntu)"
        );
        assert_eq!(
            e.details,
            vec![
                "GitHub · acme/keevoro · ci.yml",
                "Branch main",
                "Commit a7c412e",
                "Failed 2 min ago",
                "Failing job: test (ubuntu)",
            ]
        );
        assert_eq!(
            e.hint.as_deref(),
            Some("Click or press Enter to open the latest run.")
        );
        assert_eq!(
            e.url.as_deref(),
            Some("https://github.com/acme/keevoro/actions/runs/1")
        );
        assert_eq!(provider_mark(e.provider), "GH");
    }

    #[test]
    fn every_status_has_distinct_icon_and_word() {
        let statuses = [
            BuildStatus::Queued,
            BuildStatus::Running,
            BuildStatus::Passed,
            BuildStatus::Deployed,
            BuildStatus::Failed,
            BuildStatus::Cancelled,
            BuildStatus::Skipped,
            BuildStatus::Unavailable,
        ];
        let mut icons = std::collections::HashSet::new();
        let mut words = std::collections::HashSet::new();
        for status in statuses {
            let e = one(
                cf("w", "W"),
                snap("w", Some(record(status, 1)), Freshness::Fresh),
            );
            assert_eq!(e.tone, status.tone());
            icons.insert(e.icon);
            words.insert(e.status_text.clone());
            // Failing job only reported on failures.
            assert_eq!(
                e.details.iter().any(|d| d.starts_with("Failing job")),
                status == BuildStatus::Failed
            );
        }
        assert_eq!(icons.len(), statuses.len());
        assert_eq!(words.len(), statuses.len());
        assert!(IconKind::Spinner.is_animated());
        assert!(!IconKind::Check.is_animated());
        assert_eq!(provider_mark(ProviderKind::Cloudflare), "CF");
    }

    #[test]
    fn empty_history_differs_from_access_error() {
        let empty = one(cf("w", "Site"), snap("w", None, Freshness::Fresh));
        assert_eq!(empty.text(), "Site · No runs yet");
        assert_eq!(empty.icon, IconKind::Empty);
        assert_eq!(empty.note.as_deref(), Some("No runs yet"));
        assert!(!empty.needs_reconnect);
        assert_eq!(empty.accessible_name, "Site, Cloudflare, No runs yet");
        assert_eq!(empty.url, None);
        assert_eq!(empty.details, vec!["Cloudflare · Worker w"]);
        assert_eq!(empty.hint, None);

        let denied = one(
            cf("w", "Site"),
            snap(
                "w",
                None,
                Freshness::Stale {
                    reason: StaleReason::AccessDenied,
                },
            ),
        );
        assert_eq!(denied.text(), "Site · Reconnect needed");
        assert_eq!(denied.icon, IconKind::Warning);
        assert!(denied.needs_reconnect);
        assert_eq!(denied.note.as_deref(), Some("Reconnect needed"));
        assert_eq!(denied.accessible_name, "Site, Cloudflare, Reconnect needed");
        assert_eq!(
            denied.hint.as_deref(),
            Some("Click or press Enter to reconnect in Settings.")
        );
    }

    #[test]
    fn stale_record_shows_age_and_reason() {
        let mut s = snap(
            "k",
            Some(record(BuildStatus::Passed, 5)),
            Freshness::Stale {
                reason: StaleReason::Offline,
            },
        );
        s.last_success = Some(now() - TimeDelta::minutes(5));
        let e = one(gh("k", "Keevoro"), s);
        assert_eq!(e.text(), "Keevoro · Passed");
        assert!(e.stale);
        assert_eq!(e.note.as_deref(), Some("5 min ago · offline"));
        assert!(e
            .accessible_name
            .ends_with("offline, last updated 5 minutes ago"));
        assert!(e.details.contains(&"5 min ago · offline".to_owned()));

        // Stale with a record but an access problem: reconnect note.
        let mut s = snap(
            "k",
            Some(record(BuildStatus::Passed, 5)),
            Freshness::Stale {
                reason: StaleReason::AccessDenied,
            },
        );
        s.last_success = None;
        let e = one(gh("k", "Keevoro"), s);
        assert_eq!(e.note.as_deref(), Some("Reconnect needed"));
        assert!(e.needs_reconnect);
        assert!(e.accessible_name.ends_with("reconnect needed"));
    }

    #[test]
    fn stale_without_record_names_reason() {
        for (reason, word) in [
            (StaleReason::Offline, "Offline"),
            (StaleReason::NotFound, "Not found"),
            (StaleReason::RateLimited, "Rate limited"),
            (StaleReason::ProviderError, "Provider error"),
        ] {
            let e = one(gh("k", "K"), snap("k", None, Freshness::Stale { reason }));
            assert_eq!(e.status_text, word);
            assert_eq!(e.note.as_deref(), Some(word));
            assert_eq!(e.icon, IconKind::Warning);
            assert!(!e.stale, "nothing shown can be out of date");
            // Spoken note is not repeated when it equals the status.
            assert_eq!(e.accessible_name, format!("K, GitHub, {word}, branch main"));
        }
    }

    #[test]
    fn cached_record_at_startup() {
        let mut s = snap(
            "k",
            Some(record(BuildStatus::Passed, 90)),
            Freshness::Pending,
        );
        s.last_success = Some(now() - TimeDelta::minutes(90));
        let e = one(gh("k", "K"), s.clone());
        assert!(e.stale);
        assert_eq!(e.note.as_deref(), Some("1 h ago · cached"));
        assert!(e
            .accessible_name
            .ends_with("cached, last updated 1 hour ago"));

        // A cache entry without last_success falls back to observed_at.
        s.last_success = None;
        let e = one(gh("k", "K"), s.clone());
        assert_eq!(e.note.as_deref(), Some("1 h ago · cached"));
    }

    #[test]
    fn pending_without_record() {
        let e = one(gh("k", "K"), WatchSnapshot::pending(WatchId::new("k")));
        assert_eq!(e.text(), "K · Checking…");
        assert_eq!(e.icon, IconKind::Pending);
        assert_eq!(e.note, None);
        // Branch filter from the target is still useful detail.
        assert!(e.details.contains(&"Branch main".to_owned()));
    }

    #[test]
    fn order_follows_watches_not_snapshots() {
        let watches = [gh("a", "A"), cf("b", "B"), gh("c", "C")];
        let snapshots = [
            snap("c", Some(record(BuildStatus::Failed, 1)), Freshness::Fresh),
            snap("a", Some(record(BuildStatus::Passed, 1)), Freshness::Fresh),
        ];
        let model = StripModel::build(&watches, &snapshots, now());
        let labels: Vec<_> = model.entries.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["A", "B", "C"]);
        assert_eq!(model.entries[1].status_text, "Checking…");
        assert_eq!(model.failure_count(), 1);
        assert!(!model.is_empty());
        assert_eq!(model.get(&WatchId::new("c")).unwrap().label, "C");
        assert!(model.get(&WatchId::new("zz")).is_none());
        assert!(StripModel::default().is_empty());
    }

    #[test]
    fn helpers() {
        assert_eq!(capitalised(""), "");
        assert_eq!(capitalised("offline"), "Offline");
    }
}
