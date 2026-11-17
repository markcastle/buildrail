//! Window-level tests of the Settings window with `egui_kittest`: the
//! Add-build wizard, managing builds and connections, import/export and
//! device preferences, against fake catalogues and real file stores.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use buildrail_app::ui::BuildRailUi;
use buildrail_core::domain::{BuildStatus, ConnectionAlias};
use buildrail_core::ports::CredentialReader;
use buildrail_platform::autostart::Autostart;
use buildrail_profile::{Placement, Profile, Theme, WatchDraft};
use eframe::egui::accesskit::Role;
use eframe::egui::Key;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use support::*;

type H = Harness<'static, BuildRailUi>;

fn settle(h: &mut H) {
    for _ in 0..3 {
        h.step();
    }
}

/// Clicks the control named `label` (not a row's visible label of the
/// same name).
fn click(h: &mut H, label: &str) {
    let node = h
        .query_all_by_label(label)
        .find(|n| n.accesskit_node().role() != Role::Label)
        .unwrap_or_else(|| panic!("no {label:?}"));
    node.click_accesskit();
    settle(h);
}

fn wait_label(h: &mut H, label: &'static str) {
    step_until(h, label, |h| h.query_by_label_contains(label).is_some());
}

fn open_settings(h: &mut H, tab: &str) {
    settle(h);
    h.get_by_label("Settings").click_accesskit();
    settle(h);
    click(h, tab);
}

#[test]
fn wizard_happy_path_pins_a_watch() {
    let f = Fixture::new();
    f.world.github.set(
        "keevoro",
        Ok(vec![record("5", 1, BuildStatus::Passed, 3, "keevoro")]),
    );
    let mut h = f.harness([1000.0, 900.0]);
    settle(&mut h);
    assert!(h.state().is_settings_open(), "first launch opens Add build");

    click(&mut h, "GitHub Actions workflow");
    assert!(h
        .query_by_label_contains("set Actions and Metadata to Read-only")
        .is_some());
    type_into(&mut h, "Token", "ghp_test_token");
    click(&mut h, "Connect");
    wait_label(&mut h, "acme/keevoro");
    click(&mut h, "acme/keevoro");
    wait_label(&mut h, "CI, ci.yml");
    click(&mut h, "CI, ci.yml");
    wait_label(&mut h, "develop");
    click(&mut h, "Check latest run");
    wait_label(&mut h, "Use this build");
    assert!(h
        .query_by_label_contains("Passed · branch main · commit a7c412e")
        .is_some());
    click(&mut h, "Use this build");
    let label = field(&h, "Label");
    assert_eq!(label.value().as_deref(), Some("Keevoro"));
    click(&mut h, "Pin build");
    wait_label(&mut h, "is on the strip now");

    // Saved profile, stored credential, registered connection.
    let profile = f.paths.profile_store().load().unwrap();
    assert_eq!(profile.len(), 1);
    let watch = &profile.watches()[0];
    assert_eq!(watch.label, "Keevoro");
    assert_eq!(watch.connection.as_str(), "github");
    assert_eq!(
        f.world
            .credentials
            .read(&ConnectionAlias::new("github"))
            .unwrap()
            .unwrap()
            .expose(),
        "ghp_test_token"
    );
    let prefs = f.paths.device_prefs_store().load().unwrap();
    assert_eq!(
        prefs.connections[&ConnectionAlias::new("github")].display_name,
        "octocat"
    );
    // The strip shows it.
    step_until(&mut h, "on strip", |h| {
        h.query_by_label_contains("Keevoro, GitHub, Passed")
            .is_some()
    });
    // "Add another build" restarts the wizard.
    click(&mut h, "Add another build");
    assert!(h.query_by_label("What do you want to watch?").is_some());
}

