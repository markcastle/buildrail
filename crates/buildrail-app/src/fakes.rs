//! In-memory implementations of the provider ports.
//!
//! They are part of the library (not `#[cfg(test)]`) because three callers
//! need them: unit tests, the window-level UI tests in `tests/`, and the
//! `--demo` mode, which runs the real UI with no network and no keyring.
//! The platform crate already ships fakes for its own ports (credential
//! store, dock, monitors, URL opener, autostart); these complete the set
//! for the provider side.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use buildrail_core::domain::{BuildRecord, ProviderKind, WatchTarget};
use buildrail_core::ports::{Clock, ProviderError, RunSource, Secret};
use buildrail_core::refresh::SourceRegistry;
use buildrail_platform::autostart::InMemoryAutostart;
use buildrail_platform::credentials::InMemoryCredentialStore;
use buildrail_platform::folders::RecordingFolderOpener;
use buildrail_platform::url_opener::RecordingUrlOpener;
use buildrail_profile::AppPaths;

use crate::app::{Catalogs, FileDialog, Services};
use buildrail_providers::catalog::{
    CloudflareAccount, CloudflareAccounts, CloudflareTokenCheck, CloudflareTokenStatus,
    CloudflareWorker, CloudflareWorkers, GitHubBranch, GitHubBranches, GitHubIdentity,
    GitHubRepositories, GitHubRepository, GitHubUser, GitHubWorkflow, GitHubWorkflows,
};
use chrono::{DateTime, TimeDelta, Utc};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A wall clock set by hand.
#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<DateTime<Utc>>,
}

impl ManualClock {
    /// A clock showing `at`.
    pub fn new(at: DateTime<Utc>) -> Self {
        Self {
            now: Mutex::new(at),
        }
    }

    /// Moves the clock forward (or back, with a negative delta).
    pub fn advance(&self, by: TimeDelta) {
        *lock(&self.now) += by;
    }

    /// Sets the time.
    pub fn set(&self, at: DateTime<Utc>) {
        *lock(&self.now) = at;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *lock(&self.now)
    }
}

/// The name a [`ScriptedRunSource`] keys replies by: the repository for
/// GitHub targets, the Worker for Cloudflare targets.
pub fn target_name(target: &WatchTarget) -> &str {
    match target {
        WatchTarget::GitHub { repo, .. } => repo,
        WatchTarget::Cloudflare { worker, .. } => worker,
    }
}

/// One scripted answer.
pub type Reply = Result<Vec<BuildRecord>, ProviderError>;

/// A [`RunSource`] answering from per-target scripts. Each target has a
/// queue of replies; the last one repeats forever. Unknown targets get an
/// empty history.
#[derive(Debug)]
pub struct ScriptedRunSource {
    kind: ProviderKind,
    replies: Mutex<HashMap<String, VecDeque<Reply>>>,
    calls: Mutex<Vec<String>>,
}

impl ScriptedRunSource {
    /// A source for `kind` with no scripts.
    pub fn new(kind: ProviderKind) -> Self {
        Self {
            kind,
            replies: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Replaces the script for `name` with one reply that repeats.
    pub fn set(&self, name: &str, reply: Reply) {
        lock(&self.replies).insert(name.to_owned(), VecDeque::from([reply]));
    }

    /// Appends a reply to the script for `name`.
    pub fn push(&self, name: &str, reply: Reply) {
        lock(&self.replies)
            .entry(name.to_owned())
            .or_default()
            .push_back(reply);
    }

    /// Target names asked for so far, in order.
    pub fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }
}

#[async_trait]
impl RunSource for ScriptedRunSource {
    fn provider(&self) -> ProviderKind {
        self.kind
    }

