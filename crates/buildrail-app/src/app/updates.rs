//! Self-update: the [`UpdateService`] port and the background check loop.
//!
//! The controller talks only to [`UpdateService`]; `compose.rs` hands it
//! the real [`buildrail_update::Updater`] (adapter below), `--demo` and
//! tests a scripted fake ([`crate::fakes::ScriptedUpdateService`]).
//!
//! [`run_update_loop`] runs on the app's tokio runtime for the life of the
//! process. It follows [`UpdateSchedule`] (first check 20 s after start,
//! then hourly, 5 min after a first failure, never again once the build is
//! known not to update itself), sleeps in between with no other timers,
//! logs every answer in its one line, and asks for a repaint only when
//! what the strip or Settings show changes.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use buildrail_update::{
    InstallError, RestartError, Update, UpdateSchedule, UpdateStatus, Updater, Version,
};

use crate::presentation::UpdateState;

/// Checks for, installs and starts new BuildRail releases.
#[async_trait]
pub trait UpdateService: Send + Sync {
    /// The running version.
    fn current_version(&self) -> Version;
    /// Asks the release feed whether a newer, installable release exists.
    async fn check(&self) -> UpdateStatus;
    /// Downloads, verifies and swaps in `update`. May block for a second
    /// or so (decompressing, writing); run it where that is fine. On
    /// failure nothing on disk has changed.
    async fn install(&self, update: &Update) -> Result<(), InstallError>;
    /// Starts the newly installed copy; the caller exits straight after.
    fn restart(&self) -> Result<(), RestartError>;
}

/// The real adapter: the library's updater, unchanged.
#[async_trait]
impl UpdateService for Updater {
    fn current_version(&self) -> Version {
        Updater::current_version(self).clone()
    }

    async fn check(&self) -> UpdateStatus {
        Updater::check(self).await
    }

    async fn install(&self, update: &Update) -> Result<(), InstallError> {
        Updater::install(self, update).await
    }

    fn restart(&self) -> Result<(), RestartError> {
        Updater::restart(self)
    }
}

/// A build that does not update itself (the updater could not be set
/// up): every check says so, nothing is ever installed.
#[derive(Debug, Clone)]
pub struct NoUpdates {
    version: Version,
}

impl NoUpdates {
    /// For the running `version`.
    pub fn new(version: Version) -> Self {
        Self { version }
    }
}

#[async_trait]
impl UpdateService for NoUpdates {
    fn current_version(&self) -> Version {
        self.version.clone()
    }

    async fn check(&self) -> UpdateStatus {
        UpdateStatus::NotConfigured
    }

    async fn install(&self, _: &Update) -> Result<(), InstallError> {
        Err(InstallError::NotConfigured)
    }

    fn restart(&self) -> Result<(), RestartError> {
        Err(RestartError("this build does not update itself".to_owned()))
    }
}

/// The update state shared by the check loop, install jobs and the UI.
#[derive(Debug, Clone, Default)]
pub struct SharedUpdates(Arc<Mutex<UpdateState>>);

impl SharedUpdates {
    /// Locks the state.
    pub fn lock(&self) -> MutexGuard<'_, UpdateState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A copy of the state.
    pub fn get(&self) -> UpdateState {
        self.lock().clone()
    }

    /// Applies `change` and returns whether anything the UI shows changed
    /// (the strip's chip, About's line, or a restart becoming due).
    pub fn change(&self, change: impl FnOnce(&mut UpdateState)) -> bool {
        let mut state = self.lock();
        let before = visible(&state);
        change(&mut state);
        visible(&state) != before
    }
}

/// What a frame would draw or do differently. The chip's preference
/// (automatic or not) is not here: a change of preference repaints by
/// itself.
fn visible(state: &UpdateState) -> (crate::presentation::UpdateAnswer, bool) {
    (state.answer(), state.restart_ready)
}

/// Logs a check's answer in its one line. "This build does not update
/// itself" is logged once per run.
pub fn log_status(status: &UpdateStatus, asked: bool, logged_not_configured: &mut bool) {
    let trigger = if asked { "asked" } else { "scheduled" };
    match status {
        UpdateStatus::NotConfigured if *logged_not_configured => {}
        UpdateStatus::NotConfigured => {
            *logged_not_configured = true;
            tracing::info!(trigger, "{status}");
        }
        _ => tracing::info!(trigger, "{status}"),
    }
}

