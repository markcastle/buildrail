//! End-to-end: export on one "machine", import on another, through real
//! files, plus property tests for the TOML round trip.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::fs;

use buildrail_core::domain::{ConnectionAlias, ProviderKind};
use buildrail_profile::{
    profile_to_toml, AliasMapping, AppPaths, DevicePrefs, ImportError, ImportPreview,
    LocalConnection, MappingProblem, Profile, WatchDraft,
};
use proptest::prelude::*;

fn machine(root: &std::path::Path, name: &str) -> AppPaths {
    let base = root.join(name);
    AppPaths::from_dirs(base.join("config"), base.join("local"), base.join("cache"))
}

fn sender_profile() -> Profile {
    let (p, _) = Profile::empty()
        .with_added(WatchDraft::github(
            "API",
            "work-gh",
            "acme",
            "api",
            "ci.yml",
            Some("main".into()),
        ))
        .unwrap();
    let (p, _) = p
        .with_added(WatchDraft::cloudflare(
            "Site",
            "acme-cf",
            "0123456789abcdef0123456789abcdef",
            "site",
            None,
        ))
        .unwrap();
    let (p, _) = p
        .with_added(WatchDraft::github(
            "Nightly",
            "work-gh",
            "acme",
            "api",
            "nightly.yml",
            None,
        ))
        .unwrap();
    p.with_moved(2, 0).unwrap()
}

#[test]
fn export_on_one_machine_import_on_another() {
    let root = tempfile::tempdir().unwrap();
    let a = machine(root.path(), "a");
    let b = machine(root.path(), "b");

    // Machine A saves and exports.
    a.profile_store().save(&sender_profile()).unwrap();
    let export_path = root.path().join("shared.toml");
    fs::write(
        &export_path,
        profile_to_toml(&a.profile_store().load().unwrap()),
    )
    .unwrap();

    // Machine B has its own profile and one GitHub connection.
    let b_store = b.profile_store();
    let (existing, _) = Profile::empty()
        .with_added(WatchDraft::github(
            "Mine", "me", "me", "dotfiles", "ci.yml", None,
        ))
        .unwrap();
    b_store.save(&existing).unwrap();
    let mut prefs = DevicePrefs::default();
    prefs.connections.insert(
        ConnectionAlias::new("me"),
        LocalConnection {
            provider: ProviderKind::GitHub,
            display_name: "me".into(),
        },
    );

    let preview = ImportPreview::from_toml(
        &b_store.load().unwrap(),
        &fs::read_to_string(&export_path).unwrap(),
    )
    .unwrap();
    let mapping = preview.suggested_mapping(&prefs.connections);

    // No Cloudflare connection yet: the UI must prompt for local credentials.
    let ImportError::Mapping(problems) = preview.apply(&mapping, &prefs.connections).unwrap_err()
    else {
        panic!("expected a mapping error")
    };
    assert!(matches!(
        &problems[..],
        [MappingProblem::Unmapped(r)] if r.alias.as_str() == "acme-cf" && r.provider == ProviderKind::Cloudflare
    ));
    assert_eq!(b_store.load().unwrap(), existing, "nothing saved yet");

    // The user adds a Cloudflare connection and maps it.
    prefs.connections.insert(
        ConnectionAlias::new("cf"),
        LocalConnection {
            provider: ProviderKind::Cloudflare,
            display_name: "me@example.com".into(),
        },
    );
    b.device_prefs_store().save(&prefs).unwrap();
    let mut mapping: AliasMapping = mapping;
    mapping.insert(
        ConnectionAlias::new("acme-cf"),
        ProviderKind::Cloudflare,
        ConnectionAlias::new("cf"),
    );
    let applied = preview.apply(&mapping, &prefs.connections).unwrap();
    b_store.save(&applied).unwrap();

    let sent = a.profile_store().load().unwrap();
    let received = b_store.load().unwrap();
    let shape = |p: &Profile| {
        p.watches()
            .iter()
            .map(|w| (w.id.clone(), w.label.clone(), w.target.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shape(&received),
        shape(&sent),
        "identical targets and order"
    );
    let aliases: Vec<&str> = received
        .watches()
        .iter()
        .map(|w| w.connection.as_str())
        .collect();
    assert_eq!(aliases, ["me", "me", "cf"]);

    // The device preferences file is separate and was not part of the export.
    let exported = fs::read_to_string(&export_path).unwrap();
    assert!(!exported.contains("display_name"));
    assert!(!exported.contains("me@example.com"));
}

#[test]
fn malformed_import_leaves_working_configuration_intact() {
    let root = tempfile::tempdir().unwrap();
    let paths = machine(root.path(), "a");
    let store = paths.profile_store();
    store.save(&sender_profile()).unwrap();
    let before = fs::read(store.path()).unwrap();

    for bad in [
        "version = 1\n[[watch]\n",
        "version = 2\n",
        "version = 1\n[[watch]]\nid = \"x\"\nlabel = \"\"\n",
    ] {
        assert!(ImportPreview::from_toml(&store.load().unwrap(), bad).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), before);
    }
}

fn draft_strategy() -> impl Strategy<Value = WatchDraft> {
    let label = "[A-Za-z0-9 \"'\\\\#=é-]{1,30}".prop_filter("non-blank", |s| !s.trim().is_empty());
    let alias = "[a-z0-9][a-z0-9._-]{0,15}";
    let branch = proptest::option::of("[a-z0-9]{1,8}(/[a-z0-9]{1,8})?");
    let github = (
        label.clone(),
        alias,
        "[A-Za-z0-9]{1,10}",
        "[A-Za-z0-9_.-]{0,10}[A-Za-z0-9]",
        "[a-z0-9][a-z0-9 _-]{0,9}\\.ya?ml|[0-9]{1,9}",
        branch.clone(),
    )
        .prop_map(|(l, c, o, r, w, b)| WatchDraft::github(l, c, o, r, w, b));
    let cloudflare = (
        label,
        alias,
        "[0-9a-f]{32}",
        "[a-z0-9][a-z0-9_]{0,20}",
        branch,
    )
        .prop_map(|(l, c, a, w, b)| WatchDraft::cloudflare(l, c, a, w, b));
    prop_oneof![github, cloudflare]
}

proptest! {
    #[test]
    fn toml_round_trip_preserves_profile(drafts in proptest::collection::vec(draft_strategy(), 0..8)) {
        let profile = Profile::from_drafts(&drafts).unwrap();
        let text = profile_to_toml(&profile);
        prop_assert_eq!(buildrail_profile::parse_profile(&text).unwrap(), profile);
    }

    #[test]
    fn reorder_is_a_permutation(drafts in proptest::collection::vec(draft_strategy(), 1..8), from in 0usize..8, to in 0usize..8) {
        let profile = Profile::from_drafts(&drafts).unwrap();
        let (from, to) = (from % profile.len(), to % profile.len());
        let moved = profile.with_moved(from, to).unwrap();
        prop_assert_eq!(moved.watches()[to].id.clone(), profile.watches()[from].id.clone());
        let mut a: Vec<_> = profile.watches().iter().map(|w| w.id.clone()).collect();
        let mut b: Vec<_> = moved.watches().iter().map(|w| w.id.clone()).collect();
        a.sort();
        b.sort();
        prop_assert_eq!(a, b);
        prop_assert_eq!(moved.with_moved(to, from).unwrap(), profile);
    }
}
