//! The details use-case: fetched on demand, once per run attempt and
//! status, again for a running run that moved on, and on Refresh now.

use buildrail_core::details::{RunDetails, RunRef, Trigger};

use super::*;
use crate::app::DetailsSources;
use crate::presentation::{DetailsModel, PanelAction};

fn watch_id(t: &T) -> WatchId {
    t.app.profile().watches()[0].id.clone()
}

fn named(run: &str, title: &str) -> RunDetails {
    let mut details = RunDetails::new(RunRef {
        run_id: RunId::new(run),
        attempt: 1,
    });
    details.commit_message = Some(title.into());
    details.trigger = Trigger::Push;
    details.actor = Some("octocat".into());
    details
}

/// Pins Keevoro with `status` and waits until the strip shows it.
fn pinned(status: BuildStatus) -> T {
    let mut t = T::new();
    t.connect(ProviderKind::GitHub, "work", "tok");
    t.world.github.set("keevoro", Ok(vec![record("1", status)]));
    t.pin(gh_draft("Keevoro", "work", "keevoro")).unwrap();
    t.wait_for("loaded", |s| {
        s.entries
            .first()
            .is_some_and(|e| e.status_text == status.label())
    });
    t
}

fn panel(t: &mut T) -> DetailsModel {
    let model = t.app.details_panel().expect("a panel is open");
    t.app.settle(WAIT);
    model
}

fn trigger(model: &DetailsModel) -> Option<String> {
    model
        .run
        .iter()
        .find(|f| f.label == "Trigger")
        .map(|f| f.value.clone())
}

#[test]
fn details_are_fetched_once_per_open_and_kept_for_the_session() {
    let mut t = pinned(BuildStatus::Failed);
    t.world
        .github_details
        .set("keevoro", Ok(named("1", "Fix the login test")));
    let id = watch_id(&t);
    assert!(t.app.details_panel().is_none(), "nothing open");
    t.app.open_details(&id);
    assert_eq!(t.app.details_open(), Some(&id));
    t.app.settle(WAIT);
    let model = panel(&mut t);
    assert_eq!(trigger(&model).as_deref(), Some("Pushed by octocat"));
    assert!(model.recent.is_none(), "one run only: nothing to compare");
    assert_eq!(t.world.github_details.calls_for("keevoro"), 1);
    let (_, run) = &t.world.github_details.calls()[0];
    assert_eq!(run.run_id.as_str(), "1");

    // Drawn again and again, closed and opened: never fetched again.
    for _ in 0..3 {
        panel(&mut t);
    }
    t.app.close_details();
    assert!(t.app.details_panel().is_none());
    t.app.open_details(&id);
    let model = panel(&mut t);
    assert_eq!(
        trigger(&model).as_deref(),
        Some("Pushed by octocat"),
        "at once"
    );
    assert_eq!(t.world.github_details.calls_for("keevoro"), 1);

    // Refresh now re-checks and fetches again.
    t.app.refresh_details();
    t.app.settle(WAIT);
    assert_eq!(t.world.github_details.calls_for("keevoro"), 2);
}

#[test]
fn a_running_run_is_fetched_again_when_its_snapshot_changes() {
    let mut t = pinned(BuildStatus::Running);
    let id = watch_id(&t);
    t.app.open_details(&id);
    t.app.settle(WAIT);
    // Polls that confirm the same state do not fetch again.
    std::thread::sleep(Duration::from_millis(200));
    panel(&mut t);
    assert_eq!(t.world.github_details.calls_for("keevoro"), 1);

    let mut progressed = record("1", BuildStatus::Running);
    progressed.updated_at += chrono::TimeDelta::seconds(30);
    let moved = progressed.updated_at;
    t.world.github.set("keevoro", Ok(vec![progressed]));
    let deadline = Instant::now() + WAIT;
    while t.app.refresh_handle().snapshots()[0]
        .record
        .as_ref()
        .is_none_or(|r| r.updated_at != moved)
    {
        assert!(Instant::now() < deadline, "the strip never moved on");
        std::thread::sleep(Duration::from_millis(10));
    }
    panel(&mut t);
    assert_eq!(t.world.github_details.calls_for("keevoro"), 2);
}

#[test]
fn a_failure_keeps_the_basic_facts_and_try_again_fetches() {
    let mut t = pinned(BuildStatus::Failed);
    t.world
        .github_details
        .set("keevoro", Err(ProviderError::Network("dns".into())));
    let id = watch_id(&t);
    t.app.open_details(&id);
    t.app.settle(WAIT);
    let model = panel(&mut t);
    let line = model.load_line.as_ref().unwrap();
    assert!(line.failed);
    assert_eq!(
        line.text,
        "Details could not be loaded: GitHub could not be reached."
    );
    assert!(model.run.iter().any(|f| f.label == "Branch"));
    assert_eq!(model.primary.label, "Open run");
    // A failure does not retry by itself.
    panel(&mut t);
    assert_eq!(t.world.github_details.calls_for("keevoro"), 1);

    t.world
        .github_details
        .set("keevoro", Ok(named("1", "Fix it")));
    t.app.retry_details();
    t.app.settle(WAIT);
    let model = panel(&mut t);
    assert_eq!(model.load_line, None);
    assert_eq!(trigger(&model).as_deref(), Some("Pushed by octocat"));
}

