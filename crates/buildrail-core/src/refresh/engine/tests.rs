//! Engine tests. Tokio time is paused, so timers auto-advance once every
//! task is idle; the fake wall clock follows tokio time plus a manual
//! offset, which is how wake from sleep is simulated.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use tokio::task::JoinHandle;

use super::*;
use crate::domain::{
    BuildStatus, ConnectionAlias, Freshness, ProviderKind, RunId, StaleReason, WatchId, WatchTarget,
};
use crate::ports::{CredentialError, ProviderError, RunSource, Secret};

// ---------------------------------------------------------------- fakes

struct FakeClock {
    base: DateTime<Utc>,
    start: Instant,
    offset: Mutex<TimeDelta>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            base: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            start: Instant::now(),
            offset: Mutex::new(TimeDelta::zero()),
        }
    }

    /// Wall time jumps forward without monotonic time moving: a suspend.
    fn jump(&self, by: TimeDelta) {
        *self.offset.lock().unwrap() += by;
    }

    fn at(&self, secs: u64) -> DateTime<Utc> {
        self.base + TimeDelta::seconds(i64::try_from(secs).unwrap())
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        let mono = TimeDelta::from_std(self.start.elapsed()).unwrap();
        self.base + mono + *self.offset.lock().unwrap()
    }
}

#[derive(Clone)]
enum Reply {
    Runs(Vec<BuildRecord>),
    Fail(ProviderError),
    Panic,
}

/// Identifies a target by repo (GitHub) or worker (Cloudflare) name.
fn name_of(target: &WatchTarget) -> String {
    match target {
        WatchTarget::GitHub { repo, .. } => repo.clone(),
        WatchTarget::Cloudflare { worker, .. } => worker.clone(),
    }
}

struct FakeSource {
    kind: ProviderKind,
    /// Per-target replies; the last one repeats forever.
    replies: Mutex<HashMap<String, VecDeque<Reply>>>,
    delays: Mutex<HashMap<String, Duration>>,
    calls: Mutex<Vec<(String, Duration, String)>>,
    start: Instant,
}

impl FakeSource {
    fn new(kind: ProviderKind) -> Arc<Self> {
        Arc::new(Self {
            kind,
            replies: Mutex::new(HashMap::new()),
            delays: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            start: Instant::now(),
        })
    }

    fn reply(&self, name: &str, replies: Vec<Reply>) {
        self.replies
            .lock()
            .unwrap()
            .insert(name.to_owned(), replies.into());
    }

    fn delay(&self, name: &str, delay: Duration) {
        self.delays.lock().unwrap().insert(name.to_owned(), delay);
    }

    fn calls_to(&self, name: &str) -> Vec<u64> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _, _)| n == name)
            .map(|(_, at, _)| at.as_secs())
            .collect()
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    fn secrets_used(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, s)| s.clone())
            .collect()
    }
}

#[async_trait]
impl RunSource for FakeSource {
    fn provider(&self) -> ProviderKind {
        self.kind
    }

    async fn recent_runs(
        &self,
        target: &WatchTarget,
        credential: &Secret,
    ) -> Result<Vec<BuildRecord>, ProviderError> {
        let name = name_of(target);
        self.calls.lock().unwrap().push((
            name.clone(),
            self.start.elapsed(),
            credential.expose().to_owned(),
        ));
        let delay = self.delays.lock().unwrap().get(&name).copied();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let reply = {
            let mut replies = self.replies.lock().unwrap();
            match replies.get_mut(&name) {
                Some(queue) if queue.len() > 1 => queue.pop_front(),
                Some(queue) => queue.front().cloned(),
                None => None,
            }
        };
        match reply.unwrap_or(Reply::Runs(Vec::new())) {
            Reply::Runs(runs) => Ok(runs),
            Reply::Fail(error) => Err(error),
            Reply::Panic => panic!("adapter bug"),
        }
    }
}

#[derive(Default)]
struct FakeCredentials {
    overrides: Mutex<HashMap<String, Result<Option<Secret>, CredentialError>>>,
}

impl FakeCredentials {
    fn set(&self, alias: &str, value: Result<Option<Secret>, CredentialError>) {
        self.overrides
            .lock()
            .unwrap()
            .insert(alias.to_owned(), value);
    }
}

impl CredentialReader for FakeCredentials {
    fn read(&self, alias: &ConnectionAlias) -> Result<Option<Secret>, CredentialError> {
        self.overrides
            .lock()
            .unwrap()
            .get(alias.as_str())
            .cloned()
            .unwrap_or_else(|| Ok(Some(Secret::new(format!("token-{alias}")))))
    }
}

