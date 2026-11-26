//! Window-level tests of the details panel with `egui_kittest`: real UI,
//! real controller and refresh engine, fake providers. In a single-window
//! harness the panel is an area under the strip, so it is queried like
//! the rest of the UI.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use buildrail_app::ui::BuildRailUi;
use buildrail_core::details::{JobSummary, RunDetails, RunRef, Trigger};
use buildrail_core::domain::{BuildStatus, RunId};
use chrono::TimeDelta;
use eframe::egui::accesskit::Role;
use eframe::egui::{Key, Modifiers, PointerButton};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use support::*;

const KEEVORO: &str =
    "Keevoro, GitHub, Failed, branch main, commit a7c412e, 2 minutes ago, failing job test";
const HEADING: &str = "Keevoro, Failed, GitHub · acme/keevoro · ci.yml";
const RUN: &str = "https://github.com/acme/keevoro/actions/runs/7";

fn keevoro_details() -> RunDetails {
    let started = t0() - TimeDelta::minutes(2) + TimeDelta::seconds(20);
    let mut details = RunDetails::new(RunRef {
        run_id: RunId::new("7"),
        attempt: 1,
    });
    details.commit_message = Some("Fix the flaky login test\n\nMore.".into());
    details.actor = Some("octocat".into());
    details.trigger = Trigger::Push;
    details.run_number = Some(562);
    details.started_at = Some(started);
    details.finished_at = Some(started + TimeDelta::seconds(95));
    let job = |name: &str, status, step: Option<&str>| JobSummary {
        name: name.into(),
        status,
        started_at: Some(started),
        finished_at: Some(started + TimeDelta::seconds(80)),
        url: Some(format!("{RUN}/job/{name}")),
        failing_step: step.map(str::to_owned),
    };
    details.jobs = Some(vec![
        job("lint", BuildStatus::Passed, None),
        job("test", BuildStatus::Failed, Some("cargo test")),
    ]);
    details.jobs_total = 2;
    details
}

fn fixture() -> Fixture {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro"), cf("Site", "site")]);
    f.world.github.set(
        "keevoro",
        Ok(vec![
            record("7", 1, BuildStatus::Failed, 2, "keevoro"),
            record("6", 1, BuildStatus::Passed, 90, "keevoro"),
            record("5", 1, BuildStatus::Failed, 200, "keevoro"),
        ]),
    );
    f.world.cloudflare.set(
        "site",
        Ok(vec![record("9", 1, BuildStatus::Deployed, 30, "site")]),
    );
    f.world.github_details.set("keevoro", Ok(keevoro_details()));
    f
}

/// Waits until *both* entries have their first result (they are fetched
/// concurrently).
fn loaded(f: &Fixture) -> Harness<'static, BuildRailUi> {
    let mut h = f.harness([900.0, 700.0]);
    step_until(&mut h, "both loaded", |h| {
        h.query_by_label(KEEVORO).is_some()
            && h.query_by_label_contains("Site, Cloudflare, Deployed")
                .is_some()
    });
    h
}

fn panel_open(h: &Harness<'_, BuildRailUi>) -> bool {
    h.query_by_role_and_label(Role::Button, "Close details")
        .is_some()
}

fn open_keevoro(h: &mut Harness<'_, BuildRailUi>) {
    h.get_by_label(KEEVORO).click();
    step_until(h, "details loaded", |h| {
        h.query_by_label("Trigger: Pushed by octocat").is_some()
    });
}

#[test]
fn a_click_opens_the_panel_with_its_sections_and_names() {
    let f = fixture();
    let mut h = loaded(&f);
    open_keevoro(&mut h);
    assert!(
        f.world.urls.opened().is_empty(),
        "a click no longer opens the browser"
    );
    assert_eq!(
        h.state().controller().details_open().map(|w| w.as_str()),
        Some("keevoro")
    );
    // Header, facts, the failed job first with its step, history, actions.
    assert!(h.query_by_role_and_label(Role::Heading, HEADING).is_some());
    for name in [
        "Branch: main",
        "Commit: a7c412e · Fix the flaky login test",
        "Run: #562",
        "Duration: Took 1 min 35 s",
        "2 of the last 3 runs failed.",
    ] {
        assert!(h.query_by_label(name).is_some(), "{name}");
    }
    assert!(h.query_by_label_contains("Started: 1 min ago, ").is_some());
    let jobs: Vec<String> = h
        .query_all_by_role(Role::Link)
        .filter_map(|n| {
            use egui_kittest::kittest::NodeT;
            n.accesskit_node().label()
        })
        .collect();
    assert_eq!(
        jobs[..2],
        [
            "test, Failed, 1 min 20 s, failed at step cargo test, opens the job".to_owned(),
            "lint, Passed, 1 min 20 s, opens the job".to_owned(),
        ]
    );
    assert!(jobs
        .iter()
        .any(|j| j.starts_with("Failed, 2 minutes ago, the run shown above")));
    for button in ["Open run", "Refresh now", "Copy link", "Edit in Settings"] {
        assert!(
            h.query_by_role_and_label(Role::Button, button).is_some(),
            "{button}"
        );
    }
    // One details fetch for the open.
    assert_eq!(f.world.github_details.calls_for("keevoro"), 1);

    // Rows open their own pages; Open run opens the run.
    h.get_by_label_contains("test, Failed, 1 min 20 s").click();
    h.step();
    h.get_by_role_and_label(Role::Button, "Open run").click();
    h.step();
    assert_eq!(
        f.world.urls.opened(),
        vec![format!("{RUN}/job/test"), RUN.to_owned()]
    );
}

