//! The application service: owns the profile, the device preferences and
//! the refresh engine handle, and executes settings [`Command`]s.
//!
//! # Threads
//!
//! The controller lives on the UI thread and never blocks it:
//!
//! * provider calls and OS credential-store calls (which block, so they go
//!   through `spawn_blocking`) run as jobs on the tokio runtime; each job
//!   ends by sending its result over a channel and requesting a repaint;
//! * [`AppController::poll_events`] (called once per frame) drains that
//!   channel, applies controller-side effects (registering a connection,
//!   triggering a refresh) and returns the [`Event`]s for the settings
//!   machines;
//! * profile and preference writes are small atomic file replacements and
//!   run inline; their results are queued for the next poll so every
//!   command answers the same way.
//!
//! # Consistency
//!
//! Every profile change follows *save, then adopt, then `set_watches`*: the
//! new profile is built purely, written with the atomic store, and only
//! after a successful write does it replace the in-memory profile and
//! reach the refresh engine. A failed write leaves file, memory and strip
//! exactly as they were.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use buildrail_core::domain::{ConnectionAlias, ProviderKind, WatchTarget};
use buildrail_core::ports::{CredentialReader, Secret, SnapshotCache};
use buildrail_core::refresh::{RefreshEngine, RefreshHandle, RefreshPolicy};
use buildrail_core::selection::select_latest;
use buildrail_profile::{DevicePrefs, LocalConnection, Profile, WatchDraft};
use tokio::runtime::Handle;

use super::ports::Services;
use crate::presentation::StripModel;
use crate::settings::{
    watches_using, CatalogItems, CatalogQuery, Command, ConnectError, ConnectRequest, Event,
    LoadError, NewConnection, ProfileEdit, Ticket,
};

/// Called (from any thread) when something changed that the UI should
/// draw: new snapshots or a finished job. The UI sets it to
/// `egui::Context::request_repaint`.
pub type RepaintFn = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Default)]
struct Repainter(Arc<Mutex<Option<RepaintFn>>>);

impl Repainter {
    fn set(&self, f: RepaintFn) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(f);
    }

    fn request(&self) {
        let f = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(f) = f {
            f();
        }
    }
}

/// What a job sends back. Most results are final [`Event`]s; connecting
/// and removing a connection still need controller-side bookkeeping on the
/// UI thread.
enum JobResult {
    Event(Event),
    Connected {
        ticket: Ticket,
        request: ConnectMeta,
        result: Result<String, ConnectError>,
    },
    ConnectionRemoved {
        ticket: Ticket,
        alias: ConnectionAlias,
        result: Result<(), String>,
    },
}

/// The parts of a [`ConnectRequest`] needed after the job (no secret).
struct ConnectMeta {
    provider: ProviderKind,
    alias: ConnectionAlias,
    replace: bool,
}

/// The application service. See the module docs.
pub struct AppController {
    services: Services,
    runtime: Handle,
    refresh: RefreshHandle,
    profile: Profile,
    prefs: DevicePrefs,
    profile_error: Option<String>,
    prefs_error: Option<String>,
    jobs_tx: mpsc::Sender<JobResult>,
    jobs_rx: mpsc::Receiver<JobResult>,
    pending_jobs: usize,
    ready: VecDeque<Event>,
    repaint: Repainter,
}