#[test]
fn wizard_cloudflare_with_existing_connection_and_no_runs() {
    let f = Fixture::with_profile(&[]);
    let mut h = f.harness([1000.0, 900.0]);
    settle(&mut h);
    click(&mut h, "Cloudflare Workers build");
    click(&mut h, "cf");
    wait_label(&mut h, "Acme");
    click(&mut h, "Acme");
    wait_label(&mut h, "marketing-site");
    type_into(&mut h, "Filter", "market");
    assert!(h.query_by_label("api-gateway").is_none(), "filtered out");
    click(&mut h, "marketing-site");
    type_into(&mut h, "Branch", "main");
    click(&mut h, "Check latest run");
    wait_label(&mut h, "Nothing has run for this yet");
    click(&mut h, "Use it anyway");
    click(&mut h, "Pin build");
    wait_label(&mut h, "is on the strip now");
    let profile = f.paths.profile_store().load().unwrap();
    assert_eq!(profile.watches()[0].target.branch(), Some("main"));
    assert_eq!(profile.watches()[0].label, "Marketing-site");
}

#[test]
fn wizard_shows_access_error_and_back_navigation() {
    let f = Fixture::with_profile(&[]);
    f.world
        .catalog
        .update(|d| d.valid_token = Some("other".into()));
    let mut h = f.harness([1000.0, 900.0]);
    settle(&mut h);
    click(&mut h, "GitHub Actions workflow");
    click(&mut h, "work");
    wait_label(&mut h, "access was refused");
    assert!(h.query_by_label("Try again").is_some());
    click(&mut h, "Reconnect");
    assert!(h.query_all_by_label("Token").count() >= 1);
    type_into(&mut h, "Token", "wrong");
    click(&mut h, "Connect");
    wait_label(&mut h, "GitHub refused this token");
    click(&mut h, "Back");
    assert!(h.query_by_label("What do you want to watch?").is_some());
}

#[test]
fn malformed_import_leaves_profile_intact_and_shows_error() {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro")]);
    let before = std::fs::read(f.paths.profile_file()).unwrap();
    let mut h = f.harness([1000.0, 900.0]);
    open_settings(&mut h, "Share");
    type_into(
        &mut h,
        "Profile to import",
        "version = 1\n[[watch]]\nlable = \"x\"\n",
    );
    click(&mut h, "Preview import");
    wait_label(
        &mut h,
        "This profile cannot be imported. Nothing was changed.",
    );
    assert!(h.query_by_label_contains("lable").is_some());
    assert_eq!(std::fs::read(f.paths.profile_file()).unwrap(), before);
    assert_eq!(h.state().controller().profile().len(), 1);
}

#[test]
fn import_maps_aliases_and_replaces_profile() {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro")]);
    let incoming = Profile::from_drafts(&[
        WatchDraft::github("Billing", "team-gh", "acme", "billing", "ci.yml", None).with_id("b"),
        WatchDraft::cloudflare(
            "Site",
            "acme-cf",
            "0123456789abcdef0123456789abcdef",
            "site",
            None,
        )
        .with_id("s"),
    ])
    .unwrap();
    let file = f.dir.path().join("teammate.toml");
    std::fs::write(&file, buildrail_profile::profile_to_toml(&incoming)).unwrap();

    let mut h = f.harness([1000.0, 900.0]);
    open_settings(&mut h, "Share");
    type_into(&mut h, "Load file", file.to_str().unwrap());
    click(&mut h, "Load");
    click(&mut h, "Preview import");
    wait_label(&mut h, "2 added, 1 removed");
    assert!(h
        .query_by_label_contains("GitHub · 1 build in the profile")
        .is_some());
    click(&mut h, "Review changes");
    wait_label(&mut h, "Your pinned builds will be replaced");
    assert!(h
        .query_by_label("Removed · GitHub · acme/keevoro · ci.yml · branch main")
        .is_some());
    click(&mut h, "Replace my builds");
    wait_label(&mut h, "Imported 2 builds.");
    let saved = f.paths.profile_store().load().unwrap();
    let connections: Vec<&str> = saved
        .watches()
        .iter()
        .map(|w| w.connection.as_str())
        .collect();
    assert_eq!(connections, ["work", "cf"], "mapped to local connections");
    click(&mut h, "Import another");
    assert!(h.query_by_label("Preview import").is_some());
}

