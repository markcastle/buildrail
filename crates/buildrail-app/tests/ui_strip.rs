//! Window-level tests of the strip with `egui_kittest`: real UI, real
//! controller and refresh engine, fake providers.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::path::PathBuf;

use buildrail_app::demo::{demo_connections, demo_profile, DemoRunSource, PHASE_SECS};
use buildrail_app::fakes::ManualClock;
use buildrail_app::ui::BuildRailUi;
use buildrail_core::domain::{BuildStatus, ConnectionAlias, ProviderKind};
use buildrail_core::ports::{CredentialWriter, ProviderError, Secret};
use buildrail_core::refresh::SourceRegistry;
use chrono::TimeDelta;
use eframe::egui::accesskit::Role;
use eframe::egui::Key;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use support::*;

const KEEVORO: &str =
    "Keevoro, GitHub, Failed, branch main, commit a7c412e, 2 minutes ago, failing job test";

fn three_builds() -> Fixture {
    let f = Fixture::with_profile(&[
        gh("Keevoro", "keevoro"),
        cf("Site", "site"),
        cf("Docs", "docs"),
    ]);
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

/// Waits until every entry of [`three_builds`] has its first result. The
/// three requests are fetched concurrently, so one entry being loaded says
/// nothing about the others.
fn wait_all_loaded(h: &mut Harness<'_, BuildRailUi>) {
    step_until(h, "all loaded", |h| {
        h.query_by_label(KEEVORO).is_some()
            && h.query_by_label_contains("Site, Cloudflare, Deployed")
                .is_some()
            && h.query_by_label("Docs, Cloudflare, No runs yet").is_some()
    });
}

fn entries(h: &Harness<'_, BuildRailUi>) -> Vec<String> {
    labels_left_to_right(h, Role::Button)
        .into_iter()
        .filter(|l| l.contains(", GitHub, ") || l.contains(", Cloudflare, "))
        .collect()
}

#[test]
fn entries_in_pinned_order_with_accessible_names() {
    let f = three_builds();
    let mut h = f.harness([900.0, 24.0]);
    step_until(&mut h, "all loaded", |h| {
        h.query_by_label(KEEVORO).is_some()
            && h.query_by_label_contains("Site, Cloudflare, Deployed")
                .is_some()
            && h.query_by_label("Docs, Cloudflare, No runs yet").is_some()
    });
    let names = entries(&h);
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(names[0], KEEVORO);
    assert!(
        names[1].starts_with("Site, Cloudflare, Deployed, branch main"),
        "{}",
        names[1]
    );
    assert_eq!(names[2], "Docs, Cloudflare, No runs yet");
    // The caption, add and settings buttons are labelled too.
    assert!(h.query_by_label("Builds").is_some());
    assert!(h
        .query_by_role_and_label(Role::Button, "Settings")
        .is_some());
    assert!(h
        .query_by_role_and_label(Role::Button, "Add build")
        .is_some());
    // First launch does not apply: settings stays closed.
    assert!(!h.state().is_settings_open());
}

#[test]
fn order_is_stable_when_a_failed_build_is_rerun() {
    let f = three_builds();
    let mut h = f.harness([900.0, 24.0]);
    wait_all_loaded(&mut h);
    let before: Vec<String> = entries(&h)
        .iter()
        .map(|n| n.split(',').next().unwrap().to_owned())
        .collect();

    // Re-run: same run, attempt 2, now running.
    f.world.github.set(
        "keevoro",
        Ok(vec![record("7", 2, BuildStatus::Running, 0, "keevoro")]),
    );
    h.state().controller().refresh_now();
    step_until(&mut h, "rerun shown", |h| {
        h.query_by_label_contains("Keevoro, GitHub, Building")
            .is_some()
    });
    let after: Vec<String> = entries(&h)
        .iter()
        .map(|n| n.split(',').next().unwrap().to_owned())
        .collect();
    assert_eq!(before, after);
    assert_eq!(after, ["Keevoro", "Site", "Docs"]);
}

#[test]
fn keyboard_focus_shows_details_and_enter_opens_the_run() {
    let f = three_builds();
    let mut h = f.harness([900.0, 24.0]);
    wait_all_loaded(&mut h);

    h.get_by_label(KEEVORO).focus();
    h.step();
    h.step();
    assert!(h.get_by_label(KEEVORO).is_focused());
    // Focus reveals the same details as hover.
    assert!(h.query_by_label("Failing job: test").is_some());
    assert!(h.query_by_label("Commit a7c412e").is_some());

    h.key_press(Key::Enter);
    h.step();
    assert_eq!(
        f.world.urls.opened(),
        vec!["https://github.com/acme/keevoro/actions/runs/7".to_owned()]
    );

    // Arrow keys move between entries; Space activates too.
    h.key_press(Key::ArrowRight);
    h.step();
    h.step();
    assert!(h.get_by_label_contains("Site, Cloudflare").is_focused());
    h.key_press(Key::Space);
    h.step();
    assert_eq!(f.world.urls.opened().len(), 2);

    // Hover shows details as well.
    h.get_by_label("Docs, Cloudflare, No runs yet").hover();
    h.step();
    h.step();
    assert!(h.query_by_label("Cloudflare · Worker docs").is_some());
}

#[test]
fn offline_entry_shows_cached_state_with_age_then_recovers() {
    let f = three_builds();
    let mut h = f.harness([900.0, 24.0]);
    wait_all_loaded(&mut h);

    f.world.clock.advance(TimeDelta::minutes(5));
    f.world
        .github
        .set("keevoro", Err(ProviderError::Network("dns failure".into())));
    h.state().controller().refresh_now();
    step_until(&mut h, "stale", |h| {
        h.query_by_label_contains("offline, last updated 5 minutes ago")
            .is_some()
    });
    let name = h
        .get_by_label_contains("offline, last updated")
        .accesskit_node()
        .label()
        .unwrap();
    assert!(
        name.starts_with("Keevoro, GitHub, Failed"),
        "cached run still shown: {name}"
    );

    // Network back (a wake or reconnect calls refresh_now).
    f.world.github.set(
        "keevoro",
        Ok(vec![record("8", 1, BuildStatus::Passed, 0, "keevoro")]),
    );
    h.state().controller().refresh_now();
    step_until(&mut h, "recovered", |h| {
        h.query_by_label_contains("Keevoro, GitHub, Passed")
            .is_some()
    });
    assert!(h.query_by_label_contains("offline").is_none());
}

#[test]
fn overflow_counts_hidden_failures() {
    let f = Fixture::with_profile(&[
        gh("Keevoro", "keevoro"),
        gh("Billing", "billing"),
        gh("Payments", "payments"),
        gh("Search", "search"),
    ]);
    for (repo, status) in [
        ("keevoro", BuildStatus::Passed),
        ("billing", BuildStatus::Failed),
        ("payments", BuildStatus::Failed),
        ("search", BuildStatus::Running),
    ] {
        f.world
            .github
            .set(repo, Ok(vec![record("1", 1, status, 1, repo)]));
    }
    let mut h = f.harness([420.0, 300.0]);
    step_until(&mut h, "overflow", |h| {
        h.query_by_label_contains("more builds, 2 failed").is_some()
    });
    let overflow = h.get_by_label_contains("more builds, 2 failed");
    let label = overflow.accesskit_node().label().unwrap();
    assert!(label.ends_with("2 failed"), "{label}");
    assert!(entries(&h).len() < 4);

    // Activating it lists the hidden builds; choosing one opens it.
    overflow.click();
    h.step();
    h.step();
    let hidden = h.get_by_label_contains("Payments, GitHub, Failed");
    hidden.click();
    h.step();
    assert!(f.world.urls.opened()[0].contains("/payments/"));
}

#[test]
fn first_launch_shows_add_build() {
    let f = Fixture::new();
    let mut h = f.harness([900.0, 700.0]);
    h.step();
    h.step();
    let add = h
        .query_all_by_role_and_label(Role::Button, "Add build")
        .find(|n| n.rect().max.y <= 24.0)
        .expect("Add build on the strip");
    assert!(add.rect().height() <= 24.0);
    assert!(h.query_by_label("No builds pinned yet").is_some());
    // Settings opened itself on the wizard.
    assert!(h.state().is_settings_open());
    assert!(h.query_by_label("What do you want to watch?").is_some());
}

#[test]
fn reconnect_needed_entry_opens_settings() {
    let f = Fixture::with_profile(&[gh("Billing", "billing")]);
    f.world.github.set(
        "billing",
        Err(ProviderError::AccessDenied {
            provider: ProviderKind::GitHub,
        }),
    );
    let mut h = f.harness([900.0, 600.0]);
    step_until(&mut h, "reconnect", |h| {
        h.query_by_label("Billing, GitHub, Reconnect needed, branch main")
            .is_some()
    });
    h.get_by_label("Billing, GitHub, Reconnect needed, branch main")
        .click();
    h.step();
    h.step();
    assert!(h.state().is_settings_open());
    assert!(h.query_by_label_contains("Or paste a new token").is_some());
}

/// The strip lays out in exactly 24 logical px and renders; the demo
/// profile is drawn and saved as a PNG (to `docs/images/strip-demo.png`
/// when `BUILDRAIL_WRITE_SCREENSHOT=1`, else under `target/`).
#[test]
fn strip_lays_out_in_24_px_and_renders() {
    let f = Fixture::new();
    let clock = std::sync::Arc::new(ManualClock::new(t0()));
    let epoch = t0() - TimeDelta::seconds(PHASE_SECS * 3);
    let profile = demo_profile().unwrap();
    f.paths.profile_store().save(&profile).unwrap();
    let mut prefs = buildrail_profile::DevicePrefs::default();
    for (alias, connection) in demo_connections() {
        prefs
            .connections
            .insert(ConnectionAlias::new(alias), connection);
        f.world
            .credentials
            .write(&ConnectionAlias::new(alias), &Secret::new("t"))
            .unwrap();
    }
    let theme = std::env::var("BUILDRAIL_SCREENSHOT_THEME").unwrap_or_default();
    prefs.theme = if theme == "light" {
        buildrail_profile::Theme::Light
    } else {
        buildrail_profile::Theme::Dark
    };
    f.paths.device_prefs_store().save(&prefs).unwrap();
    let mut services = f.world.services(&f.paths);
    services.clock = clock.clone();
    services.sources = SourceRegistry::new()
        .with(std::sync::Arc::new(DemoRunSource::new(
            ProviderKind::GitHub,
            clock.clone(),
            epoch,
        )))
        .with(std::sync::Arc::new(DemoRunSource::new(
            ProviderKind::Cloudflare,
            clock.clone(),
            epoch,
        )));
    let controller = buildrail_app::app::AppController::start(
        services,
        std::sync::Arc::new(f.paths.snapshot_cache()),
        fast_policy(),
        f.handle(),
    );
    let mut app = BuildRailUi::new(controller);
    app.set_animate(false);
    let mut h = Harness::builder()
        .with_size([1280.0, 24.0])
        .with_pixels_per_point(1.0)
        .renderer(CpuRenderer::default())
        .build_ui_state(root_ui, app);
    step_until(&mut h, "demo loaded", |h| {
        h.query_by_label_contains("Keevoro, GitHub, Checking")
            .is_none()
            && h.query_by_label_contains("Docs, Cloudflare, No runs yet")
                .is_some()
            && h.query_by_label_contains("Marketing site, Cloudflare, Checking")
                .is_none()
    });
    for node in h.query_all_by_role(Role::Button) {
        let r = node.rect();
        if r.min.y < 24.0 {
            assert!(r.min.y >= 0.0 && r.max.y <= 24.0, "{r:?}");
        }
    }
    let image = h.render().unwrap();
    assert_eq!((image.width(), image.height()), (1280, 24));
    // The strip background is painted edge to edge.
    let bg = image.get_pixel(1, 1);
    assert_eq!(bg.0[3], 255);
    let name = if theme == "light" {
        "strip-demo-light.png"
    } else {
        "strip-demo.png"
    };
    let out = if std::env::var("BUILDRAIL_WRITE_SCREENSHOT").as_deref() == Ok("1") {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/images")
            .join(name)
    } else {
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
    };
    save_png(&image, &out);
}
