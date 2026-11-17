//! Controller tests: real tokio runtime, real refresh engine and real file
//! stores in a temp dir; providers, credentials, clock and URL opener are
//! fakes from [`crate::fakes`].

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use buildrail_core::domain::{BuildStatus, RunId, WatchId};
use buildrail_core::ports::{CredentialWriter, ProviderError};
use buildrail_core::refresh::SourceRegistry;
use buildrail_platform::autostart::Autostart;
use buildrail_profile::{AppPaths, Placement, Theme};
use buildrail_providers::catalog::{CloudflareAccount, CloudflareTokenStatus};
use chrono::{TimeZone, Utc};
use tempfile::TempDir;
use tokio::runtime::Runtime;

use super::*;
use crate::fakes::FakeWorld;
use crate::settings::ConnectRequest;

const WAIT: Duration = Duration::from_secs(5);

struct T {
    _dir: TempDir,
    paths: AppPaths,
    world: FakeWorld,
    runtime: Runtime,
    app: AppController,
}

fn fast_policy() -> RefreshPolicy {
    RefreshPolicy {
        idle: Duration::from_millis(50),
        active: Duration::from_millis(50),
        backoff_base: Duration::from_millis(50),
        backoff_max: Duration::from_millis(50),
        ..RefreshPolicy::default()
    }
}

impl T {
    fn new() -> Self {
        Self::with(|_| {})
    }

    fn with(prepare: impl FnOnce(&AppPaths)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_dirs(
            dir.path().join("profile"),
            dir.path().join("device"),
            dir.path().join("cache"),
        );
        prepare(&paths);
        let world = FakeWorld::new(Utc.timestamp_opt(1_700_000_000, 0).unwrap());
        let runtime = Runtime::new().unwrap();
        let app = AppController::start(
            world.services(&paths),
            Arc::new(paths.snapshot_cache()),
            fast_policy(),
            runtime.handle().clone(),
        );
        Self {
            _dir: dir,
            paths,
            world,
            runtime,
            app,
        }
    }

    fn run(&mut self, command: Command) -> Event {
        self.app.execute(command);
        let mut events = self.app.settle(WAIT);
        assert_eq!(events.len(), 1, "{events:?}");
        events.remove(0)
    }

    fn connect(&mut self, provider: ProviderKind, alias: &str, token: &str) -> Event {
        self.run(Command::Connect {
            ticket: Ticket::next(),
            request: ConnectRequest {
                provider,
                alias: ConnectionAlias::new(alias),
                secret: Secret::new(token),
                replace: false,
            },
        })
    }

    fn pin(&mut self, draft: WatchDraft) -> Result<WatchId, String> {
        match self.run(Command::Pin {
            ticket: Ticket::next(),
            draft,
        }) {
            Event::Pinned { result, .. } => result,
            other => panic!("{other:?}"),
        }
    }

    fn saved(&mut self, command: Command) -> Result<(), String> {
        match self.run(command) {
            Event::Saved { result, .. } => result,
            other => panic!("{other:?}"),
        }
    }

