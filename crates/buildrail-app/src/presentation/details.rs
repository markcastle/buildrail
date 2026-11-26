//! The details panel's view-model: everything the panel says about one
//! pinned build, decided without a window.
//!
//! [`DetailsModel::build`] takes the watch, its snapshot (what the strip
//! shows), whatever details and recent runs were fetched for it, and where
//! loading stands, and returns the panel's sections as plain strings:
//!
//! 1. a header (status glyph, label, status word) and one line of facts
//!    (`GitHub · acme/keevoro · ci.yml`);
//! 2. a notice when there is something to explain first (reconnect, no
//!    runs yet, not checked yet, cannot be reached) and the freshness line
//!    when the data is not current;
//! 3. "This run": branch, commit, trigger, run number, started, duration;
//! 4. "Jobs" (GitHub): failed jobs first with their failing step, capped
//!    at [`MAX_JOBS`] with "and N more";
//! 5. "Recent runs": the last [`MAX_RECENT`] runs as status marks;
//! 6. the actions: one primary (Open run, or Reconnect, or Refresh now
//!    when there is no run) and quiet ones.
//!
//! The snapshot's facts are shown at once; fetched details refine them
//! when they land. Times are said the way people say them ("12 min ago",
//! "took 4 min 12 s", "running for 3 min"); the exact local time of the
//! start is the started row's description (hover, and screen readers).
//! [`DetailsModel::next_text_change`] says when the words next change on
//! their own, so an open panel schedules exactly one repaint for it.

use buildrail_core::details::{JobSummary, RunDetails, Trigger};
use buildrail_core::domain::{
    BuildRecord, BuildStatus, Freshness, ProviderKind, StaleReason, Tone, Watch, WatchId,
    WatchSnapshot, WatchTarget,
};
use buildrail_core::selection::select_latest;
use chrono::{DateTime, FixedOffset, TimeDelta, Utc};

use super::repaint::next_age_change;
use super::strip::{capitalised, reason_word, target_line, IconKind};
use super::text::{age_long, age_short, short_commit};
use crate::app::{DetailsError, DetailsLoad};

/// At most this many jobs are listed; the rest are "and N more".
pub const MAX_JOBS: usize = 12;

/// At most this many recent runs are shown.
pub const MAX_RECENT: usize = 8;

/// A commit message's first line is cut to this many characters.
pub const MAX_MESSAGE: usize = 72;

/// Everything [`DetailsModel::build`] needs.
#[derive(Debug, Clone, Copy)]
pub struct DetailsInput<'a> {
    /// The pinned build.
    pub watch: &'a Watch,
    /// What the strip shows for it.
    pub snapshot: &'a WatchSnapshot,
    /// The details fetched for the run on the strip, if any yet.
    pub details: Option<&'a RunDetails>,
    /// The recent runs fetched, if any yet.
    pub recent: Option<&'a [BuildRecord]>,
    /// Where loading stands.
    pub load: &'a DetailsLoad,
    /// The time to measure ages from.
    pub now: DateTime<Utc>,
    /// The local time zone's offset, for exact times.
    pub offset: FixedOffset,
}

/// Something the panel's buttons and rows can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelAction {
    /// Open this page in the browser (the run, a job, a recent run).
    Open(String),
    /// Open Settings to reconnect the build's connection.
    Reconnect,
    /// Check the build now and fetch its details again.
    Refresh,
    /// Fetch the details again after a failure.
    Retry,
    /// Put this link on the clipboard.
    CopyLink(String),
    /// Open Settings › Builds on this build.
    EditInSettings,
    /// Close the panel.
    Close,
}

/// A button in the actions row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelButton {
    /// The words on it, verb first.
    pub label: String,
    /// What it does.
    pub action: PanelAction,
    /// `false` dims it (a refresh already on its way).
    pub enabled: bool,
}

impl PanelButton {
    fn new(label: &str, action: PanelAction) -> Self {
        Self {
            label: label.to_owned(),
            action,
            enabled: true,
        }
    }
}