#[test]
fn the_same_entry_escape_the_close_button_or_the_ground_close_it() {
    let f = fixture();
    let mut h = loaded(&f);
    let closed = |h: &Harness<'_, BuildRailUi>| !panel_open(h);

    open_keevoro(&mut h);
    h.get_by_label(KEEVORO).click();
    step_until(&mut h, "closed by a second click", closed);
    assert_eq!(h.state().controller().details_open(), None);

    open_keevoro(&mut h);
    h.key_press(Key::Escape);
    step_until(&mut h, "closed by Escape", closed);

    open_keevoro(&mut h);
    h.get_by_role_and_label(Role::Button, "Close details")
        .click();
    step_until(&mut h, "closed by its button", closed);

    open_keevoro(&mut h);
    // The strip's own ground, right of the entries.
    h.event(eframe::egui::Event::PointerMoved(eframe::egui::pos2(
        850.0, 12.0,
    )));
    for pressed in [true, false] {
        h.event(eframe::egui::Event::PointerButton {
            pos: eframe::egui::pos2(850.0, 12.0),
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        });
    }
    step_until(&mut h, "closed by the ground", closed);
    // Reopening shows what was fetched before, without fetching again.
    open_keevoro(&mut h);
    assert_eq!(f.world.github_details.calls_for("keevoro"), 1);
}

#[test]
fn clicking_another_entry_switches_the_panel() {
    let f = fixture();
    let mut h = loaded(&f);
    open_keevoro(&mut h);
    h.get_by_label_contains("Site, Cloudflare, Deployed")
        .click();
    step_until(&mut h, "site", |h| {
        h.query_by_role_and_label(Role::Heading, "Site, Deployed, Cloudflare · Worker site")
            .is_some()
    });
    assert!(h.query_by_role_and_label(Role::Heading, HEADING).is_none());
    assert_eq!(
        h.state().controller().details_open().map(|w| w.as_str()),
        Some("site")
    );
    // Cloudflare lists no jobs.
    assert!(h.query_by_label("Jobs").is_none());
}

#[test]
fn double_click_middle_click_and_ctrl_enter_open_the_run_without_the_panel() {
    let f = fixture();
    // Frames a sixtieth of a second apart, so two clicks make a
    // double-click.
    let mut h = Harness::builder()
        .with_size([900.0, 700.0])
        .with_step_dt(1.0 / 60.0)
        .renderer(CpuRenderer::default())
        .build_ui_state(root_ui, f.ui());
    step_until(&mut h, "both loaded", |h| {
        h.query_by_label(KEEVORO).is_some()
            && h.query_by_label_contains("Site, Cloudflare, Deployed")
                .is_some()
    });

    // Two clicks in quick succession: the first shows the panel, the
    // second (a double-click) opens the run and closes it.
    h.get_by_label(KEEVORO).click();
    h.step();
    assert!(h.state().controller().details_open().is_some());
    h.get_by_label(KEEVORO).click();
    step_until(&mut h, "opened", |_| f.world.urls.opened().len() == 1);
    step_until(&mut h, "no panel", |h| !panel_open(h));

    h.get_by_label(KEEVORO).click_button(PointerButton::Middle);
    step_until(&mut h, "opened again", |_| f.world.urls.opened().len() == 2);
    assert!(!panel_open(&h));

    h.get_by_label(KEEVORO).focus();
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    step_until(&mut h, "opened a third time", |_| {
        f.world.urls.opened().len() == 3
    });
    assert!(!panel_open(&h));
    assert!(f.world.urls.opened().iter().all(|u| u == RUN));
}

#[test]
fn the_keyboard_reaches_and_activates_open_run() {
    let f = fixture();
    let mut h = loaded(&f);
    h.get_by_label(KEEVORO).focus();
    h.step();
    h.key_press(Key::Enter);
    step_until(&mut h, "panel with focus on Open run", |h| {
        h.query_by_role_and_label(Role::Button, "Open run")
            .is_some_and(|b| b.is_focused())
    });
    // Tab moves on to the quiet buttons and Shift+Tab back.
    h.key_press(Key::Tab);
    h.step();
    h.step();
    assert!(h
        .get_by_role_and_label(Role::Button, "Refresh now")
        .is_focused());
    h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
    h.step();
    h.step();
    assert!(h
        .get_by_role_and_label(Role::Button, "Open run")
        .is_focused());
    h.key_press(Key::Enter);
    h.step();
    assert_eq!(f.world.urls.opened(), vec![RUN.to_owned()]);
}

