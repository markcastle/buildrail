//! Window-level tests of self-update with `egui_kittest`: the strip's
//! Update chip and its card, Settings › About, and that the background
//! check loop paints nothing unless what is shown changes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use buildrail_app::fakes::ScriptedUpdateService;
use buildrail_app::presentation::update::RELEASES_PAGE;
use buildrail_app::ui::settings::Page;
use buildrail_app::ui::BuildRailUi;
use buildrail_core::domain::BuildStatus;
use buildrail_core::refresh::RefreshPolicy;
use buildrail_update::UpdateStatus;
use eframe::egui;
use eframe::egui::accesskit::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use support::*;

const CHIP: &str = "BuildRail 0.3.0 is available. Install now";
const NOTES: &str = "https://github.com/markcastle/buildrail/releases/tag/v0.3.0";

fn two_builds() -> Fixture {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro"), cf("Site", "site")]);
    f.world.github.set(
        "keevoro",
        Ok(vec![record("7", 1, BuildStatus::Failed, 2, "keevoro")]),
    );
    f.world.cloudflare.set(
        "site",
        Ok(vec![record("9", 1, BuildStatus::Deployed, 30, "site")]),
    );
    f
}

fn loaded(h: &Harness<'_, BuildRailUi>) -> bool {
    h.query_by_label_contains("Keevoro, GitHub, Failed")
        .is_some()
        && h.query_by_label_contains("Site, Cloudflare, Deployed")
            .is_some()
}

/// A strip with two builds loaded and 0.3.0 found by "Check for updates".
fn offered(f: &Fixture, size: [f32; 2]) -> Harness<'static, BuildRailUi> {
    f.world
        .updates
        .answer(ScriptedUpdateService::available("0.3.0"));
    let mut h = f.harness(size);
    step_until(&mut h, "loaded", loaded);
    assert!(h.query_by_label(CHIP).is_none(), "nothing until a check");
    h.state().controller().check_for_updates();
    step_until(&mut h, "chip", |h| {
        h.query_by_label(CHIP).is_some() && loaded(h)
    });
    h
}

fn settle(h: &mut Harness<'_, BuildRailUi>) {
    for _ in 0..4 {
        h.step();
    }
}

#[test]
fn the_chip_sits_before_the_wells_and_installs() {
    let f = two_builds();
    let mut h = offered(&f, [900.0, 200.0]);
    let order = labels_left_to_right(&h, Role::Button);
    let at = |name: &str| order.iter().position(|l| l == name).unwrap();
    assert_eq!(at(CHIP) + 1, at("Add build"), "{order:?}");
    assert_eq!(at("Add build") + 1, at("Settings"));
    assert!(at(CHIP) > order.iter().position(|l| l.starts_with("Site,")).unwrap());

    h.get_by_label(CHIP).click();
    step_until(&mut h, "restart", |_| {
        f.world.updates.calls().iter().any(|c| c == "restart")
    });
    assert_eq!(
        f.world.updates.calls(),
        ["check", "install 0.3.0", "restart"]
    );
}

#[test]
fn hover_shows_the_card_and_not_now_hides_the_offer_until_restart() {
    let f = two_builds();
    let mut h = offered(&f, [900.0, 200.0]);
    assert!(h.query_by_label("What's new").is_none());
    h.get_by_label(CHIP).hover();
    settle(&mut h);
    assert!(h.state().strip_view().is_update_card_open());
    assert!(h.query_by_label("BuildRail 0.3.0 is available.").is_some());
    for name in ["Install now", "What's new", "Not now"] {
        let node = h.get_by_label(name);
        assert_eq!(node.accesskit_node().role(), Role::Button, "{name}");
    }

    // What's new opens the release page; the app explains nothing itself.
    h.get_by_label("What's new").click();
    settle(&mut h);
    assert_eq!(f.world.urls.opened(), [NOTES.to_owned()]);

    // Pointer elsewhere on the strip: the card closes.
    h.get_by_label_contains("Keevoro, GitHub").hover();
    settle(&mut h);
    assert!(!h.state().strip_view().is_update_card_open());

    h.get_by_label(CHIP).hover();
    settle(&mut h);
    h.get_by_label("Not now").click();
    settle(&mut h);
    assert!(h.query_by_label(CHIP).is_none());
    assert!(h.query_by_label("Not now").is_none());
    // The next scheduled answer for the same version stays quiet.
    h.state().controller().check_for_updates();
    step_until(&mut h, "answered", |h| {
        !h.state().controller().update_state().checking
    });
    assert!(h.query_by_label(CHIP).is_none());
    assert!(!f
        .world
        .updates
        .calls()
        .iter()
        .any(|c| c.starts_with("install")));
}