/// Something to explain before the facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// A short heading ("No runs yet."), if it has one.
    pub heading: Option<String>,
    /// The sentence that explains it.
    pub text: String,
    /// `true` when something is wrong (drawn in the error colour).
    pub problem: bool,
}

/// One row of "This run".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    /// The row's name ("Branch").
    pub label: &'static str,
    /// What it says ("main").
    pub value: String,
    /// More, on hover and for screen readers (the exact start time).
    pub description: Option<String>,
}

impl Fact {
    fn new(label: &'static str, value: impl Into<String>) -> Self {
        Self {
            label,
            value: value.into(),
            description: None,
        }
    }

    /// What a screen reader says for the row.
    pub fn accessible_name(&self) -> String {
        match &self.description {
            Some(more) => format!("{}: {}, {more}", self.label, self.value),
            None => format!("{}: {}", self.label, self.value),
        }
    }
}

/// The loading line under the facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadLine {
    /// "Loading details…" or "Details could not be loaded: …."
    pub text: String,
    /// `true` for a failure (shown with **Try again**).
    pub failed: bool,
}

/// One job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    /// Its name.
    pub name: String,
    /// Its status word.
    pub status_text: String,
    /// Colour family.
    pub tone: Tone,
    /// Glyph.
    pub icon: IconKind,
    /// "2 min 55 s", "running for 1 min", "waiting".
    pub duration: Option<String>,
    /// "Failed at “cargo test”." for a failed job whose step is known.
    pub failing_step: Option<String>,
    /// The job's page.
    pub url: Option<String>,
    /// What a screen reader says.
    pub accessible_name: String,
}

/// The link after a capped job list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoreLink {
    /// "and 5 more".
    pub text: String,
    /// Where it goes (the run).
    pub url: String,
    /// What a screen reader says.
    pub accessible_name: String,
}

/// The "Jobs" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobsSection {
    /// "Jobs".
    pub heading: &'static str,
    /// Failed first, then in the provider's order, capped.
    pub rows: Vec<JobRow>,
    /// "and N more", when capped.
    pub more: Option<MoreLink>,
    /// Said instead of rows when there are none ("No jobs have started
    /// yet.").
    pub empty: Option<String>,
}

/// One recent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentRun {
    /// Glyph.
    pub icon: IconKind,
    /// Colour family.
    pub tone: Tone,
    /// Status word.
    pub status_text: String,
    /// "12 min ago".
    pub when: String,
    /// Its branch, when the watch follows every branch.
    pub branch: Option<String>,
    /// Its page.
    pub url: String,
    /// `true` for the run the strip shows.
    pub current: bool,
    /// What a screen reader says.
    pub accessible_name: String,
}

/// The "Recent runs" section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentSection {
    /// "Recent runs" (GitHub) or "Recent builds" (Cloudflare).
    pub heading: &'static str,
    /// Newest first.
    pub runs: Vec<RecentRun>,
    /// "2 of the last 6 failed."
    pub summary: Option<String>,
}

/// An instant some words in the panel are measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Anchor {
    /// An age, worded by [`age_short`] ("5 min ago").
    Age(DateTime<Utc>),
    /// Time running since, worded by [`running_words`] (whole minutes).
    Elapsed(DateTime<Utc>),
}

/// The details panel for one build. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailsModel {
    /// The build.
    pub watch_id: WatchId,
    /// Which provider serves it.
    pub provider: ProviderKind,
    /// The user's short name.
    pub label: String,
    /// The status word, as on the strip.
    pub status_text: String,
    /// Colour family.
    pub tone: Tone,
    /// Glyph.
    pub icon: IconKind,
    /// `GitHub · acme/keevoro · ci.yml`.
    pub facts: String,
    /// The panel's name for screen readers and the window title.
    pub title: String,
    /// What to explain first.
    pub notice: Option<Notice>,
    /// "Last checked 5 min ago · offline.", when not current.
    pub freshness: Option<String>,
    /// "This run".
    pub run: Vec<Fact>,
    /// Loading or failed.
    pub load_line: Option<LoadLine>,
    /// "Jobs", when the provider has them.
    pub jobs: Option<JobsSection>,
    /// "Recent runs", once fetched.
    pub recent: Option<RecentSection>,
    /// The one primary button.
    pub primary: PanelButton,
    /// The quiet ones, in order.
    pub secondary: Vec<PanelButton>,
    anchors: Vec<Anchor>,
}