    async fn recent_runs(
        &self,
        target: &WatchTarget,
        _credential: &Secret,
    ) -> Result<Vec<BuildRecord>, ProviderError> {
        let name = target_name(target).to_owned();
        lock(&self.calls).push(name.clone());
        let mut replies = lock(&self.replies);
        match replies.get_mut(&name) {
            Some(queue) if queue.len() > 1 => queue.pop_front().unwrap_or(Ok(Vec::new())),
            Some(queue) => queue.front().cloned().unwrap_or(Ok(Vec::new())),
            None => Ok(Vec::new()),
        }
    }
}

/// What a [`FakeCatalog`] answers. Each field is the answer to one
/// catalogue call; set an `Err` to simulate a failure.
#[derive(Debug, Clone)]
pub struct CatalogData {
    /// The only token accepted by both providers; any other token is
    /// refused with `AccessDenied`. `None` accepts every token.
    pub valid_token: Option<String>,
    /// `GET /user`.
    pub github_user: Result<GitHubUser, ProviderError>,
    /// Repositories.
    pub repositories: Result<Vec<GitHubRepository>, ProviderError>,
    /// Workflows, for any repository.
    pub workflows: Result<Vec<GitHubWorkflow>, ProviderError>,
    /// Branches, for any repository.
    pub branches: Result<Vec<GitHubBranch>, ProviderError>,
    /// Cloudflare token verification.
    pub token: Result<CloudflareTokenStatus, ProviderError>,
    /// Cloudflare accounts.
    pub accounts: Result<Vec<CloudflareAccount>, ProviderError>,
    /// Workers, for any account.
    pub workers: Result<Vec<CloudflareWorker>, ProviderError>,
}

impl Default for CatalogData {
    /// A small, plausible catalogue: one GitHub user with two repositories,
    /// one Cloudflare account with two Workers.
    fn default() -> Self {
        let repo = |name: &str| GitHubRepository {
            owner: "acme".into(),
            name: name.into(),
            full_name: format!("acme/{name}"),
            private: true,
            archived: false,
            default_branch: Some("main".into()),
        };
        let worker = |name: &str| CloudflareWorker {
            name: name.into(),
            tag: None,
            modified_on: None,
        };
        Self {
            valid_token: None,
            github_user: Ok(GitHubUser {
                login: "octocat".into(),
                name: Some("Mona".into()),
            }),
            repositories: Ok(vec![repo("keevoro"), repo("billing")]),
            workflows: Ok(vec![
                GitHubWorkflow {
                    id: 1,
                    name: "CI".into(),
                    path: ".github/workflows/ci.yml".into(),
                    state: "active".into(),
                },
                GitHubWorkflow {
                    id: 2,
                    name: "Release".into(),
                    path: ".github/workflows/release.yml".into(),
                    state: "active".into(),
                },
            ]),
            branches: Ok(vec![
                GitHubBranch {
                    name: "main".into(),
                    protected: true,
                },
                GitHubBranch {
                    name: "develop".into(),
                    protected: false,
                },
            ]),
            token: Ok(CloudflareTokenStatus {
                id: "tok123".into(),
                status: "active".into(),
                active: true,
                expires_on: None,
            }),
            accounts: Ok(vec![CloudflareAccount {
                id: "0123456789abcdef0123456789abcdef".into(),
                name: "Acme".into(),
            }]),
            workers: Ok(vec![worker("marketing-site"), worker("api-gateway")]),
        }
    }
}

/// An in-memory catalogue for both providers.
#[derive(Debug, Default)]
pub struct FakeCatalog {
    data: Mutex<CatalogData>,
}

impl FakeCatalog {
    /// A catalogue answering `data`.
    pub fn new(data: CatalogData) -> Self {
        Self {
            data: Mutex::new(data),
        }
    }

    /// Changes the answers.
    pub fn update(&self, change: impl FnOnce(&mut CatalogData)) {
        change(&mut lock(&self.data));
    }