#[test]
fn import_can_create_a_missing_connection() {
    let f = Fixture::new();
    let incoming = Profile::from_drafts(&[WatchDraft::github(
        "Billing", "team-gh", "acme", "billing", "ci.yml", None,
    )
    .with_id("b")])
    .unwrap();
    let mut h = f.harness([1000.0, 900.0]);
    open_settings(&mut h, "Share");
    type_into(
        &mut h,
        "Profile to import",
        &buildrail_profile::profile_to_toml(&incoming),
    );
    click(&mut h, "Preview import");
    wait_label(&mut h, "Choose which of your connections");
    click(&mut h, "Review changes");
    wait_label(&mut h, "Choose a local GitHub connection for \"team-gh\"");
    click(&mut h, "New connection");
    assert_eq!(
        field(&h, "Connection name").value().as_deref(),
        Some("team-gh")
    );
    type_into(&mut h, "Token", "tok");
    click(&mut h, "Connect");
    step_until(&mut h, "mapped", |h| {
        h.state().settings().import().creating().is_none()
    });
    click(&mut h, "Review changes");
    click(&mut h, "Replace my builds");
    wait_label(&mut h, "Imported 1 build.");
    assert_eq!(
        f.paths.profile_store().load().unwrap().watches()[0]
            .connection
            .as_str(),
        "team-gh"
    );
}

#[test]
fn export_copies_and_saves_without_secrets() {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro")]);
    let mut h = f.harness([1000.0, 900.0]);
    open_settings(&mut h, "Share");
    click(&mut h, "Copy profile");
    assert!(h
        .query_by_label("Profile copied to the clipboard.")
        .is_some());
    let out = f.dir.path().join("export").join("shared.toml");
    type_into(&mut h, "Save to", out.to_str().unwrap());
    click(&mut h, "Export");
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("label = \"Keevoro\""));
    assert!(!text.contains("token"));
}

#[test]
fn manage_rename_reorder_remove_and_reconnect() {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro"), gh("Billing", "billing")]);
    let mut h = f.harness([1000.0, 900.0]);
    open_settings(&mut h, "Builds");

    // Rename (text is appended to the current label).
    click(&mut h, "Rename");
    type_into(&mut h, "Label", " API");
    h.key_press(Key::Enter);
    settle(&mut h);
    step_until(&mut h, "renamed", |h| {
        h.state().controller().profile().watches()[0].label == "Keevoro API"
    });

    // Reorder.
    let right = h.query_all_by_label("Move right").next().unwrap();
    right.click_accesskit();
    settle(&mut h);
    let labels: Vec<String> = f
        .paths
        .profile_store()
        .load()
        .unwrap()
        .watches()
        .iter()
        .map(|w| w.label.clone())
        .collect();
    assert_eq!(labels, ["Billing", "Keevoro API"]);

    // Reconnect with a new token.
    click(&mut h, "Reconnect");
    wait_label(&mut h, "Or paste a new token for the “work” connection");
    type_into(&mut h, "Token", "fresh");
    click(&mut h, "Connect");
    wait_label(&mut h, "Reconnected “work”");
    assert_eq!(
        f.world
            .credentials
            .read(&ConnectionAlias::new("work"))
            .unwrap()
            .unwrap()
            .expose(),
        "fresh"
    );

    // Remove needs confirmation.
    click(&mut h, "Remove");
    click(&mut h, "Keep");
    assert_eq!(h.state().controller().profile().len(), 2);
    click(&mut h, "Remove");
    click(&mut h, "Yes, remove");
    assert_eq!(f.paths.profile_store().load().unwrap().len(), 1);
}

#[test]
fn connections_tab_guards_removal() {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro")]);
    let mut h = f.harness([1000.0, 900.0]);
    open_settings(&mut h, "Connections");
    assert!(h.query_by_label_contains("Used by 1 build").is_some());
    // Rows are sorted by alias: "cf" first, then "work".
    let removes: Vec<_> = h.query_all_by_label("Remove").collect();
    assert_eq!(removes.len(), 2);
    removes[1].click_accesskit();
    settle(&mut h);
    wait_label(&mut h, "still used by Keevoro");
    h.query_all_by_label("Remove")
        .next()
        .unwrap()
        .click_accesskit();
    step_until(&mut h, "cf removed", |h| {
        h.state().controller().prefs().connections.len() == 1
    });
    assert!(f
        .world
        .credentials
        .read(&ConnectionAlias::new("cf"))
        .unwrap()
        .is_none());
}