#[derive(Default)]
struct FakeCache {
    stored: Vec<WatchSnapshot>,
    saves: Mutex<Vec<Vec<WatchSnapshot>>>,
}

impl SnapshotCache for FakeCache {
    fn load(&self) -> Vec<WatchSnapshot> {
        self.stored.clone()
    }

    fn save(&self, snapshots: &[WatchSnapshot]) {
        self.saves.lock().unwrap().push(snapshots.to_vec());
    }
}

// -------------------------------------------------------------- harness

struct Harness {
    handle: RefreshHandle,
    github: Arc<FakeSource>,
    cloudflare: Arc<FakeSource>,
    credentials: Arc<FakeCredentials>,
    clock: Arc<FakeClock>,
    cache: Arc<FakeCache>,
    task: JoinHandle<()>,
}

struct Setup {
    cached: Vec<WatchSnapshot>,
    register_cloudflare: bool,
    policy: RefreshPolicy,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            cached: Vec::new(),
            register_cloudflare: true,
            policy: RefreshPolicy::default(),
        }
    }
}

impl Harness {
    fn start() -> Self {
        Self::with(Setup::default())
    }

    fn with(setup: Setup) -> Self {
        let github = FakeSource::new(ProviderKind::GitHub);
        let cloudflare = FakeSource::new(ProviderKind::Cloudflare);
        let credentials = Arc::new(FakeCredentials::default());
        let clock = Arc::new(FakeClock::new());
        let cache = Arc::new(FakeCache {
            stored: setup.cached,
            saves: Mutex::new(Vec::new()),
        });
        let mut sources = SourceRegistry::new().with(github.clone());
        if setup.register_cloudflare {
            sources.register(cloudflare.clone());
        }
        let (engine, handle) = RefreshEngine::new(
            sources,
            credentials.clone(),
            clock.clone(),
            cache.clone(),
            setup.policy,
        );
        let task = tokio::spawn(engine.run());
        Self {
            handle,
            github,
            cloudflare,
            credentials,
            clock,
            cache,
            task,
        }
    }

    fn snapshot(&self, id: &str) -> WatchSnapshot {
        self.handle
            .snapshots()
            .iter()
            .find(|s| s.watch_id.as_str() == id)
            .cloned()
            .unwrap()
    }

    fn ids(&self) -> Vec<String> {
        self.handle
            .snapshots()
            .iter()
            .map(|s| s.watch_id.to_string())
            .collect()
    }
}

/// Advances paused time to `secs` after the test started (plus a hair so
/// work due exactly then has run).
async fn until(start: Instant, secs: u64) {
    let target = start + Duration::from_secs(secs) + Duration::from_millis(1);
    // Always yield to the engine, even when the target is already past.
    let floor = Instant::now() + Duration::from_millis(1);
    tokio::time::sleep_until(target.max(floor)).await;
}

fn gh(id: &str, conn: &str, repo: &str) -> Watch {
    Watch {
        id: WatchId::new(id),
        label: id.into(),
        connection: ConnectionAlias::new(conn),
        target: WatchTarget::GitHub {
            owner: "o".into(),
            repo: repo.into(),
            workflow: "ci.yml".into(),
            branch: None,
        },
    }
}

fn cf(id: &str, conn: &str, worker: &str) -> Watch {
    Watch {
        id: WatchId::new(id),
        label: id.into(),
        connection: ConnectionAlias::new(conn),
        target: WatchTarget::Cloudflare {
            account_id: "acc".into(),
            worker: worker.into(),
            branch: None,
        },
    }
}

