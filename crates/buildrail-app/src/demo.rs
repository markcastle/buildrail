//! `--demo`: the real UI against an in-memory provider, with a canned
//! profile. No network, no keyring, nothing written outside a temporary
//! (or `BUILDRAIL_CONFIG_DIR`) directory. Useful for manual testing and
//! screenshots on each OS.
//!
//! [`DemoRunSource`] derives every answer from the clock, so the strip
//! keeps moving: each target cycles queued → running → passed/failed →
//! re-run, offset so entries change at different times. A few fixed
//! targets show the other states: an empty history ("No runs yet"), an
//! access error ("Reconnect needed"), cancelled and skipped runs. It also
//! lists a few earlier runs and answers the details panel with believable
//! jobs (a failing step on failed runs), commit messages and triggers.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use async_trait::async_trait;
use buildrail_core::details::{JobSummary, RunDetails, RunRef, Trigger};
use buildrail_core::domain::{BuildRecord, BuildStatus, ProviderKind, RunId, WatchTarget};
use buildrail_core::ports::{Clock, ProviderError, RunDetailsSource, RunSource, Secret};
use buildrail_platform::url_opener::{validate_https_url, OpenUrlError, UrlOpener};
use buildrail_profile::{LocalConnection, Profile, WatchDraft};
use chrono::{DateTime, TimeDelta, Utc};

use crate::fakes::target_name;

/// Seconds each phase of a demo run lasts.
pub const PHASE_SECS: i64 = 6;

/// One step of a demo run's life.
const PHASES: [(BuildStatus, u32); 6] = [
    (BuildStatus::Queued, 1),
    (BuildStatus::Running, 1),
    (BuildStatus::Running, 1),
    (BuildStatus::Failed, 1),
    (BuildStatus::Running, 2),
    (BuildStatus::Passed, 2),
];

/// A run source whose answers are a function of the clock. See the module
/// docs.
pub struct DemoRunSource {
    kind: ProviderKind,
    clock: Arc<dyn Clock>,
    epoch: DateTime<Utc>,
}

impl DemoRunSource {
    /// A source for `kind` whose cycles start at `epoch`.
    pub fn new(kind: ProviderKind, clock: Arc<dyn Clock>, epoch: DateTime<Utc>) -> Self {
        Self { kind, clock, epoch }
    }

