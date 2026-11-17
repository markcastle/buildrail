//! The bundle of ports the controller depends on.
//!
//! Everything the app does to the outside world goes through one of these
//! trait objects, so the composition root (`compose.rs`) decides between
//! real adapters, the `--demo` fakes and test fakes, and the controller
//! cannot tell the difference.

use std::path::PathBuf;
use std::sync::Arc;

use buildrail_core::ports::{Clock, CredentialReader, CredentialWriter};
use buildrail_core::refresh::SourceRegistry;
use buildrail_platform::autostart::Autostart;
use buildrail_platform::folders::FolderOpener;
use buildrail_platform::url_opener::UrlOpener;
use buildrail_profile::{DevicePrefsStore, ProfileStore};
use buildrail_providers::catalog::{
    CloudflareAccounts, CloudflareTokenCheck, CloudflareWorkers, GitHubBranches, GitHubIdentity,
    GitHubRepositories, GitHubWorkflows,
};

/// Read *and* write access to the OS credential store. Settings needs
/// both; the refresh engine gets the same object as a plain
/// [`CredentialReader`].
pub trait CredentialStore: CredentialReader + CredentialWriter {}

impl<T: CredentialReader + CredentialWriter + ?Sized> CredentialStore for T {}

/// Asks the person for a file with the system's own open and save
/// dialogs. Blocking: the dialog is modal and answers when it closes.
/// `None` means they cancelled (or no dialog could be shown; the Share
/// page keeps a path field as the fallback).
pub trait FileDialog: Send + Sync {
    /// "Which profile do you want to import?"
    fn open_profile(&self) -> Option<PathBuf>;
    /// "Where do you want to save your profile?", suggesting
    /// `suggested_name`.
    fn save_profile(&self, suggested_name: &str) -> Option<PathBuf>;
}

/// The catalogue ports the Add-build wizard browses with, one per
/// question (interface segregation: tests fake only what they use).
#[derive(Clone)]
pub struct Catalogs {
    /// Verifies GitHub tokens ("who am I").
    pub github_identity: Arc<dyn GitHubIdentity>,
    /// Lists GitHub repositories.
    pub github_repositories: Arc<dyn GitHubRepositories>,
    /// Lists workflows of a repository.
    pub github_workflows: Arc<dyn GitHubWorkflows>,
    /// Lists branches of a repository.
    pub github_branches: Arc<dyn GitHubBranches>,
    /// Verifies Cloudflare tokens.
    pub cloudflare_token: Arc<dyn CloudflareTokenCheck>,
    /// Lists Cloudflare accounts.
    pub cloudflare_accounts: Arc<dyn CloudflareAccounts>,
    /// Lists Workers of an account.
    pub cloudflare_workers: Arc<dyn CloudflareWorkers>,
}

impl Catalogs {
    /// Uses one object for every GitHub question and one for every
    /// Cloudflare question (what the real adapters and most fakes are).
    pub fn from_pair<G, C>(github: Arc<G>, cloudflare: Arc<C>) -> Self
    where
        G: GitHubIdentity + GitHubRepositories + GitHubWorkflows + GitHubBranches + 'static,
        C: CloudflareTokenCheck + CloudflareAccounts + CloudflareWorkers + 'static,
    {
        Self {
            github_identity: github.clone(),
            github_repositories: github.clone(),
            github_workflows: github.clone(),
            github_branches: github,
            cloudflare_token: cloudflare.clone(),
            cloudflare_accounts: cloudflare.clone(),
            cloudflare_workers: cloudflare,
        }
    }
}

/// Everything the controller needs from the outside world.
#[derive(Clone)]
pub struct Services {
    /// Run sources per provider, shared by the refresh engine and the
    /// wizard's preview.
    pub sources: SourceRegistry,
    /// Catalogue ports for the wizard.
    pub catalogs: Catalogs,
    /// The OS credential store (or a fake).
    pub credentials: Arc<dyn CredentialStore>,
    /// Wall clock.
    pub clock: Arc<dyn Clock>,
    /// Opens run pages in the browser.
    pub urls: Arc<dyn UrlOpener>,
    /// Launch-at-login, when the platform supports it.
    pub autostart: Option<Arc<dyn Autostart>>,
    /// Where the profile lives.
    pub profile_store: ProfileStore,
    /// Where this device's preferences live.
    pub prefs_store: DevicePrefsStore,
    /// Open and save dialogs (Share page).
    pub dialogs: Arc<dyn FileDialog>,
    /// Shows a folder in the file manager (the log folder).
    pub folders: Arc<dyn FolderOpener>,
    /// Where log files are written, when they are.
    pub logs_dir: Option<PathBuf>,
}