#[test]
fn device_preferences_are_saved() {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro")]);
    let mut h = f.harness([1000.0, 900.0]);
    open_settings(&mut h, "Strip");
    click(&mut h, "Light");
    click(&mut h, "Start at login");
    click(&mut h, "Floating");
    click(&mut h, "Custom scale");
    let prefs = f.paths.device_prefs_store().load().unwrap();
    assert_eq!(prefs.theme, Theme::Light);
    assert!(prefs.launch_at_startup);
    assert!(f.world.autostart.is_enabled().unwrap());
    assert_eq!(prefs.placement, Placement::Floating { position: None });
    assert_eq!(prefs.scale, Some(1.0));
    click(&mut h, "Top");
    assert_eq!(
        f.paths.device_prefs_store().load().unwrap().placement,
        Placement::DockedTop
    );
    assert!(h
        .query_by_label_contains("At the top or bottom, the strip sits along that edge")
        .is_some());
    click(&mut h, "Bottom");
    assert_eq!(
        f.paths.device_prefs_store().load().unwrap().placement,
        Placement::DockedBottom
    );
    // The settings window can be closed and reopened.
    h.state_mut().set_settings_open(false);
    settle(&mut h);
    assert!(h.query_by_label("Quit BuildRail").is_none());
}

#[test]
fn empty_builds_tab_links_to_the_wizard() {
    let f = Fixture::with_profile(&[]);
    let mut h = f.harness([1000.0, 900.0]);
    settle(&mut h);
    // "Builds" is also the strip caption; pick the tab below the strip.
    h.query_all_by_label("Builds")
        .find(|n| n.rect().min.y > 24.0)
        .unwrap()
        .click_accesskit();
    settle(&mut h);
    assert!(h.query_by_label("No builds are pinned yet.").is_some());
    let add = h
        .query_all_by_label("Add build")
        .find(|n| {
            n.rect().min.y > 24.0
                && n.accesskit_node().role() == eframe::egui::accesskit::Role::Button
        })
        .unwrap();
    add.click_accesskit();
    settle(&mut h);
    assert!(h.query_by_label("What do you want to watch?").is_some());
}

/// A harness that draws only the Settings window, as the screenshots do.
fn settings_only(f: &Fixture) -> H {
    Harness::builder()
        .with_size([900.0, 1000.0])
        .renderer(CpuRenderer::default())
        .build_ui_state(
            |ui, app: &mut BuildRailUi| {
                let full = ui.ctx().content_rect();
                let mut child = ui.new_child(eframe::egui::UiBuilder::new().max_rect(full));
                let out = app.settings_ui(&mut child);
                if out.quit {
                    app.set_settings_open(false);
                }
            },
            f.ui(),
        )
}

#[test]
fn strip_page_picks_a_display_and_a_scale() {
    use buildrail_app::ui::settings::{DeviceInfo, MonitorInfo, Page};
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro")]);
    let mut h = settings_only(&f);
    let monitor = |id: &str, primary| MonitorInfo {
        id: id.into(),
        name: format!("Display {id}"),
        width: 2560,
        height: 1440,
        primary,
    };
    h.state_mut().set_device_info(DeviceInfo {
        monitors: vec![monitor("1", true), monitor("2", false)],
        placement_note: Some("Docked above the taskbar.".into()),
    });
    h.state_mut().settings_mut().page = Page::Strip;
    settle(&mut h);
    assert!(h.query_by_label("Docked above the taskbar.").is_some());
    click(&mut h, "Display 2, 2560 × 1440");
    let prefs = f.paths.device_prefs_store().load().unwrap();
    assert_eq!(prefs.monitor.unwrap().id, "2");
    assert!(
        h.query_by_label("Saved").is_some(),
        "the change is confirmed"
    );
    click(&mut h, "Display 1 (main), 2560 × 1440");
    assert!(f
        .paths
        .device_prefs_store()
        .load()
        .unwrap()
        .monitor
        .is_none());
    click(&mut h, "Custom scale");
    assert!(h.query_by_role(Role::Slider).is_some());
    click(&mut h, "Dark");
    assert_eq!(
        f.paths.device_prefs_store().load().unwrap().theme,
        Theme::Dark
    );
}