#[test]
fn install_now_on_the_card_and_a_failure_shown_in_one_line() {
    let f = two_builds();
    f.world
        .updates
        .fail_install(buildrail_update::InstallError::Decompress);
    let mut h = offered(&f, [900.0, 200.0]);
    h.get_by_label(CHIP).focus();
    settle(&mut h);
    assert!(
        h.state().strip_view().is_update_card_open(),
        "focus opens it"
    );
    h.get_by_label("Install now").click();
    let error = "Update not installed: the download could not be decompressed.";
    step_until(&mut h, "error", |h| {
        h.state()
            .controller()
            .update_state()
            .install_error
            .is_some()
    });
    h.get_by_label(CHIP).hover();
    settle(&mut h);
    assert!(h.query_by_label(error).is_some());
    assert!(h.query_by_label(CHIP).is_some(), "still offered");
    assert!(!f.world.updates.calls().iter().any(|c| c == "restart"));
}

#[test]
fn the_chip_only_pushes_builds_into_the_overflow_when_they_do_not_fit() {
    let f = two_builds();
    let mut h = offered(&f, [900.0, 200.0]);
    assert!(h.query_by_label_contains("more build").is_none());
    for width in [700.0, 560.0, 480.0, 420.0, 360.0] {
        h.set_size(egui::vec2(width, 200.0));
        settle(&mut h);
        let chip = h.get_by_label(CHIP).rect();
        assert!(chip.max.x <= width, "chip inside at {width}");
        let strip_buttons: Vec<_> = h
            .query_all_by_role(Role::Button)
            .filter(|n| n.rect().min.y < 24.0)
            .filter(|n| {
                let label = n.accesskit_node().label().unwrap_or_default();
                label.contains(", GitHub")
                    || label.contains(", Cloudflare")
                    || label.contains("more build")
            })
            .map(|n| n.rect())
            .collect();
        for rect in strip_buttons {
            assert!(rect.max.x <= chip.min.x, "no overlap at {width}");
        }
    }
    // Narrow enough, a build moves into the overflow next to the chip.
    assert!(h.query_by_label_contains("more build").is_some());
}

