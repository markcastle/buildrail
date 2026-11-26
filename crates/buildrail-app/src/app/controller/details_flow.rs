//! The details use-case: open, close, refresh and fetch a build's details
//! panel.
//!
//! The controller keeps a [`DetailsBook`](crate::app::DetailsBook) (which
//! panel is open, what was fetched this session) and asks it, whenever the
//! panel is drawn, whether to fetch: once per run attempt and status, again
//! while a running run moves on, and when the person presses Refresh now
//! or Try again. A fetch is one job on the runtime: the connection's token
//! off the UI thread, the watch's recent runs from its `RunSource`, then
//! the run's details from its `RunDetailsSource`, sent back over the job
//! channel with one repaint. The refresh engine never fetches details.

use buildrail_core::domain::{Freshness, RequestKey, StaleReason, Watch, WatchId, WatchSnapshot};
use buildrail_core::ports::Secret;
use chrono::{Local, Offset, TimeZone};

use super::{AppController, JobResult};
use crate::app::details::{DetailsAnswer, DetailsError, DetailsFetch};
use crate::presentation::{DetailsInput, DetailsModel};

impl AppController {
    /// Opens the details panel for `watch` (closing any other one) and
    /// starts fetching its details when they are not known yet.
    pub fn open_details(&mut self, watch: &WatchId) {
        self.details.open(watch.clone());
        self.plan_details(false);
    }

    /// Closes the details panel.
    pub fn close_details(&mut self) {
        self.details.close();
    }

    /// The build whose details panel is open.
    pub fn details_open(&self) -> Option<&WatchId> {
        self.details.open_watch()
    }

    /// "Refresh now": checks the open build's request at once (the refresh
    /// engine, as for every poll) and fetches its details again.
    pub fn refresh_details(&mut self) {
        if let Some(watch) = self.open_details_watch() {
            if self
                .refresh
                .refresh_request(RequestKey::from(&watch))
                .is_err()
            {
                tracing::warn!("refresh requested after the engine stopped");
            }
        }
        self.plan_details(true);
    }

    /// "Try again" after the details could not be loaded.
    pub fn retry_details(&mut self) {
        self.plan_details(true);
    }

    /// The open details panel's view-model, fetching (again) first when
    /// what the strip shows for the build asks for it. `None` when no panel
    /// is open (or its build was removed, which closes it).
    pub fn details_panel(&mut self) -> Option<DetailsModel> {
        self.plan_details(false);
        let watch = self.open_details_watch()?;
        let snapshots = self.refresh.snapshots();
        let pending = WatchSnapshot::pending(watch.id.clone());
        let snapshot = snapshots
            .iter()
            .find(|s| s.watch_id == watch.id)
            .unwrap_or(&pending);
        let view = self.details.view(&watch.id);
        let now = self.services.clock.now();
        let offset = Local.offset_from_utc_datetime(&now.naive_utc()).fix();
        Some(DetailsModel::build(DetailsInput {
            watch: &watch,
            snapshot,
            details: view.details,
            recent: view.recent,
            load: view.load,
            now,
            offset,
        }))
    }

    /// The open panel's watch; closes the panel if it is no longer pinned.
    fn open_details_watch(&mut self) -> Option<Watch> {
        let id = self.details.open_watch()?.clone();
        let watch = self.profile.watches().iter().find(|w| w.id == id).cloned();
        if watch.is_none() {
            self.details.close();
        }
        watch
    }

    /// Asks the book whether to fetch for the open panel, and starts the
    /// fetch if so.
    fn plan_details(&mut self, force: bool) {
        let Some(watch) = self.open_details_watch() else {
            return;
        };
        let snapshots = self.refresh.snapshots();
        let snapshot = snapshots.iter().find(|s| s.watch_id == watch.id);
        // A build that needs reconnecting would only be refused again.
        let record = snapshot
            .filter(|s| {
                s.freshness
                    != (Freshness::Stale {
                        reason: StaleReason::AccessDenied,
                    })
            })
            .and_then(|s| s.record.as_ref());
        if let Some(fetch) = self.details.plan(record, force) {
            self.fetch_details(watch, fetch);
        }
    }

    fn fetch_details(&mut self, watch: Watch, fetch: DetailsFetch) {
        let provider = watch.target.provider();
        let runs = self.services.sources.get(provider).cloned();
        let source = self.services.details.get(provider).cloned();
        let credentials = self.services.credentials.clone();
        tracing::debug!(watch = %watch.id, run = %fetch.run.run_id, "fetching run details");
        self.spawn(async move {
            let answer = async {
                let Some(source) = source else {
                    return DetailsAnswer {
                        details: Err(DetailsError::NoSource(provider)),
                        recent: None,
                    };
                };
                let secret = match details_secret(credentials, &watch).await {
                    Ok(secret) => secret,
                    Err(error) => {
                        return DetailsAnswer {
                            details: Err(error),
                            recent: None,
                        }
                    }
                };
                // History first: for Cloudflare the details are read from
                // the list this call just fetched.
                let recent = match runs {
                    Some(runs) => runs.recent_runs(&watch.target, &secret).await.ok(),
                    None => None,
                };
                let details = source
                    .run_details(&watch.target, &fetch.run, &secret)
                    .await
                    .map_err(|error| DetailsError::Provider(provider, error));
                DetailsAnswer { details, recent }
            }
            .await;
            JobResult::Details {
                request: fetch.request,
                watch: fetch.watch,
                answer,
            }
        });
    }
}

/// Reads the watch's token off the UI thread.
async fn details_secret(
    credentials: std::sync::Arc<dyn crate::app::CredentialStore>,
    watch: &Watch,
) -> Result<Secret, DetailsError> {
    let alias = watch.connection.clone();
    let key = alias.clone();
    match tokio::task::spawn_blocking(move || credentials.read(&key)).await {
        Ok(Ok(Some(secret))) => Ok(secret),
        Ok(Ok(None)) => Err(DetailsError::NoCredential(alias)),
        Ok(Err(_)) | Err(_) => Err(DetailsError::CredentialStore),
    }
}