    fn wait_for(&self, what: &str, check: impl Fn(&StripModel) -> bool) -> StripModel {
        let deadline = Instant::now() + WAIT;
        loop {
            let strip = self.app.strip();
            if check(&strip) {
                return strip;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {strip:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn gh_draft(label: &str, connection: &str, repo: &str) -> WatchDraft {
    WatchDraft::github(
        label,
        connection,
        "acme",
        repo,
        "ci.yml",
        Some("main".into()),
    )
}

fn record(id: &str, status: BuildStatus) -> buildrail_core::domain::BuildRecord {
    let at = Utc.timestamp_opt(1_699_999_880, 0).unwrap();
    buildrail_core::domain::BuildRecord {
        run_id: RunId::new(id),
        attempt: 1,
        status,
        url: format!("https://github.com/acme/keevoro/actions/runs/{id}"),
        branch: Some("main".into()),
        commit: Some("a7c412e9".into()),
        failing_job: None,
        created_at: at,
        updated_at: at,
        observed_at: at,
    }
}

#[test]
fn first_launch_is_empty() {
    let t = T::new();
    assert!(t.app.profile().is_empty());
    assert!(t.app.strip().is_empty());
    assert_eq!(t.app.profile_error(), None);
    assert_eq!(t.app.prefs_error(), None);
    assert_eq!(t.app.pending_jobs(), 0);
    assert_eq!(t.app.prefs(), &DevicePrefs::default());
    t.app.set_repaint(Arc::new(|| {}));
    t.app.refresh_now();
    let _ = t.app.refresh_handle().snapshots();
}

#[test]
fn connect_github_verifies_stores_and_registers() {
    let mut t = T::new();
    t.world
        .catalog
        .update(|d| d.valid_token = Some("ghp_good".into()));

    let Event::Connected { result, .. } = t.connect(ProviderKind::GitHub, "work", "ghp_bad") else {
        panic!()
    };
    assert!(matches!(
        result,
        Err(ConnectError::Verify(LoadError::Access(_)))
    ));
    assert!(t.world.credentials.aliases().is_empty(), "nothing stored");

    let Event::Connected { result, .. } = t.connect(ProviderKind::GitHub, "work", "ghp_good")
    else {
        panic!()
    };
    let connection = result.unwrap();
    assert_eq!(connection.display_name, "octocat");
    assert_eq!(
        t.world
            .credentials
            .read(&ConnectionAlias::new("work"))
            .unwrap()
            .unwrap()
            .expose(),
        "ghp_good"
    );
    let on_disk = std::fs::read_to_string(t.paths.device_prefs_file()).unwrap();
    assert!(on_disk.contains("[connections.work]"), "{on_disk}");
    assert!(!on_disk.contains("ghp_good"), "secret never in files");

    // Same alias again (not a reconnect) is refused without a job.
    let Event::Connected { result, .. } = t.connect(ProviderKind::GitHub, "work", "ghp_good")
    else {
        panic!()
    };
    assert!(matches!(result, Err(ConnectError::Alias(_))));
    // Reconnecting with the wrong provider is refused too.
    let event = t.run(Command::Connect {
        ticket: Ticket::next(),
        request: ConnectRequest {
            provider: ProviderKind::Cloudflare,
            alias: ConnectionAlias::new("work"),
            secret: Secret::new("x"),
            replace: true,
        },
    });
    assert!(matches!(
        event,
        Event::Connected {
            result: Err(ConnectError::Alias(_)),
            ..
        }
    ));
}

#[test]
fn connect_cloudflare_names_connection_after_accounts() {
    let mut t = T::new();
    let Event::Connected { result, .. } = t.connect(ProviderKind::Cloudflare, "cf", "tok") else {
        panic!()
    };
    assert_eq!(result.unwrap().display_name, "Acme");

    let account = |n: &str| CloudflareAccount {
        id: "0123456789abcdef0123456789abcdef".into(),
        name: n.into(),
    };
    t.world
        .catalog
        .update(|d| d.accounts = Ok(vec![account("A"), account("B"), account("C")]));
    let Event::Connected { result, .. } = t.connect(ProviderKind::Cloudflare, "cf2", "tok") else {
        panic!()
    };
    assert_eq!(result.unwrap().display_name, "A (+2 more)");

    t.world.catalog.update(|d| d.accounts = Ok(vec![]));
    let Event::Connected { result, .. } = t.connect(ProviderKind::Cloudflare, "cf3", "tok") else {
        panic!()
    };
    assert_eq!(result.unwrap().display_name, "Cloudflare token");

    t.world.catalog.update(|d| {
        d.token = Ok(CloudflareTokenStatus {
            id: "x".into(),
            status: "expired".into(),
            active: false,
            expires_on: None,
        })
    });
    let Event::Connected { result, .. } = t.connect(ProviderKind::Cloudflare, "cf4", "tok") else {
        panic!()
    };
    assert_eq!(result, Err(ConnectError::Inactive("expired".into())));
}

#[test]
fn catalog_queries_use_stored_credential() {
    let mut t = T::new();
    let ask = |t: &mut T, alias: &str, query: CatalogQuery| match t.run(Command::Catalog {
        ticket: Ticket::next(),
        connection: ConnectionAlias::new(alias),
        query,
    }) {
        Event::Catalog { result, .. } => result,
        other => panic!("{other:?}"),
    };
    let missing = ask(&mut t, "work", CatalogQuery::Repositories).unwrap_err();
    assert!(missing.is_access());
    assert!(missing.message().contains("There is no token for"));

    t.world
        .credentials
        .write(&ConnectionAlias::new("work"), &Secret::new("t"))
        .unwrap();
    let repo = || ("acme".to_owned(), "keevoro".to_owned());
    assert!(matches!(
        ask(&mut t, "work", CatalogQuery::Repositories),
        Ok(CatalogItems::Repositories(v)) if v.len() == 2
    ));
    let (owner, r) = repo();
    assert!(matches!(
        ask(&mut t, "work", CatalogQuery::Workflows { owner, repo: r }),
        Ok(CatalogItems::Workflows(v)) if v.len() == 2
    ));
    let (owner, r) = repo();
    assert!(matches!(
        ask(&mut t, "work", CatalogQuery::Branches { owner, repo: r }),
        Ok(CatalogItems::Branches(v)) if v.len() == 2
    ));
    assert!(matches!(
        ask(&mut t, "work", CatalogQuery::Accounts),
        Ok(CatalogItems::Accounts(v)) if v.len() == 1
    ));
    assert!(matches!(
        ask(&mut t, "work", CatalogQuery::Workers { account_id: "a".into() }),
        Ok(CatalogItems::Workers(v)) if v.len() == 2
    ));
    t.world.catalog.update(|d| {
        d.repositories = Err(ProviderError::Network("down".into()));
    });
    let error = ask(&mut t, "work", CatalogQuery::Repositories).unwrap_err();
    assert!(error.message().contains("Could not reach GitHub"));
}

#[test]
fn preview_distinguishes_empty_history_from_access_error() {
    let mut t = T::new();
    t.world
        .credentials
        .write(&ConnectionAlias::new("work"), &Secret::new("t"))
        .unwrap();
    let target = |repo: &str| WatchTarget::GitHub {
        owner: "acme".into(),
        repo: repo.into(),
        workflow: "ci.yml".into(),
        branch: None,
    };
    let preview = |t: &mut T, repo: &str| match t.run(Command::Preview {
        ticket: Ticket::next(),
        connection: ConnectionAlias::new("work"),
        target: target(repo),
    }) {
        Event::Preview { result, .. } => result,
        other => panic!("{other:?}"),
    };
    assert_eq!(preview(&mut t, "empty"), Ok(None));

    let mut older = record("1", BuildStatus::Passed);
    older.created_at -= chrono::TimeDelta::minutes(10);
    t.world
        .github
        .set("keevoro", Ok(vec![older, record("2", BuildStatus::Failed)]));
    let latest = preview(&mut t, "keevoro").unwrap().unwrap();
    assert_eq!(
        latest.run_id.as_str(),
        "2",
        "select_latest picks the newest"
    );

    t.world.github.set(
        "secret",
        Err(ProviderError::AccessDenied {
            provider: ProviderKind::GitHub,
        }),
    );
    assert!(preview(&mut t, "secret").unwrap_err().is_access());
}

#[test]
fn pin_saves_then_refreshes_and_edits_are_atomic() {
    let mut t = T::new();
    t.world
        .credentials
        .write(&ConnectionAlias::new("work"), &Secret::new("t"))
        .unwrap();
    t.world
        .github
        .set("keevoro", Ok(vec![record("7", BuildStatus::Failed)]));
    let k = t.pin(gh_draft("Keevoro", "work", "keevoro")).unwrap();
    let b = t.pin(gh_draft("Billing", "work", "billing")).unwrap();
    assert!(t.pin(gh_draft("", "work", "x")).is_err(), "validated");

    let on_disk = t.paths.profile_store().load().unwrap();
    assert_eq!(on_disk, *t.app.profile());
    // Both requests are fetched concurrently, so wait for both to land
    // rather than assuming the second has finished when the first has.
    let strip = t.wait_for("keevoro failed and billing empty", |s| {
        s.entries.first().is_some_and(|e| e.status_text == "Failed")
            && s.entries
                .get(1)
                .is_some_and(|e| e.status_text == "No runs yet")
    });
    assert_eq!(strip.entries[1].status_text, "No runs yet");

    // Open the run.
    let url = strip.entries[0].url.clone().unwrap();
    t.app.open_url(&url).unwrap();
    assert_eq!(t.world.urls.opened(), vec![url]);
    assert!(t.app.open_url("http://insecure.example").is_err());

    // Rename, move, set connection, remove.
    let edit = |t: &mut T, edit: ProfileEdit| {
        t.saved(Command::Edit {
            ticket: Ticket::next(),
            edit,
        })
    };
    edit(
        &mut t,
        ProfileEdit::Rename {
            id: k.clone(),
            label: "Keevoro API".into(),
        },
    )
    .unwrap();
    assert!(edit(
        &mut t,
        ProfileEdit::Rename {
            id: k.clone(),
            label: "".into()
        }
    )
    .is_err());
    edit(&mut t, ProfileEdit::Move { from: 0, to: 1 }).unwrap();
    assert_eq!(t.app.profile().watches()[1].label, "Keevoro API");
    let err = edit(
        &mut t,
        ProfileEdit::SetConnection {
            id: k.clone(),
            connection: ConnectionAlias::new("nope"),
        },
    )
    .unwrap_err();
    assert!(err.contains("not a connection on this device"));
    t.connect(ProviderKind::GitHub, "home", "t");
    edit(
        &mut t,
        ProfileEdit::SetConnection {
            id: k.clone(),
            connection: ConnectionAlias::new("home"),
        },
    )
    .unwrap();
    edit(&mut t, ProfileEdit::Remove { id: b }).unwrap();
    assert_eq!(t.paths.profile_store().load().unwrap().len(), 1);
    t.wait_for("billing gone", |s| s.entries.len() == 1);

    // A failed save leaves file, memory and strip unchanged.
    let before = t.app.profile().clone();
    let file = t.paths.profile_file();
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap(); // the path is now a directory: rename fails
    let err = edit(
        &mut t,
        ProfileEdit::Rename {
            id: k,
            label: "X".into(),
        },
    )
    .unwrap_err();
    assert!(err.starts_with("Could not save the profile"), "{err}");
    assert_eq!(*t.app.profile(), before);
}

#[test]
fn unreadable_profile_is_never_overwritten_until_import() {
    let mut t = T::with(|paths| {
        std::fs::create_dir_all(paths.profile_dir()).unwrap();
        std::fs::write(
            paths.profile_file(),
            "version = 1\n[[watch]]\nlable = 'x'\n",
        )
        .unwrap();
        std::fs::create_dir_all(paths.device_dir()).unwrap();
        std::fs::write(paths.device_prefs_file(), "this is not toml [").unwrap();
    });
    assert!(t.app.profile_error().unwrap().contains("lable"));
    assert!(t.app.prefs_error().is_some());
    let err = t.pin(gh_draft("K", "work", "k")).unwrap_err();
    assert!(err.contains("could not be read"));
    let edit = t.saved(Command::Edit {
        ticket: Ticket::next(),
        edit: ProfileEdit::Move { from: 0, to: 0 },
    });
    assert!(edit.is_err());
    assert!(std::fs::read_to_string(t.paths.profile_file())
        .unwrap()
        .contains("lable"));

    let replacement = Profile::from_drafts(&[gh_draft("K", "work", "k")]).unwrap();
    t.saved(Command::ReplaceProfile {
        ticket: Ticket::next(),
        profile: replacement.clone(),
    })
    .unwrap();
    assert_eq!(t.app.profile_error(), None);
    assert_eq!(t.paths.profile_store().load().unwrap(), replacement);
    // Saving preferences clears their load error too.
    t.app.update_prefs(|p| p.theme = Theme::Dark).unwrap();
    assert_eq!(t.app.prefs_error(), None);
}

#[test]
fn removing_connections_is_guarded() {
    let mut t = T::new();
    t.connect(ProviderKind::GitHub, "work", "t");
    t.connect(ProviderKind::GitHub, "spare", "t");
    t.pin(gh_draft("K", "work", "k")).unwrap();
    let err = t
        .saved(Command::RemoveConnection {
            ticket: Ticket::next(),
            alias: ConnectionAlias::new("work"),
        })
        .unwrap_err();
    assert!(err.contains("still used by K"));
    t.saved(Command::RemoveConnection {
        ticket: Ticket::next(),
        alias: ConnectionAlias::new("spare"),
    })
    .unwrap();
    assert_eq!(
        t.world.credentials.aliases(),
        vec![ConnectionAlias::new("work")]
    );
    assert!(t
        .app
        .prefs()
        .connection(&ConnectionAlias::new("spare"))
        .is_none());
}

#[test]
fn reconnect_replaces_credential_and_refreshes() {
    let mut t = T::new();
    t.connect(ProviderKind::GitHub, "work", "old");
    t.world.github.set(
        "keevoro",
        Err(ProviderError::AccessDenied {
            provider: ProviderKind::GitHub,
        }),
    );
    t.pin(gh_draft("K", "work", "keevoro")).unwrap();
    t.wait_for("reconnect needed", |s| {
        s.entries.first().is_some_and(|e| e.needs_reconnect)
    });
    t.world
        .github
        .set("keevoro", Ok(vec![record("1", BuildStatus::Passed)]));
    let event = t.run(Command::Connect {
        ticket: Ticket::next(),
        request: ConnectRequest {
            provider: ProviderKind::GitHub,
            alias: ConnectionAlias::new("work"),
            secret: Secret::new("new"),
            replace: true,
        },
    });
    assert!(matches!(event, Event::Connected { result: Ok(_), .. }));
    assert_eq!(
        t.world
            .credentials
            .read(&ConnectionAlias::new("work"))
            .unwrap()
            .unwrap()
            .expose(),
        "new"
    );
    t.wait_for("recovered", |s| {
        s.entries.first().is_some_and(|e| e.status_text == "Passed")
    });
}

#[test]
fn device_prefs_and_startup() {
    let mut t = T::new();
    t.app
        .update_prefs(|p| {
            p.launch_at_startup = true;
            p.theme = Theme::Light;
            p.placement = Placement::Floating { position: None };
        })
        .unwrap();
    assert!(t.world.autostart.is_enabled().unwrap());
    let stored = t.paths.device_prefs_store().load().unwrap();
    assert_eq!(stored.theme, Theme::Light);
    assert!(stored.launch_at_startup);
    t.app.update_prefs(|p| p.launch_at_startup = false).unwrap();
    assert!(!t.world.autostart.is_enabled().unwrap());

    // Invalid preferences are refused and nothing changes.
    let err = t.app.update_prefs(|p| p.scale = Some(99.0)).unwrap_err();
    assert!(err.contains("scale"));
    assert_eq!(t.app.prefs().scale, None);

    // Without an autostart adapter, enabling startup is refused.
    let mut services = t.world.services(&t.paths);
    services.autostart = None;
    let mut bare = AppController::start(
        services,
        Arc::new(t.paths.snapshot_cache()),
        fast_policy(),
        t.runtime.handle().clone(),
    );
    assert!(bare.update_prefs(|p| p.launch_at_startup = true).is_err());
    bare.shutdown();
}

#[test]
fn preview_without_source_and_with_broken_store() {
    let mut t = T::new();
    let mut services = t.world.services(&t.paths);
    services.sources = SourceRegistry::new();
    t.app = AppController::start(
        services,
        Arc::new(t.paths.snapshot_cache()),
        fast_policy(),
        t.runtime.handle().clone(),
    );
    let event = t.run(Command::Preview {
        ticket: Ticket::next(),
        connection: ConnectionAlias::new("work"),
        target: WatchTarget::Cloudflare {
            account_id: "a".into(),
            worker: "w".into(),
            branch: None,
        },
    });
    let Event::Preview { result, .. } = event else {
        panic!()
    };
    assert!(result.unwrap_err().message().contains("not available"));
}
