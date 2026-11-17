//! [`ShellWorker`]: the [`DockController`] on its own thread, so that
//! polling never costs a frame.
//!
//! The dock must be re-applied every few seconds (the taskbar moved, the
//! resolution changed, Explorer restarted) and fullscreen apps must be
//! noticed about once a second. Doing that from the UI would mean waking
//! the UI, and in egui every wake-up is a painted frame. Instead a small
//! thread owns the controller:
//!
//! ```text
//!   UI thread                         shell thread (wakes every TICK)
//!   ─────────                         ──────────────────────────────
//!   start / apply(prefs) ──request──▶ DockController::start / apply
//!   window moved         ──request──▶ DockController::window_moved
//!                                     DockController::tick (timer)
//!   drain() ◀──updates── commands, mode, hidden, monitors, position
//!            ◀─ notify() ─ only when there is an update
//! ```
//!
//! The UI never waits for the thread except in [`ShellLink::release`]
//! (exit, restart), and then only for [`RELEASE_TIMEOUT`]. The thread's
//! native calls (`SHAppBarMessage`, `SetWindowPos`, X11 requests on its
//! own connection) are all allowed from any thread; `SetWindowPos` on the
//! strip is delivered to the UI thread's message loop by Windows, which
//! is why the UI must never block on this thread for long.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use buildrail_platform::dock::WindowRef;
use buildrail_platform::geometry::{MonitorInfo, PhysicalPosition};
use buildrail_profile::{DevicePrefs, Position};

use super::dock::{DockController, ShellCommand, ShellMode};
use crate::app::Held;

/// How often the thread probes for fullscreen apps (and, every
/// [`super::DOCK_UPDATE_INTERVAL`], re-applies the dock).
pub const TICK: Duration = Duration::from_secs(1);

/// How long [`ShellLink::release`] waits for the screen edge to be given
/// back.
pub const RELEASE_TIMEOUT: Duration = Duration::from_secs(2);

/// Wakes the UI (it is `egui::Context::request_repaint`).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// What the thread tells the UI, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellUpdate {
    /// Apply these to the window.
    Commands(Vec<ShellCommand>),
    /// The placement changed.
    Mode(ShellMode),
    /// The strip was hidden (`true`) or shown again around a fullscreen
    /// app.
    Hidden(bool),
    /// The connected monitors (after start, apply, or when asked).
    Monitors(Vec<MonitorInfo>),
    /// The floating window was moved: remember this position.
    Moved(Position),
}

enum Request {
    Start(WindowRef, Box<DevicePrefs>),
    Apply(Box<DevicePrefs>),
    WindowMoved(PhysicalPosition),
    Monitors,
    Native(u32, usize, isize),
    Release(Sender<()>),
}

/// The UI's end of the shell thread. See the module docs.
pub struct ShellWorker {
    link: Arc<ShellLink>,
    updates: Receiver<ShellUpdate>,
}

impl std::fmt::Debug for ShellWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShellWorker")
    }
}

/// Sends requests to the shell thread; also the [`Held`] that gives the
/// screen edge back. Cheap to share.
pub struct ShellLink {
    requests: Mutex<Option<Sender<Request>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ShellLink {
    fn send(&self, request: Request) -> bool {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|tx| tx.send(request).is_ok())
    }

    /// Releases the reserved edge and stops the thread, waiting at most
    /// [`RELEASE_TIMEOUT`]. Later calls do nothing.
    pub fn release(&self) -> Result<(), String> {
        let (ack_tx, ack_rx) = mpsc::channel();
        if !self.send(Request::Release(ack_tx)) {
            return Ok(());
        }
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match ack_rx.recv_timeout(RELEASE_TIMEOUT) {
            Ok(()) => {
                let thread = self
                    .thread
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(thread) = thread {
                    let _ = thread.join();
                }
                Ok(())
            }
            Err(RecvTimeoutError::Timeout) => {
                Err("the screen edge was not released in time".to_owned())
            }
            // The thread ended without answering (it panicked; its
            // controller released the edge while unwinding).
            Err(RecvTimeoutError::Disconnected) => Ok(()),
        }
    }
}

impl Held for ShellLink {
    fn name(&self) -> &'static str {
        "screen edge"
    }

    fn release(&self) -> Result<(), String> {
        ShellLink::release(self)
    }
}

impl ShellWorker {
    /// Starts the thread with `shell`, places the strip for `window` with
    /// `prefs`, and ticks every [`TICK`].
    pub fn spawn(
        shell: DockController,
        window: WindowRef,
        prefs: &DevicePrefs,
        notify: Notify,
    ) -> std::io::Result<Self> {
        Self::spawn_with_tick(shell, window, prefs, notify, TICK)
    }