    /// The record shown for `name` at `now` (`None`: empty history).
    pub fn record_at(
        &self,
        name: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<BuildRecord>, ProviderError> {
        Ok(self.history_at(name, now)?.into_iter().next())
    }

    /// The runs listed for `name` at `now`, newest first: the current run
    /// and the [`HISTORY`] runs before it (`[]`: empty history).
    pub fn history_at(
        &self,
        name: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<BuildRecord>, ProviderError> {
        let hash = name_hash(name);
        let fixed = match name {
            "docs" => return Ok(Vec::new()),
            "billing" => {
                return Err(ProviderError::AccessDenied {
                    provider: self.kind,
                })
            }
            "mobile" => Some(BuildStatus::Cancelled),
            "infra" => Some(BuildStatus::Skipped),
            "payments" => Some(BuildStatus::Failed),
            _ => None,
        };
        let elapsed = (now - self.epoch).num_seconds().max(0);
        let offset = i64::try_from(hash % 6).unwrap_or(0);
        let step = elapsed / PHASE_SECS + offset;
        let cycle = step / 6;
        let (mut status, attempt) = PHASES[usize::try_from(step % 6).unwrap_or(0)];
        if let Some(fixed) = fixed {
            status = fixed;
        }
        let updated = self.epoch + TimeDelta::seconds((step - offset) * PHASE_SECS);
        let mut runs = vec![self.run(name, hash, cycle, offset, status, attempt, updated, now)];
        for back in 1..=HISTORY {
            let past = cycle - back;
            // Earlier runs mostly passed; some failed, a pattern per name.
            let status = match fixed {
                Some(BuildStatus::Failed) if back < 4 => BuildStatus::Failed,
                Some(BuildStatus::Failed) => BuildStatus::Passed,
                Some(fixed) => fixed,
                None if (hash.wrapping_add(u64::try_from(past.rem_euclid(97)).unwrap_or(0)))
                    % 4
                    == 0 =>
                {
                    BuildStatus::Failed
                }
                None => BuildStatus::Passed,
            };
            let ended = self.epoch + TimeDelta::seconds(((past * 6 + 5) - offset) * PHASE_SECS);
            runs.push(self.run(name, hash, past, offset, status, 1, ended, now));
        }
        Ok(runs)
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        name: &str,
        hash: u64,
        cycle: i64,
        offset: i64,
        mut status: BuildStatus,
        attempt: u32,
        updated: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> BuildRecord {
        if status == BuildStatus::Passed && self.kind == ProviderKind::Cloudflare {
            status = BuildStatus::Deployed;
        }
        let cycle_start = self.epoch + TimeDelta::seconds((cycle * 6 - offset) * PHASE_SECS);
        let run = 4_000 + cycle;
        let url = match self.kind {
            ProviderKind::GitHub => format!("https://github.com/acme/{name}/actions/runs/{run}"),
            ProviderKind::Cloudflare => format!(
                "https://dash.cloudflare.com/0123456789abcdef0123456789abcdef/workers/services/view/{name}/production/builds/{run}"
            ),
        };
        BuildRecord {
            run_id: RunId::new(run.to_string()),
            attempt,
            status,
            url,
            branch: Some("main".into()),
            commit: Some(format!(
                "{:016x}",
                hash.rotate_left(u32::try_from(cycle.rem_euclid(64)).unwrap_or(0))
            )),
            failing_job: (status == BuildStatus::Failed).then(|| "test (ubuntu-latest)".to_owned()),
            created_at: cycle_start,
            updated_at: updated,
            observed_at: now,
        }
    }

    /// Believable details for one of the runs [`Self::history_at`] lists.
    pub fn details_of(&self, name: &str, record: &BuildRecord) -> RunDetails {
        let hash = name_hash(name);
        let mut details = RunDetails::new(RunRef::from(record));
        let messages = [
            "Speed up the search index rebuild",
            "Fix the flaky login test on Windows",
            "Bump serde to 1.0.215",
            "Show the invoice total in the header",
            "Retry uploads when the network drops",
        ];
        let pick = usize::try_from(
            hash.wrapping_add(record.run_id.as_str().parse::<u64>().unwrap_or(0)) % 5,
        )
        .unwrap_or(0);
        details.commit_message = Some(format!("{}\n\nDemo commit.", messages[pick]));
        details.run_number = record.run_id.as_str().parse().ok();
        details.trigger = Trigger::Push;
        let started = record.created_at + TimeDelta::seconds(PHASE_SECS);
        details.started_at = (record.status != BuildStatus::Queued).then_some(started);
        details.finished_at =
            (!record.status.is_active()).then_some(record.updated_at.max(started));
        match self.kind {
            ProviderKind::GitHub => {
                details.title = Some(messages[pick].to_owned());
                details.actor = Some("octocat".into());
                if record.attempt > 1 {
                    details.triggered_by = Some("mona".into());
                }
                let jobs = demo_jobs(record, started);
                details.jobs_total = jobs.len();
                details.jobs = Some(jobs);
            }
            ProviderKind::Cloudflare => {
                details.actor = Some("mona@acme.dev".into());
            }
        }
        details
    }
}

/// The jobs of a demo GitHub run in `record`'s state.
fn demo_jobs(record: &BuildRecord, started: DateTime<Utc>) -> Vec<JobSummary> {
    use BuildStatus::{Cancelled, Failed, Passed, Queued, Running, Skipped};
    let names = [
        "lint",
        "test (ubuntu-latest)",
        "test (windows-latest)",
        "test (macos-latest)",
        "build",
    ];
    let states: [BuildStatus; 5] = match record.status {
        Queued => [Queued; 5],
        Running => [Passed, Running, Running, Passed, Queued],
        Failed => [Passed, Failed, Passed, Passed, Skipped],
        Cancelled => [Passed, Cancelled, Cancelled, Cancelled, Skipped],
        Skipped => [Skipped; 5],
        _ => [Passed; 5],
    };
    names
        .iter()
        .zip(states)
        .enumerate()
        .map(|(i, (name, status))| {
            let secs = 40 + 35 * i64::try_from(i).unwrap_or(0);
            let job_start = started + TimeDelta::seconds(2);
            JobSummary {
                name: (*name).to_owned(),
                status,
                started_at: (status != Queued && status != Skipped).then_some(job_start),
                finished_at: (!status.is_active() && status != Skipped)
                    .then(|| job_start + TimeDelta::seconds(secs)),
                url: Some(format!("{}/job/{}", record.url, 9_000 + i)),
                failing_step: (status == Failed).then(|| "cargo test --workspace".to_owned()),
            }
        })
        .collect()
}

fn name_hash(name: &str) -> u64 {
    let mut h = DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
}

/// How many earlier runs a demo target lists besides its current one.
pub const HISTORY: i64 = 6;

#[async_trait]
impl RunDetailsSource for DemoRunSource {
    fn provider(&self) -> ProviderKind {
        self.kind
    }

    async fn run_details(
        &self,
        target: &WatchTarget,
        run: &RunRef,
        _credential: &Secret,
    ) -> Result<RunDetails, ProviderError> {
        let name = target_name(target);
        let history = self.history_at(name, self.clock.now())?;
        let record =
            history
                .iter()
                .find(|r| r.run_id == run.run_id)
                .ok_or(ProviderError::NotFound {
                    provider: self.kind,
                })?;
        Ok(self.details_of(name, record))
    }
}

#[async_trait]
impl RunSource for DemoRunSource {
    fn provider(&self) -> ProviderKind {
        self.kind
    }

    async fn recent_runs(
        &self,
        target: &WatchTarget,
        _credential: &Secret,
    ) -> Result<Vec<BuildRecord>, ProviderError> {
        self.history_at(target_name(target), self.clock.now())
    }
}

/// Logs instead of opening a browser (demo links point at made-up runs).
#[derive(Debug, Default)]
pub struct LoggingUrlOpener;

impl UrlOpener for LoggingUrlOpener {
    fn open(&self, url: &str) -> Result<(), OpenUrlError> {
        let url = validate_https_url(url)?;
        tracing::info!(%url, "demo: would open the run page");
        Ok(())
    }
}

/// Connection aliases used by the demo profile, with their providers.
pub fn demo_connections() -> Vec<(&'static str, LocalConnection)> {
    vec![
        (
            "work",
            LocalConnection {
                provider: ProviderKind::GitHub,
                display_name: "octocat".into(),
            },
        ),
        (
            "acme-cf",
            LocalConnection {
                provider: ProviderKind::Cloudflare,
                display_name: "Acme".into(),
            },
        ),
    ]
}

/// The canned demo profile.
pub fn demo_profile() -> Result<Profile, buildrail_profile::ValidationErrors> {
    let gh = |label: &str, repo: &str| {
        WatchDraft::github(label, "work", "acme", repo, "ci.yml", Some("main".into())).with_id(repo)
    };
    let cf = |label: &str, worker: &str| {
        WatchDraft::cloudflare(
            label,
            "acme-cf",
            "0123456789abcdef0123456789abcdef",
            worker,
            None,
        )
        .with_id(worker)
    };
    Profile::from_drafts(&[
        gh("Keevoro", "keevoro"),
        gh("API", "api"),
        cf("Marketing site", "marketing-site"),
        cf("Docs", "docs"),
        gh("Billing", "billing"),
        gh("Mobile", "mobile"),
        gh("Infra", "infra"),
        gh("Payments", "payments"),
    ])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::HashSet;

    use chrono::TimeZone;

    use super::*;
    use crate::fakes::ManualClock;

    fn epoch() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).unwrap()
    }

    #[test]
    fn cycles_through_states_and_newer_runs_supersede() {
        let clock = Arc::new(ManualClock::new(epoch()));
        let source = DemoRunSource::new(ProviderKind::GitHub, clock, epoch());
        let mut seen = HashSet::new();
        let mut previous: Option<BuildRecord> = None;
        for step in 0..24 {
            let now = epoch() + TimeDelta::seconds(step * PHASE_SECS);
            let record = source.record_at("keevoro", now).unwrap().unwrap();
            seen.insert(record.status);
            assert!(record
                .url
                .starts_with("https://github.com/acme/keevoro/actions/runs/"));
            if let Some(previous) = &previous {
                if previous.run_id != record.run_id
                    || previous.attempt != record.attempt
                    || previous.status != record.status
                {
                    assert!(
                        buildrail_core::selection::supersedes(&record, previous),
                        "step {step}: {record:?} vs {previous:?}"
                    );
                }
            }
            previous = Some(record);
        }
        for status in [
            BuildStatus::Queued,
            BuildStatus::Running,
            BuildStatus::Failed,
            BuildStatus::Passed,
        ] {
            assert!(seen.contains(&status), "{status:?}");
        }
    }

    #[tokio::test]
    async fn fixed_targets_and_cloudflare() {
        let clock = Arc::new(ManualClock::new(
            epoch() + TimeDelta::seconds(5 * PHASE_SECS),
        ));
        let cf = DemoRunSource::new(ProviderKind::Cloudflare, clock.clone(), epoch());
        assert_eq!(RunSource::provider(&cf), ProviderKind::Cloudflare);
        assert_eq!(RunDetailsSource::provider(&cf), ProviderKind::Cloudflare);
        let target = |worker: &str| WatchTarget::Cloudflare {
            account_id: "a".into(),
            worker: worker.into(),
            branch: None,
        };
        let secret = Secret::new("x");
        assert!(cf
            .recent_runs(&target("docs"), &secret)
            .await
            .unwrap()
            .is_empty());
        assert!(cf.recent_runs(&target("billing"), &secret).await.is_err());
        let statuses: HashSet<_> = (0..6)
            .map(|i| {
                cf.record_at("site", epoch() + TimeDelta::seconds(i * PHASE_SECS))
                    .unwrap()
                    .unwrap()
                    .status
            })
            .collect();
        assert!(statuses.contains(&BuildStatus::Deployed));
        assert_eq!(
            cf.record_at("mobile", epoch()).unwrap().unwrap().status,
            BuildStatus::Cancelled
        );
        assert_eq!(
            cf.record_at("infra", epoch()).unwrap().unwrap().status,
            BuildStatus::Skipped
        );
        assert_eq!(
            cf.record_at("payments", epoch()).unwrap().unwrap().status,
            BuildStatus::Failed
        );
        assert!(cf.recent_runs(&target("x"), &secret).await.unwrap()[0]
            .url
            .starts_with("https://dash.cloudflare.com/"));
    }

    #[tokio::test]
    async fn history_and_believable_details_for_every_state() {
        let clock = Arc::new(ManualClock::new(epoch()));
        let gh = DemoRunSource::new(ProviderKind::GitHub, clock.clone(), epoch());
        let cf = DemoRunSource::new(ProviderKind::Cloudflare, clock.clone(), epoch());
        let target = |repo: &str| WatchTarget::GitHub {
            owner: "acme".into(),
            repo: repo.into(),
            workflow: "ci.yml".into(),
            branch: None,
        };
        let secret = Secret::new("x");
        let history = gh.recent_runs(&target("keevoro"), &secret).await.unwrap();
        assert_eq!(history.len(), 1 + usize::try_from(HISTORY).unwrap());
        assert_eq!(
            buildrail_core::selection::select_latest(history.clone()),
            Some(history[0].clone()),
            "the current run is the newest"
        );
        assert_eq!(
            gh.history_at("payments", epoch()).unwrap()[1].status,
            BuildStatus::Failed
        );
        // Every state the demo shows gets jobs that match it.
        let mut seen = HashSet::new();
        for step in 0..6 {
            clock.set(epoch() + TimeDelta::seconds(step * PHASE_SECS));
            for name in ["keevoro", "mobile", "infra", "payments"] {
                let current = gh.record_at(name, clock.now()).unwrap().unwrap();
                let run = RunRef::from(&current);
                let details = gh.run_details(&target(name), &run, &secret).await.unwrap();
                let jobs = details.jobs.clone().unwrap();
                assert_eq!(jobs.len(), details.jobs_total);
                assert!(details.commit_message.is_some());
                if current.status == BuildStatus::Failed {
                    assert!(jobs.iter().any(|j| j.failing_step.is_some()));
                }
                if current.attempt > 1 {
                    assert_eq!(details.triggered_by.as_deref(), Some("mona"));
                }
                seen.insert(current.status);
            }
        }
        assert!(seen.len() >= 5, "{seen:?}");
        let missing = RunRef {
            run_id: RunId::new("1"),
            attempt: 1,
        };
        assert!(gh
            .run_details(&target("keevoro"), &missing, &secret)
            .await
            .is_err());
        let site = WatchTarget::Cloudflare {
            account_id: "a".into(),
            worker: "site".into(),
            branch: None,
        };
        let current = cf.record_at("site", clock.now()).unwrap().unwrap();
        let details = cf
            .run_details(&site, &RunRef::from(&current), &secret)
            .await
            .unwrap();
        assert_eq!(details.jobs, None);
        assert_eq!(details.actor.as_deref(), Some("mona@acme.dev"));
        assert_eq!(RunDetailsSource::provider(&cf), ProviderKind::Cloudflare);
    }

    #[test]
    fn canned_profile_and_opener() {
        assert_eq!(demo_profile().unwrap().len(), 8);
        assert_eq!(demo_connections().len(), 2);
        assert!(LoggingUrlOpener.open("https://example.com").is_ok());
        assert!(LoggingUrlOpener.open("file:///etc/passwd").is_err());
    }
}
