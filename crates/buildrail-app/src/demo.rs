//! `--demo`: the real UI against an in-memory provider, with a canned
//! profile. No network, no keyring, nothing written outside a temporary
//! (or `BUILDRAIL_CONFIG_DIR`) directory. Useful for manual testing and
//! screenshots on each OS.
//!
//! [`DemoRunSource`] derives every answer from the clock, so the strip
//! keeps moving: each target cycles queued → running → passed/failed →
//! re-run, offset so entries change at different times. A few fixed
//! targets show the other states: an empty history ("No runs yet"), an
//! access error ("Reconnect needed"), cancelled and skipped runs.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use async_trait::async_trait;
use buildrail_core::domain::{BuildRecord, BuildStatus, ProviderKind, RunId, WatchTarget};
use buildrail_core::ports::{Clock, ProviderError, RunSource, Secret};
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
        let hash = {
            let mut h = DefaultHasher::new();
            name.hash(&mut h);
            h.finish()
        };
        let fixed = match name {
            "docs" => return Ok(None),
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
        if status == BuildStatus::Passed && self.kind == ProviderKind::Cloudflare {
            status = BuildStatus::Deployed;
        }
        let cycle_start = self.epoch + TimeDelta::seconds((cycle * 6 - offset) * PHASE_SECS);
        let updated = self.epoch + TimeDelta::seconds((step - offset) * PHASE_SECS);
        let run = 4_000 + cycle;
        let url = match self.kind {
            ProviderKind::GitHub => format!("https://github.com/acme/{name}/actions/runs/{run}"),
            ProviderKind::Cloudflare => format!(
                "https://dash.cloudflare.com/0123456789abcdef0123456789abcdef/workers/services/view/{name}/production/builds/{run}"
            ),
        };
        Ok(Some(BuildRecord {
            run_id: RunId::new(run.to_string()),
            attempt,
            status,
            url,
            branch: Some("main".into()),
            commit: Some(format!(
                "{:016x}",
                hash.rotate_left(u32::try_from(cycle % 64).unwrap_or(0))
            )),
            failing_job: (status == BuildStatus::Failed).then(|| "test (ubuntu-latest)".to_owned()),
            created_at: cycle_start,
            updated_at: updated,
            observed_at: now,
        }))
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
        let now = self.clock.now();
        self.record_at(target_name(target), now)
            .map(|record| record.into_iter().collect())
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
        assert_eq!(cf.provider(), ProviderKind::Cloudflare);
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

    #[test]
    fn canned_profile_and_opener() {
        assert_eq!(demo_profile().unwrap().len(), 8);
        assert_eq!(demo_connections().len(), 2);
        assert!(LoggingUrlOpener.open("https://example.com").is_ok());
        assert!(LoggingUrlOpener.open("file:///etc/passwd").is_err());
    }
}