#[test]
fn the_hover_card_never_covers_the_entrys_own_panel() {
    let f = fixture();
    let mut h = loaded(&f);
    let hint = "Click or press Enter for details. Double-click to open the run.";
    h.get_by_label(KEEVORO).hover();
    h.step();
    h.step();
    assert!(h.query_by_label(hint).is_some(), "the card shows on hover");
    open_keevoro(&mut h);
    h.get_by_label(KEEVORO).hover();
    h.step();
    h.step();
    assert!(h.query_by_label(hint).is_none(), "not over its own panel");
    // Another entry still shows its card.
    h.get_by_label_contains("Site, Cloudflare, Deployed")
        .hover();
    h.step();
    h.step();
    assert!(h.query_by_label(hint).is_some());
}

#[test]
fn refresh_copy_and_edit_in_settings() {
    let f = fixture();
    let mut h = loaded(&f);
    open_keevoro(&mut h);
    h.get_by_role_and_label(Role::Button, "Refresh now").click();
    step_until(&mut h, "fetched again", |_| {
        f.world.github_details.calls_for("keevoro") == 2
    });
    step_until(&mut h, "settled", |h| {
        h.query_by_role_and_label(Role::Button, "Refresh now")
            .is_some()
    });

    h.get_by_role_and_label(Role::Button, "Copy link").click();
    step_until(&mut h, "copied", |h| {
        h.query_by_role_and_label(Role::Button, "Link copied")
            .is_some()
    });
    assert!(h.state().strip_view().panel_state().copied);

    h.get_by_role_and_label(Role::Button, "Edit in Settings")
        .click();
    step_until(&mut h, "settings", |h| h.state().is_settings_open());
    assert_eq!(h.state().controller().details_open(), None);
    assert_eq!(
        h.state().settings().highlighted().map(|w| w.as_str()),
        Some("keevoro")
    );
    assert_eq!(
        h.state().settings().page,
        buildrail_app::ui::settings::Page::Builds
    );
}

#[test]
fn a_failed_load_keeps_the_facts_and_try_again_recovers() {
    let f = fixture();
    f.world.github_details.set(
        "keevoro",
        Err(buildrail_core::ports::ProviderError::Network("dns".into())),
    );
    let mut h = loaded(&f);
    h.get_by_label(KEEVORO).click();
    step_until(&mut h, "failed", |h| {
        h.query_by_label("Details could not be loaded: GitHub could not be reached.")
            .is_some()
    });
    assert!(h.query_by_label("Branch: main").is_some());
    f.world.github_details.set("keevoro", Ok(keevoro_details()));
    h.get_by_role_and_label(Role::Button, "Try again").click();
    step_until(&mut h, "recovered", |h| {
        h.query_by_label("Trigger: Pushed by octocat").is_some()
    });
    assert!(h
        .query_by_label_contains("Details could not be loaded")
        .is_none());
}

#[test]
fn an_idle_open_panel_paints_only_when_its_words_change() {
    use buildrail_app::presentation::repaint::CHANGE_SLACK;
    use buildrail_app::presentation::RepaintPlanner;
    use buildrail_core::ports::Clock;
    use std::time::Duration;

    let f = fixture();
    let mut h = loaded(&f);
    open_keevoro(&mut h);
    for _ in 0..3 {
        h.step();
    }
    // The strip's earliest change is Keevoro's "2 minutes ago" at 60 s;
    // the panel's start ("1 min ago", 100 s old) changes at 20 s.
    assert_eq!(
        h.state().next_repaint(),
        Some(Duration::from_secs(20) + CHANGE_SLACK)
    );

    // Ten idle minutes: a frame exactly when some words change, never
    // sooner, and never a details fetch.
    let start = f.world.clock.now();
    let frames_before = h.state().frame_counter().total();
    let mut frames = 0;
    while f.world.clock.now() - start < TimeDelta::minutes(10) {
        let now = f.world.clock.now();
        let strip = RepaintPlanner::next_text_change(&h.state().controller().strip(), now);
        let panel = h
            .state_mut()
            .controller_mut()
            .details_panel()
            .and_then(|d| d.next_text_change(now));
        let next = [strip, panel].into_iter().flatten().min().unwrap();
        let delay = h.state().next_repaint().unwrap();
        assert!(
            delay >= (next - now).to_std().unwrap(),
            "{delay:?} too soon"
        );
        f.world.clock.advance(TimeDelta::from_std(delay).unwrap());
        h.step();
        frames += 1;
    }
    assert!(frames <= 21, "{frames} frames in ten minutes");
    assert_eq!(h.state().frame_counter().total() - frames_before, frames);
    assert_eq!(f.world.github_details.calls_for("keevoro"), 1);
    assert!(panel_open(&h));
}