    fn answer<T: Clone>(
        &self,
        provider: ProviderKind,
        credential: &Secret,
        pick: impl FnOnce(&CatalogData) -> &Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let data = lock(&self.data);
        if let Some(valid) = &data.valid_token {
            if credential.expose() != valid {
                return Err(ProviderError::AccessDenied { provider });
            }
        }
        pick(&data).clone()
    }
}

#[async_trait]
impl GitHubIdentity for FakeCatalog {
    async fn current_user(&self, credential: &Secret) -> Result<GitHubUser, ProviderError> {
        self.answer(ProviderKind::GitHub, credential, |d| &d.github_user)
    }
}

#[async_trait]
impl GitHubRepositories for FakeCatalog {
    async fn repositories(
        &self,
        credential: &Secret,
    ) -> Result<Vec<GitHubRepository>, ProviderError> {
        self.answer(ProviderKind::GitHub, credential, |d| &d.repositories)
    }
}

#[async_trait]
impl GitHubWorkflows for FakeCatalog {
    async fn workflows(
        &self,
        _owner: &str,
        _repo: &str,
        credential: &Secret,
    ) -> Result<Vec<GitHubWorkflow>, ProviderError> {
        self.answer(ProviderKind::GitHub, credential, |d| &d.workflows)
    }
}

#[async_trait]
impl GitHubBranches for FakeCatalog {
    async fn branches(
        &self,
        _owner: &str,
        _repo: &str,
        credential: &Secret,
    ) -> Result<Vec<GitHubBranch>, ProviderError> {
        self.answer(ProviderKind::GitHub, credential, |d| &d.branches)
    }
}

#[async_trait]
impl CloudflareTokenCheck for FakeCatalog {
    async fn verify_token(
        &self,
        credential: &Secret,
    ) -> Result<CloudflareTokenStatus, ProviderError> {
        self.answer(ProviderKind::Cloudflare, credential, |d| &d.token)
    }
}

#[async_trait]
impl CloudflareAccounts for FakeCatalog {
    async fn accounts(&self, credential: &Secret) -> Result<Vec<CloudflareAccount>, ProviderError> {
        self.answer(ProviderKind::Cloudflare, credential, |d| &d.accounts)
    }
}

#[async_trait]
impl CloudflareWorkers for FakeCatalog {
    async fn workers(
        &self,
        _account_id: &str,
        credential: &Secret,
    ) -> Result<Vec<CloudflareWorker>, ProviderError> {
        self.answer(ProviderKind::Cloudflare, credential, |d| &d.workers)
    }
}

/// Open and save dialogs that answer from a script: each call takes the
/// next queued answer (`None` = the person cancelled), and cancels once
/// the script runs out. Records what was asked.
#[derive(Debug, Default)]
pub struct ScriptedFileDialog {
    answers: Mutex<VecDeque<Option<PathBuf>>>,
    asked: Mutex<Vec<String>>,
}

impl ScriptedFileDialog {
    /// Cancels every dialog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues the answer to the next dialog.
    pub fn answer(&self, path: Option<PathBuf>) {
        lock(&self.answers).push_back(path);
    }

    /// What was asked so far: `"open"`, or `"save <suggested name>"`.
    pub fn asked(&self) -> Vec<String> {
        lock(&self.asked).clone()
    }

    fn next(&self, what: String) -> Option<PathBuf> {
        lock(&self.asked).push(what);
        lock(&self.answers).pop_front().flatten()
    }
}

impl FileDialog for ScriptedFileDialog {
    fn open_profile(&self) -> Option<PathBuf> {
        self.next("open".to_owned())
    }