#[test]
fn about_answers_check_for_updates_in_one_line() {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro")]);
    let mut app = f.ui();
    app.settings_mut().page = Page::About;
    let mut h = Harness::builder()
        .with_size([880.0, 640.0])
        .renderer(CpuRenderer::default())
        .build_ui_state(
            |ui, app: &mut BuildRailUi| {
                let full = ui.ctx().content_rect();
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full));
                app.settings_ui(&mut child);
            },
            app,
        );
    settle(&mut h);
    let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
    assert!(h.query_by_label(&version).is_some());
    assert!(h
        .query_by_label("BuildRail looks for a new release once an hour.")
        .is_some());
    assert!(h.query_all_by_label("Update automatically").count() >= 1);
    assert!(
        !h.state().controller().prefs().update_automatically,
        "off by default"
    );

    let ask = |h: &mut Harness<'_, BuildRailUi>, answer: UpdateStatus, line: &str| {
        f.world.updates.answer(answer);
        h.get_by_label("Check for updates").click();
        step_until(h, line, |h| h.query_by_label(line).is_some());
    };
    ask(
        &mut h,
        UpdateStatus::UpToDate {
            current: buildrail_update::Version::new(0, 2, 0),
        },
        "Up to date.",
    );
    ask(
        &mut h,
        ScriptedUpdateService::failed(),
        "Update check failed: the server answered HTTP 503.",
    );
    ask(
        &mut h,
        ScriptedUpdateService::available("0.3.0"),
        "BuildRail 0.3.0 is available.",
    );
    h.get_by_label("What's new").click();
    settle(&mut h);
    assert_eq!(f.world.urls.opened(), [NOTES.to_owned()]);
    assert!(h.query_by_label("Check for updates").is_none());

    // Automatic updates: saved, off again by default on a new device.
    h.query_all_by_label("Update automatically")
        .find(|n| n.accesskit_node().role() != Role::Label)
        .unwrap()
        .click();
    settle(&mut h);
    assert!(h.state().controller().prefs().update_automatically);
    assert!(
        f.paths
            .device_prefs_store()
            .load()
            .unwrap()
            .update_automatically
    );

    // Install now from Settings uses the same flow.
    f.world.updates.delay_install(Duration::from_millis(300));
    h.get_by_label("Install now").click();
    step_until(&mut h, "installing", |h| {
        h.query_by_label("Installing…").is_some()
    });
    step_until(&mut h, "restart", |_| {
        f.world.updates.calls().iter().any(|c| c == "restart")
    });
}

#[test]
fn about_links_to_the_releases_page_when_the_build_does_not_update_itself() {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro")]);
    f.world.updates.answer(UpdateStatus::NotConfigured);
    let mut app = f.ui();
    app.settings_mut().page = Page::About;
    app.controller().check_for_updates();
    let mut h = Harness::builder()
        .with_size([880.0, 640.0])
        .renderer(CpuRenderer::default())
        .build_ui_state(
            |ui, app: &mut BuildRailUi| {
                let full = ui.ctx().content_rect();
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full));
                app.settings_ui(&mut child);
            },
            app,
        );
    let line = "This build does not update itself. Download new versions from the releases page.";
    step_until(&mut h, "not configured", |h| {
        h.query_by_label(line).is_some()
    });
    h.get_by_label("Open releases page").click();
    settle(&mut h);
    assert_eq!(f.world.urls.opened(), [RELEASES_PAGE.to_owned()]);
}

#[tokio::test(start_paused = true)]
async fn the_update_loop_paints_nothing_unless_what_is_shown_changes() {
    let f = Fixture::with_profile_for_async(&[gh("Keevoro", "keevoro"), cf("Site", "site")]);
    f.world.github.set(
        "keevoro",
        Ok(vec![record("7", 1, BuildStatus::Failed, 2, "keevoro")]),
    );
    f.world.cloudflare.set(
        "site",
        Ok(vec![record("9", 1, BuildStatus::Deployed, 30, "site")]),
    );
    let app = f.ui_on(tokio::runtime::Handle::current(), RefreshPolicy::default());
    let mut h = Harness::builder()
        .with_size([900.0, 24.0])
        .renderer(CpuRenderer::default())
        .build_ui_state(root_ui, app);
    // Let the first refresh land and paint it.
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        h.step();
        if loaded(&h) {
            break;
        }
    }
    assert!(loaded(&h));
    let repaints = Arc::new(AtomicUsize::new(0));
    let count = repaints.clone();
    h.state().controller().set_repaint(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));

    // Three hours of "Up to date." (and of unchanged builds): checked at
    // 20 s and hourly, logged, never painted.
    tokio::time::sleep(Duration::from_secs(3 * 3600)).await;
    assert_eq!(f.world.updates.checks(), 3);
    assert_eq!(repaints.load(Ordering::SeqCst), 0);

    // A release: exactly one repaint, and the chip.
    f.world
        .updates
        .answer(ScriptedUpdateService::available("0.3.0"));
    tokio::time::sleep(Duration::from_secs(2 * 3600)).await;
    assert_eq!(f.world.updates.checks(), 5);
    assert_eq!(repaints.load(Ordering::SeqCst), 1);
    h.step();
    assert!(h.query_by_label(CHIP).is_some());
}
