//! The acceptance scenario end to end, without a window: fake providers
//! drive a build queued → running → failed through the real refresh
//! engine (tokio time paused), the strip model follows, the run opens,
//! the network drops (stale state with its age), the machine sleeps and
//! wakes (immediate recovery), and the profile travels from machine A to
//! machine B through export, alias mapping and import.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use buildrail_app::app::AppController;
use buildrail_app::presentation::{IconKind, StripEntry, StripModel};
use buildrail_app::settings::import::{export_text, ImportFlow, ImportInput, ImportStep};
use buildrail_app::settings::SettingsContext;
use buildrail_core::domain::{BuildStatus, ConnectionAlias, ProviderKind, Tone};
use buildrail_core::ports::{Clock, CredentialWriter, ProviderError, Secret};
use buildrail_core::refresh::RefreshPolicy;
use buildrail_profile::{AppPaths, DevicePrefs, LocalConnection, Profile};
use chrono::{DateTime, TimeDelta, Utc};
use support::*;
use tokio::time::Instant;

/// Wall clock = start + tokio's (paused) monotonic time + a manual jump.
struct SleepyClock {
    base: DateTime<Utc>,
    start: Instant,
    jump: Mutex<TimeDelta>,
}

impl SleepyClock {
    fn new() -> Self {
        Self {
            base: t0(),
            start: Instant::now(),
            jump: Mutex::new(TimeDelta::zero()),
        }
    }

    /// The machine slept for `by`: wall time moves, monotonic time does not.
    fn sleep_machine(&self, by: TimeDelta) {
        *self.jump.lock().unwrap() += by;
    }
}

impl Clock for SleepyClock {
    fn now(&self) -> DateTime<Utc> {
        self.base + TimeDelta::from_std(self.start.elapsed()).unwrap() + *self.jump.lock().unwrap()
    }
}