    /// [`spawn`](Self::spawn) with another tick (tests).
    pub fn spawn_with_tick(
        shell: DockController,
        window: WindowRef,
        prefs: &DevicePrefs,
        notify: Notify,
        tick: Duration,
    ) -> std::io::Result<Self> {
        let (requests, inbox) = mpsc::channel();
        let (outbox, updates) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("buildrail-shell".into())
            .spawn(move || run(shell, &inbox, &outbox, &notify, tick))?;
        let link = Arc::new(ShellLink {
            requests: Mutex::new(Some(requests)),
            thread: Mutex::new(Some(thread)),
        });
        link.send(Request::Start(window, Box::new(prefs.clone())));
        Ok(Self { link, updates })
    }

    /// The shared link (register it with the app's lifecycle).
    pub fn link(&self) -> Arc<ShellLink> {
        self.link.clone()
    }

    /// Re-places the strip after a placement, display or scale change.
    pub fn apply(&self, prefs: &DevicePrefs) {
        self.link.send(Request::Apply(Box::new(prefs.clone())));
    }

    /// The floating window is now at `outer` (physical).
    pub fn window_moved(&self, outer: PhysicalPosition) {
        self.link.send(Request::WindowMoved(outer));
    }

    /// Asks for the monitor list (Settings was opened).
    pub fn refresh_monitors(&self) {
        self.link.send(Request::Monitors);
    }

    /// Forwards a native window message (Windows AppBar callback).
    pub fn native_message(&self, message: u32, wparam: usize, lparam: isize) {
        self.link.send(Request::Native(message, wparam, lparam));
    }

    /// Updates that arrived since the last call.
    pub fn drain(&self) -> Vec<ShellUpdate> {
        self.updates.try_iter().collect()
    }

    /// Waits up to `timeout` for the next update (tests).
    pub fn wait(&self, timeout: Duration) -> Option<ShellUpdate> {
        self.updates.recv_timeout(timeout).ok()
    }
}

impl Drop for ShellWorker {
    fn drop(&mut self) {
        if let Err(error) = self.link.release() {
            tracing::warn!(%error, "closing the strip");
        }
    }
}