#[test]
fn switching_builds_ignores_the_answer_meant_for_the_old_panel() {
    let mut t = T::new();
    t.connect(ProviderKind::GitHub, "work", "tok");
    t.world
        .github
        .set("keevoro", Ok(vec![record("1", BuildStatus::Failed)]));
    t.world
        .github
        .set("billing", Ok(vec![record("2", BuildStatus::Passed)]));
    let a = t.pin(gh_draft("Keevoro", "work", "keevoro")).unwrap();
    let b = t.pin(gh_draft("Billing", "work", "billing")).unwrap();
    t.wait_for("both", |s| {
        s.entries.len() == 2 && s.entries.iter().all(|e| e.status_text != "Checking…")
    });
    t.world
        .github_details
        .set("keevoro", Ok(named("1", "Keevoro's commit")));
    t.world
        .github_details
        .set("billing", Ok(named("2", "Billing's commit")));
    t.world.github_details.delay(Duration::from_millis(150));
    t.app.open_details(&a);
    t.app.open_details(&b);
    let model = t.app.details_panel().unwrap();
    assert_eq!(model.label, "Billing");
    t.app.settle(WAIT);
    let model = panel(&mut t);
    assert_eq!(model.label, "Billing");
    let commit = model.run.iter().find(|f| f.label == "Commit").unwrap();
    assert!(commit.value.contains("Billing's commit"), "{commit:?}");
    // Keevoro's answer was kept for its own panel.
    t.app.open_details(&a);
    let model = panel(&mut t);
    let commit = model.run.iter().find(|f| f.label == "Commit").unwrap();
    assert!(commit.value.contains("Keevoro's commit"), "{commit:?}");
    assert_eq!(t.world.github_details.calls_for("keevoro"), 1);
    assert_eq!(t.world.github_details.calls_for("billing"), 1);

    // Removing the build closes its panel.
    t.saved(Command::Edit {
        ticket: Ticket::next(),
        edit: ProfileEdit::Remove { id: a },
    })
    .unwrap();
    assert!(t.app.details_panel().is_none());
    assert_eq!(t.app.details_open(), None);
}

#[test]
fn reconnect_needed_fetches_nothing_and_a_missing_source_says_so() {
    let mut t = T::new();
    t.connect(ProviderKind::GitHub, "work", "tok");
    t.world.github.set(
        "keevoro",
        Err(ProviderError::AccessDenied {
            provider: ProviderKind::GitHub,
        }),
    );
    let id = t.pin(gh_draft("Keevoro", "work", "keevoro")).unwrap();
    t.wait_for("reconnect", |s| {
        s.entries.first().is_some_and(|e| e.needs_reconnect)
    });
    t.app.open_details(&id);
    let model = panel(&mut t);
    assert_eq!(model.primary.action, PanelAction::Reconnect);
    assert_eq!(t.world.github_details.calls().len(), 0);

    // A build of BuildRail without a details source for the provider.
    let mut services = t.world.services(&t.paths);
    services.details = DetailsSources::new();
    t.world
        .github
        .set("keevoro", Ok(vec![record("1", BuildStatus::Passed)]));
    t.app = AppController::start(
        services,
        Arc::new(t.paths.snapshot_cache()),
        fast_policy(),
        t.runtime.handle().clone(),
    );
    t.wait_for("passed", |s| {
        s.entries.first().is_some_and(|e| e.status_text == "Passed")
    });
    t.app.open_details(&id);
    t.app.settle(WAIT);
    let model = panel(&mut t);
    assert_eq!(
        model.load_line.unwrap().text,
        "Details could not be loaded: GitHub details are not available in this build."
    );
}

#[test]
fn a_missing_token_is_reported_without_asking_the_provider() {
    let mut t = pinned(BuildStatus::Passed);
    let id = watch_id(&t);
    t.world
        .credentials
        .delete(&ConnectionAlias::new("work"))
        .unwrap();
    t.app.open_details(&id);
    t.app.settle(WAIT);
    // The engine may notice first and ask for a reconnect; either way the
    // provider is never asked for details without a token.
    let model = panel(&mut t);
    assert_eq!(t.world.github_details.calls().len(), 0);
    if let Some(line) = model.load_line {
        assert_eq!(
            line.text,
            "Details could not be loaded: there is no token for the “work” connection on this device."
        );
    }
}