    fn save_profile(&self, suggested_name: &str) -> Option<PathBuf> {
        self.next(format!("save {suggested_name}"))
    }
}

/// Every port faked, with handles kept so a test can script providers,
/// inspect stored credentials and opened URLs, and move the clock.
#[derive(Clone)]
pub struct FakeWorld {
    /// GitHub run source.
    pub github: Arc<ScriptedRunSource>,
    /// Cloudflare run source.
    pub cloudflare: Arc<ScriptedRunSource>,
    /// Both catalogues.
    pub catalog: Arc<FakeCatalog>,
    /// Credential store.
    pub credentials: Arc<InMemoryCredentialStore>,
    /// Wall clock.
    pub clock: Arc<ManualClock>,
    /// Records opened URLs.
    pub urls: Arc<RecordingUrlOpener>,
    /// Launch-at-login.
    pub autostart: Arc<InMemoryAutostart>,
    /// Open and save dialogs.
    pub dialogs: Arc<ScriptedFileDialog>,
    /// Records opened folders.
    pub folders: Arc<RecordingFolderOpener>,
}

impl FakeWorld {
    /// A world whose clock shows `now`, with the default catalogue and no
    /// scripted runs.
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            github: Arc::new(ScriptedRunSource::new(ProviderKind::GitHub)),
            cloudflare: Arc::new(ScriptedRunSource::new(ProviderKind::Cloudflare)),
            catalog: Arc::new(FakeCatalog::default()),
            credentials: Arc::new(InMemoryCredentialStore::new()),
            clock: Arc::new(ManualClock::new(now)),
            urls: Arc::new(RecordingUrlOpener::new()),
            autostart: Arc::new(InMemoryAutostart::new(false)),
            dialogs: Arc::new(ScriptedFileDialog::new()),
            folders: Arc::new(RecordingFolderOpener::new()),
        }
    }

    /// The controller's ports, storing files under `paths`.
    pub fn services(&self, paths: &AppPaths) -> Services {
        let sources = SourceRegistry::new()
            .with(self.github.clone())
            .with(self.cloudflare.clone());
        Services {
            sources,
            catalogs: Catalogs::from_pair(self.catalog.clone(), self.catalog.clone()),
            credentials: self.credentials.clone(),
            clock: self.clock.clone(),
            urls: self.urls.clone(),
            autostart: Some(self.autostart.clone()),
            profile_store: paths.profile_store(),
            prefs_store: paths.device_prefs_store(),
            dialogs: self.dialogs.clone(),
            folders: self.folders.clone(),
            logs_dir: Some(paths.logs_dir().to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use buildrail_core::domain::{BuildStatus, RunId};
    use chrono::TimeZone;

    use super::*;

    fn gh(repo: &str) -> WatchTarget {
        WatchTarget::GitHub {
            owner: "acme".into(),
            repo: repo.into(),
            workflow: "ci.yml".into(),
            branch: None,
        }
    }

    fn record(id: &str) -> BuildRecord {
        let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        BuildRecord {
            run_id: RunId::new(id),
            attempt: 1,
            status: BuildStatus::Passed,
            url: "https://example.com".into(),
            branch: None,
            commit: None,
            failing_job: None,
            created_at: at,
            updated_at: at,
            observed_at: at,
        }
    }

    #[tokio::test]
    async fn scripted_source_plays_queue_then_repeats_last() {
        let source = ScriptedRunSource::new(ProviderKind::GitHub);
        assert_eq!(source.provider(), ProviderKind::GitHub);
        let s = Secret::new("t");
        assert!(source.recent_runs(&gh("x"), &s).await.unwrap().is_empty());
        source.push("x", Ok(vec![record("1")]));
        source.push("x", Err(ProviderError::Network("down".into())));
        assert_eq!(source.recent_runs(&gh("x"), &s).await.unwrap().len(), 1);
        assert!(source.recent_runs(&gh("x"), &s).await.is_err());
        assert!(
            source.recent_runs(&gh("x"), &s).await.is_err(),
            "last repeats"
        );
        source.set("x", Ok(vec![]));
        assert!(source.recent_runs(&gh("x"), &s).await.unwrap().is_empty());
        assert_eq!(source.calls().len(), 5);
        let cf = WatchTarget::Cloudflare {
            account_id: "a".into(),
            worker: "w".into(),
            branch: None,
        };
        assert_eq!(target_name(&cf), "w");
    }

    #[tokio::test]
    async fn catalog_checks_token_and_answers() {
        let catalog = FakeCatalog::new(CatalogData {
            valid_token: Some("good".into()),
            ..CatalogData::default()
        });
        let good = Secret::new("good");
        let bad = Secret::new("bad");
        assert_eq!(catalog.current_user(&good).await.unwrap().login, "octocat");
        assert!(matches!(
            catalog.current_user(&bad).await,
            Err(ProviderError::AccessDenied { .. })
        ));
        assert_eq!(catalog.repositories(&good).await.unwrap().len(), 2);
        assert_eq!(catalog.workflows("a", "b", &good).await.unwrap().len(), 2);
        assert_eq!(catalog.branches("a", "b", &good).await.unwrap().len(), 2);
        assert!(catalog.verify_token(&good).await.unwrap().active);
        assert_eq!(catalog.accounts(&good).await.unwrap().len(), 1);
        assert_eq!(catalog.workers("a", &good).await.unwrap().len(), 2);
        catalog.update(|d| d.workers = Err(ProviderError::Network("x".into())));
        assert!(catalog.workers("a", &good).await.is_err());
        let _ = FakeCatalog::default();
    }

    #[test]
    fn manual_clock() {
        let start = Utc.timestamp_opt(0, 0).unwrap();
        let clock = ManualClock::new(start);
        clock.advance(TimeDelta::seconds(5));
        assert_eq!(clock.now(), start + TimeDelta::seconds(5));
        clock.set(start);
        assert_eq!(clock.now(), start);
    }
}