fn run(id: &str, created: i64, status: BuildStatus) -> BuildRecord {
    let at = Utc.timestamp_opt(1_600_000_000 + created, 0).unwrap();
    BuildRecord {
        run_id: RunId::new(id),
        attempt: 1,
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

fn stale(reason: StaleReason) -> Freshness {
    Freshness::Stale { reason }
}

fn network() -> Reply {
    Reply::Fail(ProviderError::Network("unreachable".into()))
}

// ---------------------------------------------------------------- tests

#[tokio::test(start_paused = true)]
async fn polls_idle_every_minute_and_shares_identical_requests() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.github.reply(
        "a",
        vec![Reply::Runs(vec![run("1", 0, BuildStatus::Passed)])],
    );
    h.handle
        .set_watches(vec![
            gh("w1", "work", "a"),
            gh("w2", "work", "b"),
            gh("w3", "work", "a"),
        ])
        .unwrap();
    until(t0, 0).await;
    assert_eq!(h.github.calls_to("a"), vec![0]);
    assert_eq!(h.github.calls_to("b"), vec![0]);
    assert_eq!(h.ids(), ["w1", "w2", "w3"]);
    let w1 = h.snapshot("w1");
    assert_eq!(w1.freshness, Freshness::Fresh);
    assert_eq!(w1.last_success, Some(h.clock.at(0)));
    assert_eq!(h.snapshot("w3").record, w1.record);
    assert_eq!(h.github.secrets_used()[0], "token-work");

    until(t0, 59).await;
    assert_eq!(h.github.call_count(), 2);
    until(t0, 60).await;
    assert_eq!(h.github.calls_to("a"), vec![0, 60]);
    until(t0, 180).await;
    assert_eq!(h.github.calls_to("a"), vec![0, 60, 120, 180]);
}

#[tokio::test(start_paused = true)]
async fn active_runs_poll_every_fifteen_seconds() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.github.reply(
        "a",
        vec![
            Reply::Runs(vec![run("1", 0, BuildStatus::Queued)]),
            Reply::Runs(vec![run("1", 0, BuildStatus::Running)]),
            Reply::Runs(vec![run("1", 0, BuildStatus::Passed)]),
        ],
    );
    h.handle.set_watches(vec![gh("w", "work", "a")]).unwrap();
    until(t0, 100).await;
    assert_eq!(h.github.calls_to("a"), vec![0, 15, 30, 90]);
    assert_eq!(h.snapshot("w").record.unwrap().status, BuildStatus::Passed);
}

#[tokio::test(start_paused = true)]
async fn empty_history_is_fresh_without_record() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.handle
        .set_watches(vec![gh("w", "work", "empty")])
        .unwrap();
    until(t0, 0).await;
    let snap = h.snapshot("w");
    assert_eq!(snap.record, None);
    assert_eq!(snap.freshness, Freshness::Fresh);
}

#[tokio::test(start_paused = true)]
async fn rate_limit_pauses_every_request_on_that_connection() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.github.reply(
        "a",
        vec![
            Reply::Fail(ProviderError::RateLimited {
                provider: ProviderKind::GitHub,
                retry_after: Some(Duration::from_secs(150)),
            }),
            Reply::Runs(Vec::new()),
        ],
    );
    h.handle
        .set_watches(vec![
            gh("a", "work", "a"),
            gh("b", "work", "b"),
            gh("c", "home", "c"),
            cf("d", "work", "d"),
        ])
        .unwrap();
    until(t0, 0).await;
    assert_eq!(h.snapshot("a").freshness, stale(StaleReason::RateLimited));
    assert_eq!(h.snapshot("b").freshness, Freshness::Fresh);

    until(t0, 149).await;
    assert_eq!(h.github.calls_to("a"), vec![0]);
    assert_eq!(h.github.calls_to("b"), vec![0], "same connection is paused");
    assert_eq!(h.github.calls_to("c"), vec![0, 60, 120]);
    assert_eq!(h.cloudflare.calls_to("d"), vec![0, 60, 120]);

    // A manual refresh still honours the provider's deadline.
    h.handle.refresh_now().unwrap();
    until(t0, 149).await;
    assert_eq!(h.github.calls_to("b"), vec![0]);

    until(t0, 150).await;
    assert_eq!(h.github.calls_to("a"), vec![0, 150]);
    assert_eq!(h.github.calls_to("b"), vec![0, 150]);
    assert_eq!(h.snapshot("a").freshness, Freshness::Fresh);
}

#[tokio::test(start_paused = true)]
async fn failures_keep_cached_record_and_back_off_until_success() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.github.reply(
        "a",
        vec![
            Reply::Runs(vec![run("1", 0, BuildStatus::Passed)]),
            network(),
            network(),
            Reply::Runs(vec![run("2", 10, BuildStatus::Failed)]),
        ],
    );
    h.handle.set_watches(vec![gh("w", "work", "a")]).unwrap();
    until(t0, 60).await;
    let snap = h.snapshot("w");
    assert_eq!(snap.freshness, stale(StaleReason::Offline));
    assert_eq!(snap.record.unwrap().run_id, RunId::new("1"));
    assert_eq!(snap.last_success, Some(h.clock.at(0)));

    until(t0, 250).await;
    // 0 ok, 60 fail, +30 fail, +60 ok, +60 idle.
    assert_eq!(h.github.calls_to("a"), vec![0, 60, 90, 150, 210]);
    let snap = h.snapshot("w");
    assert_eq!(snap.freshness, Freshness::Fresh);
    assert_eq!(snap.record.unwrap().run_id, RunId::new("2"));
    assert_eq!(snap.last_success, Some(h.clock.at(210)));
}