/// Lets (paused) time run until `check` holds; returns how long it took.
async fn until(app: &AppController, what: &str, check: impl Fn(&StripModel) -> bool) -> Duration {
    let started = Instant::now();
    for _ in 0..2_000 {
        if check(&app.strip()) {
            return started.elapsed();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}: {:?}", app.strip());
}

fn keevoro(strip: &StripModel) -> &StripEntry {
    &strip.entries[0]
}

fn machine(fixture: &Fixture, aliases: &[(&str, ProviderKind)]) {
    let mut prefs = DevicePrefs::default();
    for (alias, provider) in aliases {
        prefs.connections.insert(
            ConnectionAlias::new(*alias),
            LocalConnection {
                provider: *provider,
                display_name: format!("{alias}-owner"),
            },
        );
        fixture
            .world
            .credentials
            .write(
                &ConnectionAlias::new(*alias),
                &Secret::new(format!("secret-token-{alias}")),
            )
            .unwrap();
    }
    fixture.paths.device_prefs_store().save(&prefs).unwrap();
}

fn start(fixture: &Fixture, clock: Arc<SleepyClock>) -> AppController {
    let mut services = fixture.world.services(&fixture.paths);
    services.clock = clock;
    AppController::start(
        services,
        Arc::new(fixture.paths.snapshot_cache()),
        RefreshPolicy::default(),
        tokio::runtime::Handle::current(),
    )
}

#[tokio::test(start_paused = true)]
async fn acceptance_scenario() {
    // ------------------------------------------------------------ machine A
    let a = Fixture::for_async();
    machine(
        &a,
        &[
            ("work", ProviderKind::GitHub),
            ("cf", ProviderKind::Cloudflare),
        ],
    );
    a.paths
        .profile_store()
        .save(&Profile::from_drafts(&[gh("Keevoro", "keevoro"), cf("Site", "site")]).unwrap())
        .unwrap();
    let clock = Arc::new(SleepyClock::new());
    a.world.github.set(
        "keevoro",
        Ok(vec![record("10", 1, BuildStatus::Queued, 0, "keevoro")]),
    );
    a.world.cloudflare.set(
        "site",
        Ok(vec![record("3", 1, BuildStatus::Deployed, 60, "site")]),
    );
    let app = start(&a, clock.clone());

    // Queued → running → failed, polled at the 15 s "active" cadence.
    until(&app, "queued", |s| keevoro(s).status_text == "Queued").await;
    assert_eq!(keevoro(&app.strip()).tone, Tone::Amber);
    assert_eq!(keevoro(&app.strip()).icon, IconKind::Queued);
    a.world.github.set(
        "keevoro",
        Ok(vec![record("10", 1, BuildStatus::Running, 0, "keevoro")]),
    );
    let took = until(&app, "running", |s| keevoro(s).status_text == "Building").await;
    assert!(took <= Duration::from_secs(16), "active cadence: {took:?}");
    assert_eq!(keevoro(&app.strip()).icon, IconKind::Spinner);
    let mut failed = record("10", 1, BuildStatus::Failed, 0, "keevoro");
    failed.updated_at = clock.now();
    a.world.github.set("keevoro", Ok(vec![failed]));
    until(&app, "failed", |s| keevoro(s).status_text == "Failed").await;
    let strip = app.strip();
    assert_eq!(strip.entries[0].text(), "Keevoro · Failed");
    assert!(strip.entries[0]
        .accessible_name
        .contains("failing job test"));
    assert_eq!(strip.entries[1].text(), "Site · Deployed");
    assert_eq!(strip.failure_count(), 1);

    // Activate: the provider run opens.
    let url = strip.entries[0].url.clone().unwrap();
    app.open_url(&url).unwrap();
    assert_eq!(a.world.urls.opened(), vec![url]);

    // Network loss: the cached failure stays, marked offline with its age.
    a.world
        .github
        .set("keevoro", Err(ProviderError::Network("unreachable".into())));
    until(&app, "offline", |s| {
        keevoro(s)
            .note
            .as_deref()
            .is_some_and(|n| n.ends_with("· offline"))
    })
    .await;
    tokio::time::sleep(Duration::from_secs(5 * 60)).await;
    let entry = keevoro(&app.strip()).clone();
    assert_eq!(entry.status_text, "Failed", "cached state still shown");
    assert!(entry.stale);
    let note = entry.note.unwrap();
    assert!(note.ends_with(" min ago · offline"), "{note}");
    assert!(
        entry.accessible_name.contains("offline, last updated"),
        "{}",
        entry.accessible_name
    );

    // Sleep for two hours with the network back; on wake the engine
    // refreshes at once instead of waiting out its failure backoff.
    let mut fixed = record("11", 1, BuildStatus::Passed, 0, "keevoro");
    fixed.created_at = clock.now();
    fixed.updated_at = clock.now();
    a.world.github.set("keevoro", Ok(vec![fixed]));
    // Let any backoff grow first, so recovery cannot be a scheduled retry.
    tokio::time::sleep(Duration::from_secs(1)).await;
    clock.sleep_machine(TimeDelta::hours(2));
    let took = until(&app, "recovered after wake", |s| {
        keevoro(s).status_text == "Passed"
    })
    .await;
    assert!(
        took <= Duration::from_secs(6),
        "wake recovery took {took:?}"
    );
    assert!(!keevoro(&app.strip()).stale);
    assert_eq!(keevoro(&app.strip()).note, None);

    // ------------------------------------------- export A → import on B
    let exported = export_text(app.profile());
    assert!(
        !exported.contains("secret-token"),
        "no credentials in exports"
    );
    assert!(exported.contains("connection = \"work\""));

    let b = Fixture::for_async();
    machine(
        &b,
        &[
            ("me-github", ProviderKind::GitHub),
            ("me-cf", ProviderKind::Cloudflare),
        ],
    );
    let mut app_b = start(&b, Arc::new(SleepyClock::new()));
    let mut flow = ImportFlow::new();
    let ctx = |app: &AppController| -> (Profile, buildrail_profile::LocalConnections) {
        (app.profile().clone(), app.prefs().connections.clone())
    };
    let (profile, connections) = ctx(&app_b);
    let c = SettingsContext {
        profile: &profile,
        connections: &connections,
    };
    flow.input(ImportInput::SetText(exported), c);
    flow.input(ImportInput::Load, c);
    assert_eq!(flow.step(), ImportStep::Mapping);
    // One connection per provider on B: both suggested automatically.
    for required in flow.required() {
        assert!(flow.mapped(required).is_some(), "{required:?}");
    }
    flow.input(ImportInput::Continue, c);
    assert_eq!(flow.step(), ImportStep::Review);
    for command in flow.input(ImportInput::Confirm, c) {
        app_b.execute(command);
    }
    for event in app_b.poll_events() {
        flow.on_event(&event);
    }
    assert_eq!(flow.step(), ImportStep::Done);

    let on_b = AppPaths::profile_store(&b.paths).load().unwrap();
    let ids_a: Vec<_> = app
        .profile()
        .watches()
        .iter()
        .map(|w| (&w.id, &w.label))
        .collect();
    let ids_b: Vec<_> = on_b.watches().iter().map(|w| (&w.id, &w.label)).collect();
    assert_eq!(ids_a, ids_b, "same watches, same order");
    let aliases: Vec<&str> = on_b
        .watches()
        .iter()
        .map(|w| w.connection.as_str())
        .collect();
    assert_eq!(aliases, ["me-github", "me-cf"]);
    // B's strip picks the imported watches up with B's own credentials.
    b.world.github.set(
        "keevoro",
        Ok(vec![record("11", 1, BuildStatus::Passed, 0, "keevoro")]),
    );
    until(&app_b, "B refreshed", |s| {
        s.entries.len() == 2 && keevoro(s).status_text == "Passed"
    })
    .await;
    app.shutdown();
    app_b.shutdown();
}
