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
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use support::*;

type H = Harness<'static, BuildRailUi>;

fn settle(h: &mut H) {
    for _ in 0..3 {
        h.step();
    }
}

fn click(h: &mut H, label: &str) {
    let node = h
        .query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("no {label:?}"));
    node.click_accesskit();
    settle(h);
}

fn type_into(h: &mut H, field: &str, text: &str) {
    h.get_by_label(field).focus();
    settle(h);
    h.get_by_label(field).type_text(text);
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
        .query_by_label_contains("Actions to Read-only and Metadata to Read-only")
        .is_some());
    type_into(&mut h, "Token", "ghp_test_token");
    click(&mut h, "Connect");
    wait_label(&mut h, "acme/keevoro");
    click(&mut h, "acme/keevoro");
    wait_label(&mut h, "CI (ci.yml)");
    click(&mut h, "CI (ci.yml)");
    wait_label(&mut h, "develop");
    click(&mut h, "Preview");
    wait_label(&mut h, "This is the right build");
    assert!(h
        .query_by_label_contains("Passed · branch main · commit a7c412e")
        .is_some());
    click(&mut h, "This is the right build");
    let label = h.get_by_label("Short label shown on the strip");
    assert_eq!(label.value().as_deref(), Some("Keevoro"));
    click(&mut h, "Pin");
    wait_label(&mut h, "It is on the strip now");

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
    click(&mut h, "cf (cf-user)");
    wait_label(&mut h, "Acme");
    click(&mut h, "Acme");
    wait_label(&mut h, "marketing-site");
    type_into(&mut h, "Filter", "market");
    assert!(h.query_by_label("api-gateway").is_none(), "filtered out");
    click(&mut h, "marketing-site");
    type_into(&mut h, "Branch (leave empty for every branch)", "main");
    click(&mut h, "Preview");
    wait_label(&mut h, "No runs yet. The target exists");
    click(&mut h, "Pin it anyway");
    click(&mut h, "Pin");
    wait_label(&mut h, "It is on the strip now");
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
    click(&mut h, "work (work-user)");
    wait_label(&mut h, "Access error");
    assert!(h.query_by_label("Try again").is_some());
    click(&mut h, "Reconnect");
    assert!(h.query_by_label("Token").is_some());
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
    open_settings(&mut h, "Import / export");
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
    open_settings(&mut h, "Import / export");
    type_into(&mut h, "Load file", file.to_str().unwrap());
    click(&mut h, "Load");
    click(&mut h, "Preview import");
    wait_label(&mut h, "2 added, 1 removed");
    assert!(h
        .query_by_label_contains("\"team-gh\" (GitHub, 1 build)")
        .is_some());
    click(&mut h, "Review changes");
    wait_label(&mut h, "Your pinned builds will be replaced");
    assert!(h
        .query_by_label("- Keevoro (GitHub acme/keevoro · ci.yml · main)")
        .is_some());
    click(&mut h, "Replace my builds");
    wait_label(&mut h, "Imported. 2 builds are pinned.");
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
    open_settings(&mut h, "Import / export");
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
        h.get_by_label("Connection name").value().as_deref(),
        Some("team-gh")
    );
    type_into(&mut h, "Token", "tok");
    click(&mut h, "Connect");
    step_until(&mut h, "mapped", |h| h.query_by_label("Token").is_none());
    click(&mut h, "Review changes");
    click(&mut h, "Replace my builds");
    wait_label(&mut h, "Imported. 1 builds are pinned.");
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
    open_settings(&mut h, "Import / export");
    click(&mut h, "Copy profile");
    assert!(h
        .query_by_label("Profile copied to the clipboard.")
        .is_some());
    let out = f.dir.path().join("export").join("shared.toml");
    type_into(&mut h, "Save to", out.to_str().unwrap());
    click(&mut h, "Save file");
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
    type_into(&mut h, "New label", " API");
    click(&mut h, "Save");
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
    wait_label(&mut h, "Or paste a new token for \"work\"");
    type_into(&mut h, "Token", "fresh");
    click(&mut h, "Connect");
    wait_label(&mut h, "Reconnected \"work\"");
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
    assert!(h.query_by_label_contains("used by 1 build").is_some());
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
    open_settings(&mut h, "This device");
    click(&mut h, "Light");
    click(&mut h, "Start BuildRail when I log in");
    click(&mut h, "Floating window");
    click(&mut h, "Override display scaling");
    let prefs = f.paths.device_prefs_store().load().unwrap();
    assert_eq!(prefs.theme, Theme::Light);
    assert!(prefs.launch_at_startup);
    assert!(f.world.autostart.is_enabled().unwrap());
    assert_eq!(prefs.placement, Placement::Floating { position: None });
    assert_eq!(prefs.scale, Some(1.0));
    click(&mut h, "Docked at the bottom edge");
    assert_eq!(
        f.paths.device_prefs_store().load().unwrap().placement,
        Placement::DockedBottom
    );
    // The settings window can be closed and reopened.
    h.state_mut().set_settings_open(false);
    settle(&mut h);
    assert!(h.query_by_label("This device").is_none());
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