#[tokio::test(start_paused = true)]
async fn older_run_finishing_late_never_replaces_newer() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.github.reply(
        "a",
        vec![
            Reply::Runs(vec![run("new", 10, BuildStatus::Running)]),
            Reply::Runs(vec![run("old", 0, BuildStatus::Failed)]),
        ],
    );
    h.handle.set_watches(vec![gh("w", "work", "a")]).unwrap();
    until(t0, 15).await;
    assert_eq!(h.github.calls_to("a"), vec![0, 15]);
    assert_eq!(h.snapshot("w").record.unwrap().run_id, RunId::new("new"));
}

#[tokio::test(start_paused = true)]
async fn refresh_now_fetches_immediately_and_clears_backoff() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.github.reply("a", vec![network()]);
    h.handle.set_watches(vec![gh("w", "work", "a")]).unwrap();
    until(t0, 40).await;
    assert_eq!(h.github.calls_to("a"), vec![0, 30]);
    h.handle.refresh_now().unwrap();
    until(t0, 40).await;
    assert_eq!(h.github.calls_to("a"), vec![0, 30, 40]);
    // Backoff restarted from the base: next at 40 + 30.
    until(t0, 69).await;
    assert_eq!(h.github.calls_to("a").len(), 3);
    until(t0, 71).await;
    assert_eq!(h.github.calls_to("a"), vec![0, 30, 40, 70]);
}

#[tokio::test(start_paused = true)]
async fn wall_clock_jump_is_treated_as_wake() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.github.reply("a", vec![network()]);
    h.handle.set_watches(vec![gh("w", "work", "a")]).unwrap();
    until(t0, 31).await;
    assert_eq!(h.github.calls_to("a"), vec![0, 30]);
    // Next due at 90 (backoff 60). The machine sleeps for eight hours.
    h.clock.jump(TimeDelta::hours(8));
    until(t0, 36).await;
    let calls = h.github.calls_to("a");
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(calls[2] <= 36);
    // Backoff was cleared: next retry 30 s later, not 120 s.
    let woke = calls[2];
    until(t0, woke + 30).await;
    assert_eq!(h.github.calls_to("a").len(), 4);
}

#[tokio::test(start_paused = true)]
async fn small_clock_drift_is_not_a_wake() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.handle.set_watches(vec![gh("w", "work", "a")]).unwrap();
    until(t0, 10).await;
    h.clock.jump(TimeDelta::seconds(20));
    until(t0, 59).await;
    assert_eq!(h.github.calls_to("a"), vec![0]);
}

#[tokio::test(start_paused = true)]
async fn missing_or_unreadable_credential_is_access_denied_without_a_call() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.credentials.set("none", Ok(None));
    h.credentials
        .set("broken", Err(CredentialError("keychain locked".into())));
    h.handle
        .set_watches(vec![gh("a", "none", "a"), gh("b", "broken", "b")])
        .unwrap();
    until(t0, 0).await;
    assert_eq!(h.github.call_count(), 0);
    assert_eq!(h.snapshot("a").freshness, stale(StaleReason::AccessDenied));
    assert_eq!(h.snapshot("b").freshness, stale(StaleReason::AccessDenied));

    // Reconnect: the credential is saved and the UI asks for a refresh.
    h.credentials.set("none", Ok(Some(Secret::new("fresh"))));
    h.handle.refresh_now().unwrap();
    until(t0, 1).await;
    assert_eq!(h.github.secrets_used(), ["fresh"]);
    assert_eq!(h.snapshot("a").freshness, Freshness::Fresh);
}