impl DetailsModel {
    /// Builds the panel. See the module docs.
    pub fn build(input: DetailsInput<'_>) -> Self {
        let DetailsInput {
            watch,
            snapshot,
            details,
            recent,
            load,
            now,
            offset,
        } = input;
        let provider = watch.target.provider();
        let record = snapshot.record.as_ref();
        // Details describe the run on the strip only when they are for it.
        let details = details.filter(|d| {
            record.is_some_and(|r| r.run_id == d.run.run_id && r.attempt == d.run.attempt)
        });
        let stale_reason = match snapshot.freshness {
            Freshness::Stale { reason } => Some(reason),
            _ => None,
        };
        let needs_reconnect = stale_reason == Some(StaleReason::AccessDenied);
        let mut anchors = Vec::new();

        let (status_text, tone, icon) = match (record, &snapshot.freshness) {
            (Some(r), _) => (
                r.status.label().to_owned(),
                r.status.tone(),
                IconKind::for_status(r.status),
            ),
            (None, Freshness::Pending) => {
                ("Checking…".to_owned(), Tone::Neutral, IconKind::Pending)
            }
            (None, Freshness::Fresh) => ("No runs yet".to_owned(), Tone::Neutral, IconKind::Empty),
            (None, Freshness::Stale { reason }) => (
                capitalised(reason_word(*reason)),
                Tone::Neutral,
                IconKind::Warning,
            ),
        };

        let notice = notice(watch, record, &snapshot.freshness);
        let freshness = record.and_then(|_| {
            let confirmed = snapshot
                .last_success
                .or_else(|| record.map(|r| r.observed_at))?;
            let age = age_short(now - confirmed);
            let line = match snapshot.freshness {
                Freshness::Fresh => return None,
                Freshness::Pending => format!("Last checked {age} · checking again now."),
                Freshness::Stale { reason } => {
                    format!("Last checked {age} · {}.", reason_word(reason))
                }
            };
            anchors.push(Anchor::Age(confirmed));
            Some(line)
        });

        let run = record.map_or_else(Vec::new, |r| {
            run_facts(watch, r, details, now, offset, &mut anchors)
        });

        let fetchable = record.is_some() && !needs_reconnect;
        let load_line = match load {
            DetailsLoad::Loading if fetchable && details.is_none() => Some(LoadLine {
                text: "Loading details…".to_owned(),
                failed: false,
            }),
            DetailsLoad::Failed(error) if fetchable => Some(LoadLine {
                text: format!("Details could not be loaded: {}.", failure_words(error)),
                failed: true,
            }),
            _ => None,
        };

        let jobs = details
            .and_then(|d| jobs_section(d, record.map(|r| r.url.as_str()), now, &mut anchors));
        let recent = recent
            .filter(|_| record.is_some())
            .and_then(|runs| recent_section(watch, runs, record, now, &mut anchors));

        let refreshing = matches!(load, DetailsLoad::Loading) && fetchable;
        let refresh = PanelButton {
            label: if refreshing {
                "Refreshing…".to_owned()
            } else {
                "Refresh now".to_owned()
            },
            action: PanelAction::Refresh,
            enabled: !refreshing,
        };
        let edit = PanelButton::new("Edit in Settings", PanelAction::EditInSettings);
        let url = record.map(|r| r.url.clone());
        let (primary, secondary) = match (needs_reconnect, url) {
            (true, url) => {
                let mut quiet = Vec::new();
                if let Some(url) = url {
                    quiet.push(PanelButton::new("Open run", PanelAction::Open(url)));
                }
                quiet.push(edit);
                (PanelButton::new("Reconnect", PanelAction::Reconnect), quiet)
            }
            (false, Some(url)) => (
                PanelButton::new("Open run", PanelAction::Open(url.clone())),
                vec![
                    refresh,
                    PanelButton::new("Copy link", PanelAction::CopyLink(url)),
                    edit,
                ],
            ),
            (false, None) => (refresh, vec![edit]),
        };

        Self {
            watch_id: watch.id.clone(),
            provider,
            title: format!("{} details", watch.label),
            label: watch.label.clone(),
            status_text,
            tone,
            icon,
            facts: target_line(&watch.target),
            notice,
            freshness,
            run,
            load_line,
            jobs,
            recent,
            primary,
            secondary,
            anchors,
        }
    }