impl AppController {
    /// Loads the profile and preferences, starts the refresh engine on
    /// `runtime` and pins the profile's watches.
    ///
    /// A profile file that cannot be read is *not* overwritten: the app
    /// starts empty, [`profile_error`](Self::profile_error) explains why,
    /// and edits are refused until an import replaces it (or the user fixes
    /// the file and restarts).
    pub fn start(
        services: Services,
        cache: Arc<dyn SnapshotCache>,
        policy: RefreshPolicy,
        runtime: Handle,
    ) -> Self {
        let (profile, profile_error) = match services.profile_store.load() {
            Ok(profile) => (profile, None),
            Err(error) => {
                tracing::error!(%error, "could not load the profile");
                (Profile::empty(), Some(error.to_string()))
            }
        };
        let (prefs, prefs_error) = match services.prefs_store.load() {
            Ok(prefs) => (prefs, None),
            Err(error) => {
                tracing::warn!(%error, "could not load device preferences; using defaults");
                (DevicePrefs::default(), Some(error.to_string()))
            }
        };
        let reader: Arc<dyn CredentialReader> = services.credentials.clone();
        let (engine, refresh) = RefreshEngine::new(
            services.sources.clone(),
            reader,
            services.clock.clone(),
            cache,
            policy,
        );
        runtime.spawn(engine.run());

        let repaint = Repainter::default();
        let mut changes = refresh.subscribe();
        let on_change = repaint.clone();
        runtime.spawn(async move {
            while changes.changed().await.is_ok() {
                on_change.request();
            }
        });

        let (jobs_tx, jobs_rx) = mpsc::channel();
        let controller = Self {
            services,
            runtime,
            refresh,
            profile,
            prefs,
            profile_error,
            prefs_error,
            jobs_tx,
            jobs_rx,
            pending_jobs: 0,
            ready: VecDeque::new(),
            repaint,
        };
        controller.push_watches();
        controller
    }

    // ---------------------------------------------------------------- reads

    /// The saved profile.
    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// This device's preferences.
    pub fn prefs(&self) -> &DevicePrefs {
        &self.prefs
    }

    /// Why the profile file could not be loaded, if it could not.
    pub fn profile_error(&self) -> Option<&str> {
        self.profile_error.as_deref()
    }

    /// Why the preferences file could not be loaded, if it could not.
    pub fn prefs_error(&self) -> Option<&str> {
        self.prefs_error.as_deref()
    }

    /// The strip's view-model for this instant.
    pub fn strip(&self) -> StripModel {
        StripModel::build(
            self.profile.watches(),
            &self.refresh.snapshots(),
            self.services.clock.now(),
        )
    }