#[tokio::test(start_paused = true)]
async fn unregistered_provider_is_provider_error() {
    let t0 = Instant::now();
    let h = Harness::with(Setup {
        register_cloudflare: false,
        ..Setup::default()
    });
    h.handle.set_watches(vec![cf("w", "work", "x")]).unwrap();
    until(t0, 0).await;
    assert_eq!(h.snapshot("w").freshness, stale(StaleReason::ProviderError));
    assert_eq!(h.cloudflare.call_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn slow_provider_does_not_block_others_and_times_out() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.cloudflare.delay("slow", Duration::from_secs(100));
    h.handle
        .set_watches(vec![cf("slow", "work", "slow"), gh("fast", "work", "fast")])
        .unwrap();
    until(t0, 1).await;
    assert_eq!(h.snapshot("fast").freshness, Freshness::Fresh);
    assert_eq!(h.snapshot("slow").freshness, Freshness::Pending);
    until(t0, 30).await;
    assert_eq!(h.snapshot("slow").freshness, stale(StaleReason::Offline));
    // No duplicate request while the first was in flight.
    assert_eq!(h.cloudflare.calls_to("slow"), vec![0]);
}

#[tokio::test(start_paused = true)]
async fn panicking_adapter_is_isolated() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.github.reply("boom", vec![Reply::Panic]);
    h.handle
        .set_watches(vec![gh("boom", "work", "boom"), gh("ok", "work", "ok")])
        .unwrap();
    until(t0, 30).await;
    assert_eq!(
        h.snapshot("boom").freshness,
        stale(StaleReason::ProviderError)
    );
    assert_eq!(h.snapshot("ok").freshness, Freshness::Fresh);
    assert_eq!(
        h.github.calls_to("boom"),
        vec![0, 30],
        "retried with backoff"
    );
    assert!(!h.task.is_finished());
}