    /// What a screen reader says for the header.
    pub fn accessible_heading(&self) -> String {
        format!("{}, {}, {}", self.label, self.status_text, self.facts)
    }

    /// The next instant at which the panel's words change on their own (an
    /// age or a running time crossing into new words), if any.
    pub fn next_text_change(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.anchors
            .iter()
            .map(|anchor| match *anchor {
                Anchor::Age(at) => next_age_change(at, now),
                Anchor::Elapsed(since) => next_minute(since, now),
            })
            .min()
    }
}

/// The first instant after `now` at which the whole minutes since `since`
/// change.
fn next_minute(since: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
    let secs = (now - since).num_seconds().max(0);
    since + TimeDelta::seconds((secs / 60 + 1) * 60)
}

/// "branch main", for sentences.
fn on_branch(watch: &Watch) -> String {
    watch
        .target
        .branch()
        .map(|b| format!(" on branch {b}"))
        .unwrap_or_default()
}

fn notice(watch: &Watch, record: Option<&BuildRecord>, freshness: &Freshness) -> Option<Notice> {
    let provider = watch.target.provider().display_name();
    let problem = |text: String| Notice {
        heading: None,
        text,
        problem: true,
    };
    match (record, freshness) {
        (
            _,
            Freshness::Stale {
                reason: StaleReason::AccessDenied,
            },
        ) => Some(problem(format!(
            "{provider} no longer accepts the token for this build, so BuildRail cannot check it. Reconnect to give it a new one."
        ))),
        (Some(_), _) => None,
        (None, Freshness::Pending) => Some(Notice {
            heading: None,
            text: "BuildRail is checking this build for the first time.".to_owned(),
            problem: false,
        }),
        (None, Freshness::Fresh) => {
            let text = match &watch.target {
                WatchTarget::GitHub { workflow, .. } => format!(
                    "{workflow} has not run{} yet. Its first run will show here as soon as it starts.",
                    on_branch(watch)
                ),
                WatchTarget::Cloudflare { worker, .. } => format!(
                    "{worker} has not been built{} yet. Its first build will show here as soon as it starts.",
                    on_branch(watch)
                ),
            };
            Some(Notice {
                heading: Some("No runs yet.".to_owned()),
                text,
                problem: false,
            })
        }
        (None, Freshness::Stale { reason }) => Some(problem(match reason {
            StaleReason::NotFound => format!(
                "{provider} cannot find this build, or the token cannot see it. Check it in Settings."
            ),
            StaleReason::Offline => format!(
                "BuildRail cannot reach {provider} right now. It tries again on its own."
            ),
            StaleReason::RateLimited => format!(
                "{provider} is limiting requests for now. BuildRail tries again shortly."
            ),
            StaleReason::ProviderError | StaleReason::AccessDenied => format!(
                "{provider} had a problem answering. BuildRail tries again shortly."
            ),
        })),
    }
}