#[test]
fn about_done_and_quit() {
    use buildrail_app::ui::settings::Page;
    let f = Fixture::new();
    let mut h = settings_only(&f);
    settle(&mut h);
    click(&mut h, "About");
    assert_eq!(h.state().settings().page, Page::About);
    assert!(h.query_by_label("MIT").is_some());
    assert!(h
        .query_by_label(concat!("Version ", env!("CARGO_PKG_VERSION")))
        .is_some());
    click(&mut h, "Connections");
    assert!(h.query_by_label_contains("No connections yet").is_some());
    h.state_mut().set_settings_open(true);
    click(&mut h, "Done");
    assert!(!h.state().is_settings_open());
    h.state_mut().set_settings_open(true);
    click(&mut h, "Quit BuildRail");
    assert!(!h.state().is_settings_open());
}

#[test]
fn export_and_load_errors_say_what_to_do() {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro")]);
    let mut h = f.harness([1000.0, 1000.0]);
    open_settings(&mut h, "Share");
    let missing = f.dir.path().join("nope").join("in").join("here.toml");
    type_into(&mut h, "Load file", missing.to_str().unwrap());
    click(&mut h, "Load");
    wait_label(&mut h, "could not be read");
    let blocked = f.dir.path().join("profile.toml").join("x.toml");
    type_into(&mut h, "Save to", blocked.to_str().unwrap());
    click(&mut h, "Export");
    wait_label(&mut h, "could not be saved to");
}

#[test]
fn native_dialogs_import_and_export_and_cancelling_does_nothing() {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro")]);
    let out = f.dir.path().join("picked.toml");
    // Export: cancel once, then pick a file.
    f.world.dialogs.answer(None);
    f.world.dialogs.answer(Some(out.clone()));
    // Import: pick the file just written.
    f.world.dialogs.answer(Some(out.clone()));
    let mut h = f.harness([1000.0, 1000.0]);
    open_settings(&mut h, "Share");
    click(&mut h, "Save as…");
    assert!(!out.exists(), "cancelled");
    click(&mut h, "Save as…");
    wait_label(&mut h, "Saved the profile to");
    assert!(std::fs::read_to_string(&out)
        .unwrap()
        .contains("label = \"Keevoro\""));
    click(&mut h, "Choose file…");
    step_until(&mut h, "loaded", |h| {
        h.state().settings().import().text().contains("Keevoro")
    });
    assert_eq!(
        f.world.dialogs.asked(),
        ["save builds.toml", "save builds.toml", "open"]
    );
    // A file that cannot be read says so.
    f.world
        .dialogs
        .answer(Some(f.dir.path().join("missing.toml")));
    click(&mut h, "Choose file…");
    wait_label(&mut h, "could not be read");
}

#[test]
fn animation_is_off_until_turned_on() {
    let f = Fixture::with_profile(&[gh("Keevoro", "keevoro")]);
    let mut h = f.harness([1000.0, 900.0]);
    open_settings(&mut h, "Strip");
    assert!(!h.state().controller().prefs().animation);
    assert!(h
        .query_by_label_contains("Off, nothing on the strip moves.")
        .is_some());
    click(&mut h, "Animation");
    assert!(f.paths.device_prefs_store().load().unwrap().animation);
}

#[test]
fn about_opens_the_log_folder() {
    let f = Fixture::new();
    let mut h = settings_only(&f);
    settle(&mut h);
    click(&mut h, "About");
    assert!(h
        .query_by_label(concat!("Version ", env!("CARGO_PKG_VERSION")))
        .is_some());
    // The folder does not exist yet: the error says so.
    click(&mut h, "Open log folder");
    wait_label(&mut h, "The log folder could not be opened");
    std::fs::create_dir_all(f.paths.logs_dir()).unwrap();
    click(&mut h, "Open log folder");
    assert_eq!(f.world.folders.opened(), [f.paths.logs_dir().to_owned()]);
    assert!(h
        .query_by_label_contains("The log folder could not be opened")
        .is_none());
}