/// The thread: requests and ticks in, updates out.
fn run(
    mut shell: DockController,
    inbox: &Receiver<Request>,
    outbox: &Sender<ShellUpdate>,
    notify: &Notify,
    tick: Duration,
) {
    let mut mode = shell.mode().clone();
    let mut hidden = shell.is_hidden();
    let mut next_tick = Instant::now() + tick;
    loop {
        let wait = next_tick.saturating_duration_since(Instant::now());
        let mut out = Vec::new();
        let commands = match inbox.recv_timeout(wait) {
            Ok(Request::Start(window, prefs)) => {
                let commands = shell.start(window, &prefs, Instant::now());
                out.push(ShellUpdate::Monitors(shell.monitors()));
                commands
            }
            Ok(Request::Apply(prefs)) => {
                let commands = shell.apply(&prefs, Instant::now());
                out.push(ShellUpdate::Monitors(shell.monitors()));
                commands
            }
            Ok(Request::WindowMoved(outer)) => {
                out.extend(shell.window_moved(outer).map(ShellUpdate::Moved));
                Vec::new()
            }
            Ok(Request::Monitors) => {
                out.push(ShellUpdate::Monitors(shell.monitors()));
                Vec::new()
            }
            Ok(Request::Native(message, wparam, lparam)) => {
                shell.handle_native_message(message, wparam, lparam)
            }
            Ok(Request::Release(ack)) => {
                shell.shutdown();
                tracing::debug!("screen edge released");
                let _ = ack.send(());
                return;
            }
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                next_tick = now + tick;
                shell.tick(now)
            }
            Err(RecvTimeoutError::Disconnected) => {
                shell.shutdown();
                return;
            }
        };
        if !commands.is_empty() {
            out.insert(0, ShellUpdate::Commands(commands));
        }
        if *shell.mode() != mode {
            mode = shell.mode().clone();
            out.push(ShellUpdate::Mode(mode.clone()));
        }
        if shell.is_hidden() != hidden {
            hidden = shell.is_hidden();
            out.push(ShellUpdate::Hidden(hidden));
        }
        if out.is_empty() {
            continue;
        }
        for update in out {
            if outbox.send(update).is_err() {
                // The UI is gone; give the edge back and stop.
                shell.shutdown();
                return;
            }
        }
        notify();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use std::num::NonZeroU32;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use buildrail_platform::dock::{DockCall, ScriptedDock};
    use buildrail_platform::fullscreen::{FakeFullscreenProbe, FullscreenState};
    use buildrail_platform::geometry::{MonitorId, PhysicalRect};
    use buildrail_platform::monitors::StaticMonitors;
    use buildrail_profile::Placement;

    use super::*;

    const FAST: Duration = Duration::from_millis(10);
    const WAIT: Duration = Duration::from_secs(5);

    struct F {
        dock: ScriptedDock,
        probe: FakeFullscreenProbe,
        notified: Arc<AtomicUsize>,
        worker: ShellWorker,
    }

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new("DP-1"),
            bounds: PhysicalRect::new(0, 0, 1920, 1080),
            work_area: PhysicalRect::new(0, 0, 1920, 1040),
            scale_factor: 1.0,
            is_primary: true,
        }
    }

    fn start(prefs: &DevicePrefs) -> F {
        let dock = ScriptedDock::reserving();
        let probe = FakeFullscreenProbe::new(FullscreenState::NotFullscreen);
        let shell = DockController::new(
            Box::new(dock.clone()),
            Box::new(probe.clone()),
            Box::new(StaticMonitors(vec![monitor()])),
        );
        let notified = Arc::new(AtomicUsize::new(0));
        let count = notified.clone();
        let worker = ShellWorker::spawn_with_tick(
            shell,
            WindowRef::Xcb(NonZeroU32::new(7).unwrap()),
            prefs,
            Arc::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }),
            FAST,
        )
        .unwrap();
        F {
            dock,
            probe,
            notified,
            worker,
        }
    }

    /// Collects updates until `done` says the expected one arrived.
    fn until(f: &F, done: impl Fn(&ShellUpdate) -> bool) -> Vec<ShellUpdate> {
        let deadline = Instant::now() + WAIT;
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            if let Some(update) = f.worker.wait(Duration::from_millis(50)) {
                let stop = done(&update);
                seen.push(update);
                if stop {
                    return seen;
                }
            }
        }
        panic!("timed out; saw {seen:?}");
    }

    #[test]
    fn starts_docked_and_stays_quiet_while_nothing_changes() {
        let f = start(&DevicePrefs::default());
        let seen = until(&f, |u| {
            matches!(u, ShellUpdate::Mode(ShellMode::Docked { .. }))
        });
        assert_eq!(seen[0], ShellUpdate::Commands(vec![ShellCommand::Show]));
        assert!(matches!(&seen[1], ShellUpdate::Monitors(m) if m.len() == 1));
        // The wake-up follows the updates; wait for it, not just for them.
        let deadline = Instant::now() + WAIT;
        while f.notified.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "no wake-up for the start");
            std::thread::sleep(FAST);
        }
        let after_start = f.notified.load(Ordering::SeqCst);
        assert_eq!(after_start, 1, "one wake-up for the start");

        // Many ticks (fullscreen probes, dock re-applies) and no change:
        // no update, no wake-up.
        std::thread::sleep(super::super::DOCK_UPDATE_INTERVAL + FAST * 20);
        assert!(f.worker.drain().is_empty());
        assert_eq!(f.notified.load(Ordering::SeqCst), after_start);
        assert!(f
            .dock
            .calls()
            .iter()
            .any(|c| matches!(c, DockCall::Update { .. })));

        // A fullscreen app: exactly one wake-up with Hide.
        f.probe.set(FullscreenState::Fullscreen);
        let seen = until(&f, |u| *u == ShellUpdate::Hidden(true));
        assert_eq!(seen[0], ShellUpdate::Commands(vec![ShellCommand::Hide]));
        let deadline = Instant::now() + WAIT;
        while f.notified.load(Ordering::SeqCst) == after_start {
            assert!(Instant::now() < deadline, "no wake-up for Hide");
            std::thread::sleep(FAST);
        }
        std::thread::sleep(FAST * 10);
        assert_eq!(f.notified.load(Ordering::SeqCst), after_start + 1);
        f.probe.set(FullscreenState::NotFullscreen);
        until(&f, |u| *u == ShellUpdate::Hidden(false));

        // Releasing gives the edge back, once.
        let link = f.worker.link();
        assert_eq!(link.name(), "screen edge");
        assert!(Held::release(link.as_ref()).is_ok());
        assert_eq!(f.dock.calls().last(), Some(&DockCall::Release));
        assert!(link.release().is_ok());
        // Requests after release are dropped quietly.
        f.worker.apply(&DevicePrefs::default());
        f.worker.refresh_monitors();
    }

    #[test]
    fn floating_moves_are_reported_and_placement_can_change() {
        let floating = DevicePrefs {
            placement: Placement::Floating { position: None },
            ..DevicePrefs::default()
        };
        let f = start(&floating);
        let seen = until(&f, |u| matches!(u, ShellUpdate::Mode(_)));
        let Some(ShellUpdate::Commands(commands)) = seen.first() else {
            panic!("{seen:?}")
        };
        assert!(matches!(commands[0], ShellCommand::Place(_)));

        f.worker.window_moved(PhysicalPosition { x: 100, y: 200 });
        let seen = until(&f, |u| matches!(u, ShellUpdate::Moved(_)));
        assert!(seen.contains(&ShellUpdate::Moved(Position { x: 100, y: 200 })));

        f.worker.refresh_monitors();
        until(&f, |u| matches!(u, ShellUpdate::Monitors(_)));
        f.worker.native_message(0, 0, 0);

        f.worker.apply(&DevicePrefs::default());
        until(&f, |u| {
            matches!(u, ShellUpdate::Mode(ShellMode::Docked { .. }))
        });
        drop(f.worker);
        assert_eq!(f.dock.calls().last(), Some(&DockCall::Release));
    }
}