fn run_facts(
    watch: &Watch,
    record: &BuildRecord,
    details: Option<&RunDetails>,
    now: DateTime<Utc>,
    offset: FixedOffset,
    anchors: &mut Vec<Anchor>,
) -> Vec<Fact> {
    let mut facts = Vec::new();
    if let Some(branch) = record
        .branch
        .clone()
        .or_else(|| watch.target.branch().map(str::to_owned))
    {
        facts.push(Fact::new("Branch", branch));
    }
    let message = details.and_then(|d| {
        d.commit_message
            .as_deref()
            .or(d.title.as_deref())
            .map(first_line)
            .filter(|line| !line.is_empty())
    });
    match (record.commit.as_deref().map(short_commit), message) {
        (Some(sha), Some(message)) => facts.push(Fact::new("Commit", format!("{sha} · {message}"))),
        (Some(sha), None) => facts.push(Fact::new("Commit", sha)),
        (None, Some(message)) => facts.push(Fact::new("Commit", message)),
        (None, None) => {}
    }
    if let Some(trigger) = details.and_then(trigger_words) {
        facts.push(Fact::new("Trigger", capitalised(&trigger)));
    }
    let number = details.and_then(|d| d.run_number);
    match (number, record.attempt) {
        (Some(n), 1) => facts.push(Fact::new("Run", format!("#{n}"))),
        (Some(n), attempt) => facts.push(Fact::new("Run", format!("#{n}, attempt {attempt}"))),
        (None, attempt) if attempt > 1 => {
            facts.push(Fact::new("Run", format!("Attempt {attempt}")));
        }
        (None, _) => {}
    }

    // Without details, the snapshot's creation time stands in for the
    // start (it includes time in the queue).
    let started = match details {
        Some(d) => d.started_at,
        None => (record.status != BuildStatus::Queued).then_some(record.created_at),
    };
    let finished = match details {
        Some(d) => d.finished_at,
        None => (!record.status.is_active()).then_some(record.updated_at),
    };
    if let Some(started) = started {
        anchors.push(Anchor::Age(started));
        let mut fact = Fact::new("Started", age_short(now - started));
        fact.description = Some(exact_time(started, offset));
        facts.push(fact);
    }
    let duration = match (record.status, started, finished) {
        (BuildStatus::Queued, _, _) => Some("Waiting to start".to_owned()),
        (status, Some(started), None) if status.is_active() => {
            anchors.push(Anchor::Elapsed(started));
            Some(format!("Running for {}", running_words(now - started)))
        }
        (_, Some(started), Some(finished)) => {
            Some(format!("Took {}", duration_words(finished - started)))
        }
        _ => None,
    };
    if let Some(duration) = duration {
        facts.push(Fact::new("Duration", duration));
    }
    facts
}

