//! The controller's side of self-update: "Check for updates", the one
//! install flow, "Not now", and the automatic install at a quiet moment.
//!
//! Installing runs on a blocking thread (it decompresses and writes the
//! new program). When it succeeds the job only marks the restart as due;
//! the next frame calls [`AppController::poll_update`] on the UI thread,
//! which gets the process ready ([`AppController::prepare_for_restart`]:
//! engine stopped, preferences and cache written, screen edge and
//! single-instance lock released), starts the new copy and tells the UI
//! to close. A failed install changes nothing on disk (the library
//! guarantees it); the error is logged and shown, and the app keeps
//! running.

use buildrail_update::Version;

use super::AppController;
use crate::presentation::{
    is_quiet_moment, should_install_automatically, UpdateAnswer, UpdateChip, UpdateState,
};

impl AppController {
    /// The running version, as the update service sees it.
    pub fn current_version(&self) -> Version {
        self.services.updates.current_version()
    }

    /// A copy of what the app knows about updates.
    pub fn update_state(&self) -> UpdateState {
        self.updates.get()
    }

    /// The line Settings › About shows.
    pub fn update_answer(&self) -> UpdateAnswer {
        self.updates.lock().answer()
    }

    /// The strip's update offer, if there is one.
    pub fn update_chip(&self) -> Option<UpdateChip> {
        self.updates.lock().chip(self.prefs.update_automatically)
    }

    /// "Check for updates" in Settings: checks now and answers in About.
    /// Ignored while a check is already running.
    pub fn check_for_updates(&self) {
        let already_known_not_configured = {
            let mut state = self.updates.lock();
            if state.checking {
                return;
            }
            state.checking = true;
            state.asked = true;
            state.status == Some(buildrail_update::UpdateStatus::NotConfigured)
        };
        self.repaint.request();
        let service = self.services.updates.clone();
        let updates = self.updates.clone();
        let repaint = self.repaint.clone();
        self.runtime.spawn(async move {
            let status = service.check().await;
            let mut logged = already_known_not_configured;
            super::super::updates::log_status(&status, true, &mut logged);
            updates.change(|s| s.record(status));
            repaint.request();
        });
    }

    /// "Not now": hides the offer for this version until the app starts
    /// again.
    pub fn dismiss_update(&self) {
        let changed = self.updates.change(|state| {
            if let Some(version) = state.available().map(|u| u.version.clone()) {
                tracing::info!(%version, "update offer dismissed until the next start");
                state.dismissed = Some(version);
            }
        });
        if changed {
            self.repaint.request();
        }
    }

    /// Installs the available update (the strip's chip, Settings, or an
    /// automatic install). Does nothing when there is none or an install
    /// is already running, so double clicks are harmless. The result
    /// arrives through [`poll_update`](Self::poll_update).
    pub fn install_update(&self) {
        self.start_install(false);
    }

    fn start_install(&self, automatic: bool) {
        let update = {
            let mut state = self.updates.lock();
            if state.installing || state.restart_ready {
                return;
            }
            let Some(update) = state.available().cloned() else {
                return;
            };
            state.installing = true;
            state.install_error = None;
            update
        };
        tracing::info!(version = %update.version, automatic, "installing update");
        self.repaint.request();
        let service = self.services.updates.clone();
        let updates = self.updates.clone();
        let repaint = self.repaint.clone();
        let handle = self.runtime.clone();
        self.runtime.spawn_blocking(move || {
            let result = handle.block_on(service.install(&update));
            updates.change(|state| match result {
                Ok(()) => {
                    tracing::info!(version = %update.version, "update installed; restarting");
                    state.restart_ready = true;
                }
                Err(error) => {
                    tracing::warn!(version = %update.version, %error, "update not installed");
                    state.installing = false;
                    state.install_error = Some(error.to_string());
                    if automatic {
                        state.auto_failed = Some(update.version.clone());
                    }
                }
            });
            repaint.request();
        });
    }

    /// Starts an automatic install when the person turned "Update
    /// automatically" on, an update is available and this is a quiet
    /// moment (no watched build queued or running, Settings closed).
    /// Called once per frame; cheap when there is nothing to do.
    pub fn maybe_install_automatically(&self, settings_open: bool) {
        if !self.prefs.update_automatically {
            return;
        }
        let quiet = is_quiet_moment(&self.refresh.snapshots(), settings_open);
        let due = should_install_automatically(&self.updates.lock(), true, quiet);
        if due {
            tracing::info!("a quiet moment: installing the update");
            self.start_install(true);
        }
    }

    /// Finishes a successful install on the UI thread: gets the process
    /// ready to be replaced, starts the new copy and returns `true`, which
    /// tells the UI to close. Returns `false` when no restart is due.
    ///
    /// If the new copy cannot be started, the update is still installed;
    /// the app closes anyway (everything is already released) and the log
    /// says to start BuildRail again.
    pub fn poll_update(&mut self) -> bool {
        let due = std::mem::take(&mut self.updates.lock().restart_ready);
        if !due {
            return false;
        }
        if let Err(error) = self.prepare_for_restart() {
            tracing::warn!(%error, "restarting anyway");
        }
        match self.services.updates.restart() {
            Ok(()) => tracing::info!("started the new version; closing"),
            Err(error) => tracing::error!(
                %error,
                "the update is installed but the new version could not be started; start BuildRail again"
            ),
        }
        true
    }
}