    /// The current wall-clock time, from the injected clock.
    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.services.clock.now()
    }

    /// The refresh engine handle.
    pub fn refresh_handle(&self) -> &RefreshHandle {
        &self.refresh
    }

    /// Number of jobs still running.
    pub fn pending_jobs(&self) -> usize {
        self.pending_jobs
    }

    /// Sets the repaint callback (the UI passes `request_repaint`).
    pub fn set_repaint(&self, repaint: RepaintFn) {
        self.repaint.set(repaint);
    }

    // -------------------------------------------------------------- actions

    /// Opens a run page. Only `https://` links are opened (the platform
    /// opener enforces it); failures are logged and returned for display.
    pub fn open_url(&self, url: &str) -> Result<(), String> {
        self.services.urls.open(url).map_err(|error| {
            tracing::warn!(%error, "could not open a run page");
            error.to_string()
        })
    }

    /// Refreshes everything now (user request, or after wake/reconnect).
    pub fn refresh_now(&self) {
        if self.refresh.refresh_now().is_err() {
            tracing::warn!("refresh requested after the engine stopped");
        }
    }

    /// Stops the refresh engine.
    pub fn shutdown(&self) {
        self.refresh.shutdown();
    }

    /// Changes device preferences: applies `change` to a copy, saves it
    /// atomically, then adopts it. Launch-at-startup is applied to the OS
    /// first, so the preference never claims something the OS refused.
    pub fn update_prefs(&mut self, change: impl FnOnce(&mut DevicePrefs)) -> Result<(), String> {
        let mut next = self.prefs.clone();
        change(&mut next);
        if next.launch_at_startup != self.prefs.launch_at_startup {
            let Some(autostart) = &self.services.autostart else {
                return Err("Starting at login is not available on this system.".to_owned());
            };
            let result = if next.launch_at_startup {
                autostart.enable()
            } else {
                autostart.disable()
            };
            result.map_err(|error| error.to_string())?;
        }
        self.services
            .prefs_store
            .save(&next)
            .map_err(|error| error.to_string())?;
        self.prefs = next;
        self.prefs_error = None;
        Ok(())
    }

    /// Executes a settings command. Results arrive through
    /// [`poll_events`](Self::poll_events).
    pub fn execute(&mut self, command: Command) {
        match command {
            Command::Connect { ticket, request } => self.connect(ticket, request),
            Command::Catalog {
                ticket,
                connection,
                query,
            } => self.catalog(ticket, connection, query),
            Command::Preview {
                ticket,
                connection,
                target,
            } => self.preview(ticket, connection, target),
            Command::Pin { ticket, draft } => {
                let result = self.pin(draft);
                self.ready.push_back(Event::Pinned { ticket, result });
            }
            Command::Edit { ticket, edit } => {
                let result = self.edit(edit);
                self.ready.push_back(Event::Saved { ticket, result });
            }
            Command::ReplaceProfile { ticket, profile } => {
                let result = self.replace_profile(profile);
                self.ready.push_back(Event::Saved { ticket, result });
            }
            Command::RemoveConnection { ticket, alias } => self.remove_connection(ticket, alias),
        }
        self.repaint.request();
    }

    /// Results that arrived since the last call, after controller-side
    /// effects. Call once per frame.
    pub fn poll_events(&mut self) -> Vec<Event> {
        while let Ok(result) = self.jobs_rx.try_recv() {
            self.pending_jobs = self.pending_jobs.saturating_sub(1);
            let event = self.finish(result);
            self.ready.push_back(event);
        }
        self.ready.drain(..).collect()
    }

    /// Waits (blocking) until every job has finished or `timeout` passed,
    /// then returns all events. For tests and headless drivers only: the UI
    /// uses [`poll_events`](Self::poll_events).
    pub fn settle(&mut self, timeout: Duration) -> Vec<Event> {
        let deadline = Instant::now() + timeout;
        while self.pending_jobs > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.jobs_rx.recv_timeout(left) {
                Ok(result) => {
                    self.pending_jobs -= 1;
                    let event = self.finish(result);
                    self.ready.push_back(event);
                }
                Err(_) => break,
            }
        }
        self.poll_events()
    }

    // ------------------------------------------------------------- profile

    fn push_watches(&self) {
        if self
            .refresh
            .set_watches(self.profile.watches().to_vec())
            .is_err()
        {
            tracing::warn!("watches changed after the engine stopped");
        }
    }

    fn guard_profile(&self) -> Result<(), String> {
        match &self.profile_error {
            Some(error) => Err(format!(
                "Your profile file could not be read, so it is not changed automatically. Import a profile to replace it, or fix the file and restart.\n{error}"
            )),
            None => Ok(()),
        }
    }

    /// Save, then adopt, then tell the engine.
    fn commit(&mut self, profile: Profile) -> Result<(), String> {
        self.services
            .profile_store
            .save(&profile)
            .map_err(|error| format!("Could not save the profile: {error}"))?;
        self.profile = profile;
        self.push_watches();
        Ok(())
    }

    fn pin(&mut self, draft: WatchDraft) -> Result<buildrail_core::domain::WatchId, String> {
        self.guard_profile()?;
        let (profile, id) = self
            .profile
            .with_added(draft)
            .map_err(|error| error.to_string())?;
        self.commit(profile)?;
        tracing::info!(watch = %id, "pinned a watch");
        Ok(id)
    }

    fn edit(&mut self, edit: ProfileEdit) -> Result<(), String> {
        self.guard_profile()?;
        let next = match edit {
            ProfileEdit::Rename { id, label } => self.profile.with_label(&id, &label),
            ProfileEdit::Move { from, to } => self.profile.with_moved(from, to),
            ProfileEdit::Remove { id } => self.profile.without(&id),
            ProfileEdit::SetConnection { id, connection } => {
                if !self.prefs.connections.contains_key(&connection) {
                    return Err(format!(
                        "\"{connection}\" is not a connection on this device."
                    ));
                }
                self.profile.with_connection(&id, &connection)
            }
        }
        .map_err(|error| error.to_string())?;
        self.commit(next)
    }

    fn replace_profile(&mut self, profile: Profile) -> Result<(), String> {
        self.commit(profile)?;
        self.profile_error = None;
        tracing::info!(watches = self.profile.len(), "profile replaced by import");
        Ok(())
    }

    // ---------------------------------------------------------------- jobs

    fn spawn(&mut self, job: impl Future<Output = JobResult> + Send + 'static) {
        self.pending_jobs += 1;
        let tx = self.jobs_tx.clone();
        let repaint = self.repaint.clone();
        self.runtime.spawn(async move {
            // The receiver only disappears when the controller is dropped.
            let _ = tx.send(job.await);
            repaint.request();
        });
    }

    fn connect(&mut self, ticket: Ticket, request: ConnectRequest) {
        let existing = self.prefs.connection(&request.alias);
        let refusal = match (existing, request.replace) {
            (Some(_), false) => Some(format!(
                "A connection named \"{}\" already exists.",
                request.alias
            )),
            (Some(local), true) if local.provider != request.provider => Some(format!(
                "\"{}\" is a {} connection.",
                request.alias, local.provider
            )),
            _ => None,
        };
        if let Some(message) = refusal {
            self.ready.push_back(Event::Connected {
                ticket,
                result: Err(ConnectError::Alias(message)),
            });
            return;
        }
        let catalogs = self.services.catalogs.clone();
        let credentials = self.services.credentials.clone();
        let meta = ConnectMeta {
            provider: request.provider,
            alias: request.alias.clone(),
            replace: request.replace,
        };
        self.spawn(async move {
            let ConnectRequest {
                provider,
                alias,
                secret,
                ..
            } = request;
            let result = async {
                let display_name = verify(&catalogs, provider, &secret).await?;
                tokio::task::spawn_blocking(move || credentials.write(&alias, &secret))
                    .await
                    .map_err(|error| ConnectError::Store(error.to_string()))?
                    .map_err(|error| ConnectError::Store(error.to_string()))?;
                Ok(display_name)
            }
            .await;
            JobResult::Connected {
                ticket,
                request: meta,
                result,
            }
        });
    }

    fn catalog(&mut self, ticket: Ticket, connection: ConnectionAlias, query: CatalogQuery) {
        let provider = match query {
            CatalogQuery::Repositories
            | CatalogQuery::Workflows { .. }
            | CatalogQuery::Branches { .. } => ProviderKind::GitHub,
            CatalogQuery::Accounts | CatalogQuery::Workers { .. } => ProviderKind::Cloudflare,
        };
        let catalogs = self.services.catalogs.clone();
        let credentials = self.services.credentials.clone();
        self.spawn(async move {
            let result = async {
                let secret = read_secret(credentials, &connection).await?;
                let map = |e| LoadError::from_provider(provider, &e);
                Ok(match query {
                    CatalogQuery::Repositories => CatalogItems::Repositories(
                        catalogs
                            .github_repositories
                            .repositories(&secret)
                            .await
                            .map_err(map)?,
                    ),
                    CatalogQuery::Workflows { owner, repo } => CatalogItems::Workflows(
                        catalogs
                            .github_workflows
                            .workflows(&owner, &repo, &secret)
                            .await
                            .map_err(map)?,
                    ),
                    CatalogQuery::Branches { owner, repo } => CatalogItems::Branches(
                        catalogs
                            .github_branches
                            .branches(&owner, &repo, &secret)
                            .await
                            .map_err(map)?,
                    ),
                    CatalogQuery::Accounts => CatalogItems::Accounts(
                        catalogs
                            .cloudflare_accounts
                            .accounts(&secret)
                            .await
                            .map_err(map)?,
                    ),
                    CatalogQuery::Workers { account_id } => CatalogItems::Workers(
                        catalogs
                            .cloudflare_workers
                            .workers(&account_id, &secret)
                            .await
                            .map_err(map)?,
                    ),
                })
            }
            .await;
            JobResult::Event(Event::Catalog { ticket, result })
        });
    }

    fn preview(&mut self, ticket: Ticket, connection: ConnectionAlias, target: WatchTarget) {
        let provider = target.provider();
        let source = self.services.sources.get(provider).cloned();
        let credentials = self.services.credentials.clone();
        self.spawn(async move {
            let result = async {
                let source = source.ok_or_else(|| {
                    LoadError::Other(format!("{provider} is not available in this build."))
                })?;
                let secret = read_secret(credentials, &connection).await?;
                let runs = source
                    .recent_runs(&target, &secret)
                    .await
                    .map_err(|e| LoadError::from_provider(provider, &e))?;
                Ok(select_latest(runs))
            }
            .await;
            JobResult::Event(Event::Preview { ticket, result })
        });
    }

    fn remove_connection(&mut self, ticket: Ticket, alias: ConnectionAlias) {
        let users = watches_using(&self.profile, &alias);
        if !users.is_empty() {
            self.ready.push_back(Event::Saved {
                ticket,
                result: Err(format!(
                    "\"{alias}\" is still used by {}.",
                    users.join(", ")
                )),
            });
            return;
        }
        let credentials = self.services.credentials.clone();
        self.spawn(async move {
            let target = alias.clone();
            let result = tokio::task::spawn_blocking(move || credentials.delete(&target))
                .await
                .map_err(|error| error.to_string())
                .and_then(|r| r.map_err(|error| error.to_string()));
            JobResult::ConnectionRemoved {
                ticket,
                alias,
                result,
            }
        });
    }

    /// Controller-side effects of a finished job, on the UI thread.
    fn finish(&mut self, result: JobResult) -> Event {
        match result {
            JobResult::Event(event) => event,
            JobResult::Connected {
                ticket,
                request,
                result,
            } => {
                let result = result.and_then(|display_name| {
                    let connection = LocalConnection {
                        provider: request.provider,
                        display_name: display_name.clone(),
                    };
                    let alias = request.alias.clone();
                    self.update_prefs(|prefs| {
                        prefs.connections.insert(alias, connection);
                    })
                    .map_err(ConnectError::Store)?;
                    tracing::info!(alias = %request.alias, provider = %request.provider, "connection saved");
                    if request.replace {
                        self.refresh_now();
                    }
                    Ok(NewConnection {
                        alias: request.alias,
                        provider: request.provider,
                        display_name,
                    })
                });
                Event::Connected { ticket, result }
            }
            JobResult::ConnectionRemoved {
                ticket,
                alias,
                result,
            } => {
                let result = result.and_then(|()| {
                    self.update_prefs(|prefs| {
                        prefs.connections.remove(&alias);
                    })
                });
                Event::Saved { ticket, result }
            }
        }
    }
}