/// The first line of a commit message, trimmed and cut to
/// [`MAX_MESSAGE`] characters.
pub fn first_line(message: &str) -> String {
    let line = message
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() > MAX_MESSAGE {
        let cut: String = line.chars().take(MAX_MESSAGE - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        line.to_owned()
    }
}

/// Who or what started a run, in lower case ("pushed by octocat",
/// "scheduled", "re-run by hubot"); the panel capitalises it.
pub fn trigger_words(details: &RunDetails) -> Option<String> {
    let by = |who: Option<&String>| who.map(|w| format!(" by {w}")).unwrap_or_default();
    if details.run.attempt > 1 {
        return Some(format!(
            "re-run{}",
            by(details.triggered_by.as_ref().or(details.actor.as_ref()))
        ));
    }
    let actor = by(details.actor.as_ref());
    Some(match &details.trigger {
        Trigger::Push => format!("pushed{actor}"),
        Trigger::PullRequest => format!("pull request{actor}"),
        Trigger::Schedule => "scheduled".to_owned(),
        Trigger::Manual => format!(
            "started{}",
            if actor.is_empty() {
                " by hand".to_owned()
            } else {
                actor
            }
        ),
        Trigger::Api => format!("started through the API{actor}"),
        Trigger::Workflow => "after another workflow".to_owned(),
        Trigger::Other(word) => format!("{}{actor}", word.replace('_', " ")),
        Trigger::Unknown if !actor.is_empty() => format!("started{actor}"),
        Trigger::Unknown => return None,
    })
}

/// A finished duration: "45 s", "4 min 12 s", "4 min", "1 h 5 min".
pub fn duration_words(duration: TimeDelta) -> String {
    let secs = duration.num_seconds().max(0);
    match secs {
        0..60 => format!("{secs} s"),
        60..3_600 => match secs % 60 {
            0 => format!("{} min", secs / 60),
            s => format!("{} min {s} s", secs / 60),
        },
        _ => match (secs % 3_600) / 60 {
            0 => format!("{} h", secs / 3_600),
            m => format!("{} h {m} min", secs / 3_600),
        },
    }
}

/// How long something has been running, in whole minutes so it changes at
/// most once a minute: "under a minute", "3 min", "1 h 5 min".
pub fn running_words(elapsed: TimeDelta) -> String {
    let secs = elapsed.num_seconds().max(0);
    match secs {
        0..60 => "under a minute".to_owned(),
        _ => duration_words(TimeDelta::seconds(secs - secs % 60)),
    }
}

/// The exact local time, British style: "16 Nov 2026, 10:06".
pub fn exact_time(at: DateTime<Utc>, offset: FixedOffset) -> String {
    at.with_timezone(&offset)
        .format("%-d %b %Y, %H:%M")
        .to_string()
}

/// The reason in "Details could not be loaded: <reason>.".
pub fn failure_words(error: &DetailsError) -> String {
    use buildrail_core::ports::ProviderError as E;
    match error {
        DetailsError::Provider(provider, error) => {
            let name = provider.display_name();
            match error {
                E::AccessDenied { .. } => format!("{name} refused access"),
                E::NotFound { .. } => format!("{name} no longer has this run"),
                E::RateLimited { .. } => format!("{name} is limiting requests for now"),
                E::Network(_) => format!("{name} could not be reached"),
                E::Server { status, .. } => format!("{name} had a problem (HTTP {status})"),
                E::Malformed(_) => format!("{name} sent something BuildRail could not read"),
            }
        }
        DetailsError::NoCredential(alias) => {
            format!("there is no token for the “{alias}” connection on this device")
        }
        DetailsError::CredentialStore => "the system's credential store did not answer".to_owned(),
        DetailsError::NoSource(provider) => {
            format!("{provider} details are not available in this build")
        }
    }
}

fn job_row(job: &JobSummary, now: DateTime<Utc>, anchors: &mut Vec<Anchor>) -> JobRow {
    let duration = match (job.status, job.started_at, job.finished_at) {
        (BuildStatus::Queued, _, _) => Some("waiting".to_owned()),
        (status, Some(started), None) if status.is_active() => {
            anchors.push(Anchor::Elapsed(started));
            Some(format!("running for {}", running_words(now - started)))
        }
        (_, Some(started), Some(finished)) => Some(duration_words(finished - started)),
        _ => None,
    };
    let failing_step = job
        .failing_step
        .as_ref()
        .filter(|_| job.status == BuildStatus::Failed)
        .map(|step| format!("Failed at “{step}”."));
    let mut spoken = vec![job.name.clone(), job.status.label().to_owned()];
    spoken.extend(duration.clone());
    if let (Some(step), BuildStatus::Failed) = (&job.failing_step, job.status) {
        spoken.push(format!("failed at step {step}"));
    }
    if job.url.is_some() {
        spoken.push("opens the job".to_owned());
    }
    JobRow {
        name: job.name.clone(),
        status_text: job.status.label().to_owned(),
        tone: job.status.tone(),
        icon: IconKind::for_status(job.status),
        duration,
        failing_step,
        url: job.url.clone(),
        accessible_name: spoken.join(", "),
    }
}

fn jobs_section(
    details: &RunDetails,
    run_url: Option<&str>,
    now: DateTime<Utc>,
    anchors: &mut Vec<Anchor>,
) -> Option<JobsSection> {
    let jobs = details.jobs.as_ref()?;
    // Failed first, each group in the provider's order.
    let (failed, others): (Vec<&JobSummary>, Vec<&JobSummary>) =
        jobs.iter().partition(|j| j.status == BuildStatus::Failed);
    let rows: Vec<JobRow> = failed
        .into_iter()
        .chain(others)
        .take(MAX_JOBS)
        .map(|job| job_row(job, now, anchors))
        .collect();
    let hidden = details
        .jobs_total
        .max(jobs.len())
        .saturating_sub(rows.len());
    let more = match (hidden, run_url) {
        (0, _) | (_, None) => None,
        (n, Some(url)) => Some(MoreLink {
            text: format!("and {n} more"),
            url: url.to_owned(),
            accessible_name: format!(
                "and {n} more {}, opens the run",
                if n == 1 { "job" } else { "jobs" }
            ),
        }),
    };
    let empty = rows
        .is_empty()
        .then(|| "No jobs have started yet.".to_owned());
    Some(JobsSection {
        heading: "Jobs",
        rows,
        more,
        empty,
    })
}

fn recent_section(
    watch: &Watch,
    runs: &[BuildRecord],
    current: Option<&BuildRecord>,
    now: DateTime<Utc>,
    anchors: &mut Vec<Anchor>,
) -> Option<RecentSection> {
    // One entry per run (its newest attempt), newest run first.
    let mut latest: Vec<BuildRecord> = Vec::new();
    let mut ids: Vec<&buildrail_core::domain::RunId> = runs.iter().map(|r| &r.run_id).collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        latest.extend(select_latest(
            runs.iter().filter(|r| &r.run_id == id).cloned(),
        ));
    }
    // The strip's own record is the freshest word on the current run.
    if let Some(current) = current {
        latest.retain(|r| r.run_id != current.run_id);
        latest.push(current.clone());
    }
    latest.sort_by(|a, b| (b.created_at, &b.run_id).cmp(&(a.created_at, &a.run_id)));
    latest.truncate(MAX_RECENT);
    if latest.len() < 2 {
        // Only the run already described above: nothing to compare with.
        return None;
    }
    let show_branch = watch.target.branch().is_none();
    let noun = match watch.target.provider() {
        ProviderKind::GitHub => ("run", "runs", "Recent runs"),
        ProviderKind::Cloudflare => ("build", "builds", "Recent builds"),
    };
    let entries = latest
        .iter()
        .map(|r| {
            anchors.push(Anchor::Age(r.created_at));
            let is_current = current.is_some_and(|c| c.run_id == r.run_id);
            let branch = r.branch.clone().filter(|_| show_branch);
            let mut spoken = vec![r.status.label().to_owned(), age_long(now - r.created_at)];
            if let Some(branch) = &branch {
                spoken.push(format!("branch {branch}"));
            }
            if is_current {
                spoken.push(format!("the {} shown above", noun.0));
            }
            spoken.push(format!("opens the {}", noun.0));
            RecentRun {
                icon: IconKind::for_status(r.status),
                tone: r.status.tone(),
                status_text: r.status.label().to_owned(),
                when: age_short(now - r.created_at),
                branch,
                url: r.url.clone(),
                current: is_current,
                accessible_name: spoken.join(", "),
            }
        })
        .collect::<Vec<_>>();
    let total = entries.len();
    let failed = latest
        .iter()
        .filter(|r| r.status == BuildStatus::Failed)
        .count();
    let summary = match failed {
        0 => format!("None of the last {total} {} failed.", noun.1),
        n if n == total => format!("All of the last {total} {} failed.", noun.1),
        n => format!("{n} of the last {total} {} failed.", noun.1),
    };
    Some(RecentSection {
        heading: noun.2,
        runs: entries,
        summary: Some(summary),
    })
}

#[cfg(test)]
mod tests;