/// Checks for updates on [`UpdateSchedule`]'s timetable, measured from
/// when the loop starts, until the build turns out not to update itself.
/// Each answer is recorded in `state`; `repaint` is called only when that
/// changes what is shown.
pub async fn run_update_loop(
    service: Arc<dyn UpdateService>,
    state: SharedUpdates,
    repaint: impl Fn() + Send,
) {
    let start = tokio::time::Instant::now();
    let mut schedule = UpdateSchedule::new();
    let mut logged_not_configured = false;
    while let Some(wait) = schedule.wait_from(start.elapsed()) {
        tokio::time::sleep(wait).await;
        let status = service.check().await;
        log_status(&status, false, &mut logged_not_configured);
        schedule.record(start.elapsed(), &status);
        if state.change(|s| s.record(status)) {
            repaint();
        }
    }
    tracing::debug!("no more scheduled update checks");
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use buildrail_update::{CHECK_INTERVAL, FIRST_CHECK_DELAY, RETRY_AFTER_FAILURE};

    use super::*;
    use crate::fakes::ScriptedUpdateService;

    fn counter() -> (Arc<AtomicUsize>, impl Fn() + Send) {
        let count = Arc::new(AtomicUsize::new(0));
        let inner = count.clone();
        (count, move || {
            inner.fetch_add(1, Ordering::SeqCst);
        })
    }

    #[tokio::test(start_paused = true)]
    async fn checks_on_the_schedule_and_repaints_only_for_visible_changes() {
        let service = Arc::new(ScriptedUpdateService::up_to_date("0.2.0"));
        let state = SharedUpdates::default();
        let (repaints, repaint) = counter();
        let task = tokio::spawn(run_update_loop(service.clone(), state.clone(), repaint));

        tokio::time::sleep(FIRST_CHECK_DELAY - Duration::from_secs(1)).await;
        assert_eq!(service.checks(), 0, "nothing in the first 20 s");
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(service.checks(), 1);
        // Up to date, hour after hour: logged, never painted.
        tokio::time::sleep(CHECK_INTERVAL * 5).await;
        assert_eq!(service.checks(), 6);
        assert_eq!(repaints.load(Ordering::SeqCst), 0);

        // A failure is retried soon, then hourly; still nothing to paint.
        service.answer(ScriptedUpdateService::failed());
        tokio::time::sleep(CHECK_INTERVAL).await;
        assert_eq!(service.checks(), 7);
        tokio::time::sleep(RETRY_AFTER_FAILURE).await;
        assert_eq!(service.checks(), 8);
        assert_eq!(repaints.load(Ordering::SeqCst), 0);

        // An update: one repaint (the strip offers it), however often it
        // is found again.
        service.answer(ScriptedUpdateService::available("0.3.0"));
        tokio::time::sleep(CHECK_INTERVAL * 3).await;
        assert_eq!(repaints.load(Ordering::SeqCst), 1);
        assert!(state.get().available().is_some());
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_build_that_does_not_update_itself_checks_once() {
        let service = Arc::new(ScriptedUpdateService::not_configured("0.2.0"));
        let state = SharedUpdates::default();
        let (repaints, repaint) = counter();
        run_update_loop(service.clone(), state.clone(), repaint).await;
        assert_eq!(service.checks(), 1);
        assert_eq!(repaints.load(Ordering::SeqCst), 1, "About says so");
        assert_eq!(state.get().status, Some(UpdateStatus::NotConfigured));
        let mut logged = false;
        log_status(&UpdateStatus::NotConfigured, true, &mut logged);
        assert!(logged);
        log_status(&UpdateStatus::NotConfigured, true, &mut logged);
    }

    #[tokio::test]
    async fn no_updates_never_installs() {
        let service = NoUpdates::new(Version::new(0, 2, 0));
        assert_eq!(service.current_version(), Version::new(0, 2, 0));
        assert_eq!(service.check().await, UpdateStatus::NotConfigured);
        let update = buildrail_update::testing::update(Version::new(9, 0, 0), "x");
        assert!(service.install(&update).await.is_err());
        assert!(service.restart().is_err());
    }

    #[tokio::test]
    async fn the_real_updater_is_an_update_service() {
        use buildrail_update::testing::{RecordingInstaller, RecordingRestarter, StaticFeed};

        let restarter = Arc::new(RecordingRestarter::new());
        let updater = Updater::new(
            "0.2.0",
            Arc::new(StaticFeed::new("{}")),
            Arc::new(RecordingInstaller::new()),
            restarter.clone(),
        )
        .unwrap()
        .with_public_key(None);
        let service: &dyn UpdateService = &updater;
        assert_eq!(service.current_version(), Version::new(0, 2, 0));
        assert_eq!(service.check().await, UpdateStatus::NotConfigured);
        let update = buildrail_update::testing::update(Version::new(9, 0, 0), "x");
        assert!(service.install(&update).await.is_err());
        service.restart().unwrap();
        assert_eq!(restarter.calls(), 1);
    }
}