/// Reads a connection's credential off the UI thread.
async fn read_secret(
    credentials: Arc<dyn super::ports::CredentialStore>,
    alias: &ConnectionAlias,
) -> Result<Secret, LoadError> {
    let key = alias.clone();
    let read = tokio::task::spawn_blocking(move || credentials.read(&key))
        .await
        .map_err(|error| LoadError::Other(error.to_string()))?;
    match read {
        Ok(Some(secret)) => Ok(secret),
        Ok(None) => Err(LoadError::missing_credential(alias)),
        Err(error) => Err(LoadError::Other(error.to_string())),
    }
}

/// Verifies a token with the provider's identity call and returns a
/// display name for the connection.
async fn verify(
    catalogs: &super::ports::Catalogs,
    provider: ProviderKind,
    secret: &Secret,
) -> Result<String, ConnectError> {
    let rejected = |e| ConnectError::Verify(LoadError::from_provider(provider, &e));
    match provider {
        ProviderKind::GitHub => {
            let user = catalogs
                .github_identity
                .current_user(secret)
                .await
                .map_err(rejected)?;
            Ok(user.login)
        }
        ProviderKind::Cloudflare => {
            let status = catalogs
                .cloudflare_token
                .verify_token(secret)
                .await
                .map_err(rejected)?;
            if !status.active {
                return Err(ConnectError::Inactive(status.status));
            }
            // Name the connection after the accounts it can see; the token
            // check itself only returns an opaque ID.
            let accounts = catalogs
                .cloudflare_accounts
                .accounts(secret)
                .await
                .map_err(rejected)?;
            Ok(match accounts.as_slice() {
                [] => "Cloudflare token".to_owned(),
                [one] => one.name.clone(),
                [first, rest @ ..] => format!("{} (+{} more)", first.name, rest.len()),
            })
        }
    }
}

#[cfg(test)]
mod tests;