#[tokio::test(start_paused = true)]
async fn cached_snapshots_are_shown_then_replaced_and_changes_are_saved() {
    let t0 = Instant::now();
    let cached = WatchSnapshot {
        watch_id: WatchId::new("w"),
        record: Some(run("cached", 0, BuildStatus::Passed)),
        freshness: Freshness::Fresh,
        last_success: Some(Utc.timestamp_opt(1_699_000_000, 0).unwrap()),
    };
    let h = Harness::with(Setup {
        cached: vec![cached.clone()],
        ..Setup::default()
    });
    h.github.delay("a", Duration::from_secs(5));
    h.github.reply(
        "a",
        vec![Reply::Runs(vec![run("live", 1, BuildStatus::Failed)])],
    );
    assert!(h.handle.snapshots().is_empty(), "no watches yet");
    h.handle.set_watches(vec![gh("w", "work", "a")]).unwrap();
    until(t0, 1).await;
    let snap = h.snapshot("w");
    assert_eq!(snap.record, cached.record);
    assert_eq!(snap.freshness, Freshness::Pending);
    assert_eq!(snap.last_success, cached.last_success);

    until(t0, 6).await;
    let snap = h.snapshot("w");
    assert_eq!(snap.record.as_ref().unwrap().run_id, RunId::new("live"));
    let saves = h.cache.saves.lock().unwrap().clone();
    assert_eq!(saves.len(), 2, "seeded view, then live result");
    assert_eq!(saves.last().unwrap(), &vec![snap]);

    // Unchanged results (same record, same time) are not re-saved.
    h.handle.set_watches(vec![gh("w", "work", "a")]).unwrap();
    until(t0, 7).await;
    assert_eq!(h.cache.saves.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn set_watches_hot_swaps_without_refetching_unchanged_requests() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.handle
        .set_watches(vec![gh("a", "work", "a"), gh("b", "work", "b")])
        .unwrap();
    until(t0, 30).await;
    h.handle
        .set_watches(vec![cf("c", "work", "c"), gh("a2", "work", "a")])
        .unwrap();
    until(t0, 30).await;
    assert_eq!(h.ids(), ["c", "a2"]);
    assert_eq!(h.cloudflare.calls_to("c"), vec![30]);
    assert_eq!(h.snapshot("a2").freshness, Freshness::Fresh, "kept state");
    until(t0, 60).await;
    assert_eq!(h.github.calls_to("a"), vec![0, 60]);
    assert_eq!(h.github.calls_to("b"), vec![0], "removed watch not polled");
}

#[tokio::test(start_paused = true)]
async fn result_for_removed_watch_is_ignored() {
    let t0 = Instant::now();
    let h = Harness::start();
    h.github.delay("a", Duration::from_secs(10));
    h.handle.set_watches(vec![gh("a", "work", "a")]).unwrap();
    until(t0, 1).await;
    h.handle.set_watches(Vec::new()).unwrap();
    until(t0, 11).await;
    assert!(h.handle.snapshots().is_empty());
}

#[tokio::test(start_paused = true)]
async fn subscribers_are_notified_of_changes() {
    let h = Harness::start();
    let mut rx = h.handle.subscribe();
    h.handle.set_watches(vec![gh("w", "work", "a")]).unwrap();
    rx.changed().await.unwrap();
    assert_eq!(rx.borrow_and_update().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn shutdown_stops_the_engine() {
    let h = Harness::start();
    h.handle.shutdown();
    h.task.await.unwrap();
    assert_eq!(h.handle.refresh_now(), Err(EngineStopped));
    assert_eq!(h.handle.set_watches(Vec::new()), Err(EngineStopped));
    h.handle.shutdown(); // idempotent
    assert_eq!(EngineStopped.to_string(), "the refresh engine has stopped");
}

#[tokio::test(start_paused = true)]
async fn dropping_every_handle_stops_the_engine() {
    let h = Harness::start();
    let Harness { handle, task, .. } = h;
    let clone = handle.clone();
    drop(handle);
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    drop(clone);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn run_until_stops_on_external_signal() {
    let (engine, handle) = RefreshEngine::new(
        SourceRegistry::new(),
        Arc::new(FakeCredentials::default()),
        Arc::new(FakeClock::new()),
        Arc::new(FakeCache::default()),
        RefreshPolicy::default(),
    );
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(engine.run_until(async {
        let _ = rx.await;
    }));
    handle.set_watches(vec![gh("w", "work", "a")]).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        handle.snapshots()[0].freshness,
        stale(StaleReason::ProviderError)
    );
    tx.send(()).unwrap();
    task.await.unwrap();
}

#[test]
fn registry_is_keyed_by_reported_provider() {
    let first: Arc<dyn RunSource> = FakeSource::new(ProviderKind::GitHub);
    let second: Arc<dyn RunSource> = FakeSource::new(ProviderKind::GitHub);
    let cloudflare: Arc<dyn RunSource> = FakeSource::new(ProviderKind::Cloudflare);
    let mut registry: SourceRegistry = [first.clone(), cloudflare].into_iter().collect();
    assert_eq!(
        registry.providers(),
        [ProviderKind::GitHub, ProviderKind::Cloudflare]
    );
    let replaced = registry.register(second.clone()).unwrap();
    assert!(Arc::ptr_eq(&replaced, &first));
    assert!(Arc::ptr_eq(
        registry.get(ProviderKind::GitHub).unwrap(),
        &second
    ));
    assert_eq!(
        format!("{registry:?}"),
        "SourceRegistry { providers: [GitHub, Cloudflare] }"
    );
}

#[tokio::test(start_paused = true)]
async fn steady_polls_do_not_rewrite_the_cache_but_shutdown_does() {
    let t0 = Instant::now();
    let h = Harness::start();
    // The same finished run on every poll: only `last_success` moves.
    h.github.reply(
        "a",
        vec![Reply::Runs(vec![run("1", 0, BuildStatus::Passed)])],
    );
    h.handle.set_watches(vec![gh("w", "work", "a")]).unwrap();
    until(t0, 1).await;
    let saves = || h.cache.saves.lock().unwrap().len();
    assert_eq!(saves(), 2, "pending view, then the first result");
    // Polled every minute; four minutes of identical results: no writes.
    until(t0, 241).await;
    assert_eq!(h.github.calls_to("a").len(), 5);
    assert_eq!(saves(), 2);
    // Five minutes after the last write, the drifted ages are written once.
    until(t0, 301).await;
    assert_eq!(saves(), 3);
    until(t0, 361).await;
    assert_eq!(saves(), 3);
    // A status change is written at once.
    h.github.reply(
        "a",
        vec![Reply::Runs(vec![run("2", 10, BuildStatus::Running)])],
    );
    until(t0, 421).await;
    assert_eq!(saves(), 4);
    assert_eq!(
        h.cache.saves.lock().unwrap().last().unwrap()[0]
            .record
            .as_ref()
            .unwrap()
            .status,
        BuildStatus::Running
    );
    // Active now: polled every 15 s, still nothing new to write...
    until(t0, 451).await;
    assert_eq!(saves(), 4);
    // ...until the engine stops, which writes the held-back ages.
    h.handle.shutdown();
    h.task.await.unwrap();
    assert_eq!(h.cache.saves.lock().unwrap().len(), 5);
    let last = h.cache.saves.lock().unwrap().last().unwrap().clone();
    assert_eq!(last, h.handle.snapshots().to_vec());
}
