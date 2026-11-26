//! The async driver: ties [`Schedule`] and [`Board`] to the ports.
//!
//! The engine owns all mutable state and runs as one tokio task; the UI
//! talks to it through a [`RefreshHandle`] (commands in, snapshots out), so
//! there are no locks around scheduling state. Provider calls run as
//! separate tasks, so distinct requests due at the same time are fetched
//! concurrently and one slow provider never holds up another.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tokio::task::{Id, JoinError, JoinSet};
use tokio::time::Instant;

use super::board::Board;
use super::fetch::{FetchError, Fetcher, SourceRegistry};
use super::policy::RefreshPolicy;
use super::save::{SavePolicy, SaveTracker};
use super::schedule::{Outcome, Schedule};
use super::wake::WakeDetector;
use crate::domain::{BuildRecord, RequestKey, Watch, WatchSnapshot};
use crate::ports::{Clock, CredentialReader, SnapshotCache};

/// The snapshots for all watches, in pinned order. Shared immutably so the
/// UI can hold on to a frame's worth of data without copying.
pub type Snapshots = Arc<[WatchSnapshot]>;

/// Messages from handles to the engine.
#[derive(Debug)]
enum Command {
    SetWatches(Vec<Watch>),
    RefreshNow,
    RefreshOne(RequestKey),
    Shutdown,
}

/// The engine is no longer running, so the command was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the refresh engine has stopped")]
pub struct EngineStopped;

/// A cheap, cloneable handle to a running [`RefreshEngine`].
///
/// Every method is synchronous and non-blocking (unbounded channel send,
/// `watch` borrow), so it can be called from the egui update loop or any
/// other non-async code. The engine stops when [`shutdown`](Self::shutdown)
/// is called or every handle has been dropped.
#[derive(Debug, Clone)]
pub struct RefreshHandle {
    commands: mpsc::UnboundedSender<Command>,
    snapshots: watch::Receiver<Snapshots>,
}

impl RefreshHandle {
    /// Replaces the pinned watches (profile loaded, edited, imported).
    /// Unchanged requests keep their data and timing; new ones are fetched
    /// immediately; snapshots always follow the order given here.
    pub fn set_watches(&self, watches: Vec<Watch>) -> Result<(), EngineStopped> {
        self.send(Command::SetWatches(watches))
    }

    /// Refreshes every watch now and forgets failure backoff. Call it on
    /// user request, on reconnect (network back, credential saved) and on
    /// an OS wake notification. Rate-limit pauses are still honoured.
    pub fn refresh_now(&self) -> Result<(), EngineStopped> {
        self.send(Command::RefreshNow)
    }

    /// Refreshes the one request behind `key` now and forgets its failure
    /// backoff (a person pressed Refresh in a build's details). Every
    /// watch sharing that request is refreshed with it; others keep their
    /// timing. Rate-limit pauses are still honoured.
    pub fn refresh_request(&self, key: RequestKey) -> Result<(), EngineStopped> {
        self.send(Command::RefreshOne(key))
    }

    /// Asks the engine to stop. In-flight requests are abandoned.
    pub fn shutdown(&self) {
        // Already stopped is exactly what was asked for.
        let _ = self.send(Command::Shutdown);
    }

    /// The latest snapshots, for drawing a frame.
    pub fn snapshots(&self) -> Snapshots {
        Arc::clone(&self.snapshots.borrow())
    }

    /// A receiver that is notified whenever the snapshots change. Useful to
    /// request a repaint only when something changed instead of polling.
    pub fn subscribe(&self) -> watch::Receiver<Snapshots> {
        self.snapshots.clone()
    }

    fn send(&self, command: Command) -> Result<(), EngineStopped> {
        self.commands.send(command).map_err(|_| EngineStopped)
    }
}

/// Polls providers for the pinned watches and publishes snapshots.
///
/// Every change is published; the snapshot cache is written only when a
/// restart would show something different (see [`super::save`]), and once
/// more when the engine stops.
///
/// Create it with [`new`](Self::new), hand the [`RefreshHandle`] to the UI
/// and spawn [`run`](Self::run) (or [`run_until`](Self::run_until)) on a
/// tokio runtime. The engine starts with no watches; send them with
/// [`RefreshHandle::set_watches`].
pub struct RefreshEngine {
    fetcher: Fetcher,
    clock: Arc<dyn Clock>,
    cache: Arc<dyn SnapshotCache>,
    saves: SaveTracker,
    wake: WakeDetector,
    schedule: Schedule,
    board: Board,
    commands: mpsc::UnboundedReceiver<Command>,
    publisher: watch::Sender<Snapshots>,
    in_flight: JoinSet<Result<Vec<BuildRecord>, FetchError>>,
    task_keys: HashMap<Id, RequestKey>,
}

impl RefreshEngine {
    /// Builds an engine and its first handle.
    ///
    /// The snapshot cache is read once here; its snapshots are shown for
    /// matching watch IDs (with `Pending` freshness) until the first fetch
    /// answers. See [`Board`] for the exact rules.
    pub fn new(
        sources: SourceRegistry,
        credentials: Arc<dyn CredentialReader>,
        clock: Arc<dyn Clock>,
        cache: Arc<dyn SnapshotCache>,
        policy: RefreshPolicy,
    ) -> (Self, RefreshHandle) {
        let (command_tx, commands) = mpsc::unbounded_channel();
        let (publisher, snapshots) = watch::channel(Snapshots::from(Vec::new()));
        let board = Board::new(cache.load());
        let engine = Self {
            fetcher: Fetcher::new(sources, credentials, policy.request_timeout),
            clock,
            cache,
            saves: SaveTracker::new(SavePolicy::default()),
            wake: WakeDetector::new(policy.wake_threshold),
            schedule: Schedule::new(policy),
            board,
            commands,
            publisher,
            in_flight: JoinSet::new(),
            task_keys: HashMap::new(),
        };
        let handle = RefreshHandle {
            commands: command_tx,
            snapshots,
        };
        (engine, handle)
    }

    /// Runs until [`RefreshHandle::shutdown`] or until every handle is
    /// dropped.
    pub async fn run(self) {
        self.run_until(std::future::pending()).await;
    }

    /// Like [`run`](Self::run), but also stops when `shutdown` completes
    /// (e.g. a cancellation token or the app's exit signal).
    pub async fn run_until(mut self, shutdown: impl Future<Output = ()>) {
        tokio::pin!(shutdown);
        let check_interval = self.schedule.policy().wake_check_interval;
        loop {
            self.launch_due();
            let slept_from = Instant::now();
            let wall_from = self.clock.now();
            // Never sleep longer than the check interval: monotonic timers
            // pause during system sleep, so a long timer would notice the
            // wake late.
            let cap = slept_from + check_interval;
            let wake_at = self.schedule.next_wake().map_or(cap, |due| due.min(cap));
            tokio::select! {
                biased;
                () = &mut shutdown => break,
                command = self.commands.recv() => match command {
                    None | Some(Command::Shutdown) => break,
                    Some(Command::SetWatches(watches)) => self.set_watches(watches),
                    Some(Command::RefreshNow) => self.schedule.refresh_all(Instant::now()),
                    Some(Command::RefreshOne(key)) => self.schedule.refresh_one(&key, Instant::now()),
                },
                Some(joined) = self.in_flight.join_next_with_id() => self.complete(joined),
                () = tokio::time::sleep_until(wake_at) => {}
            }
            let wall_elapsed = self.clock.now() - wall_from;
            if self.wake.is_wake(wall_elapsed, slept_from.elapsed()) {
                tracing::info!("wall clock jumped ahead; assuming wake from sleep");
                self.schedule.refresh_all(Instant::now());
            }
        }
        self.in_flight.abort_all();
        // Write what the save policy held back (timestamp drift), so the
        // next start shows ages as of now.
        let snapshots = self.publisher.borrow().clone();
        if self.saves.flush(&snapshots, Instant::now()) {
            self.cache.save(&snapshots);
        }
    }

    fn set_watches(&mut self, watches: Vec<Watch>) {
        self.board.set_watches(watches);
        self.schedule
            .sync_keys(self.board.request_keys(), Instant::now());
        self.publish();
    }

    fn launch_due(&mut self) {
        for key in self.schedule.take_due(Instant::now()) {
            let fetcher = self.fetcher.clone();
            let task_key = key.clone();
            let task = self
                .in_flight
                .spawn(async move { fetcher.fetch(&task_key).await });
            self.task_keys.insert(task.id(), key);
        }
    }

    fn complete(&mut self, joined: Result<(Id, Result<Vec<BuildRecord>, FetchError>), JoinError>) {
        let (id, result) = match joined {
            Ok((id, result)) => (id, result),
            Err(error) => {
                tracing::error!(%error, "provider task failed");
                (error.id(), Err(FetchError::Crashed))
            }
        };
        let Some(key) = self.task_keys.remove(&id) else {
            return;
        };
        let now = Instant::now();
        match result {
            Ok(runs) => {
                let active = self
                    .board
                    .apply_success(&key, runs, self.clock.now())
                    .unwrap_or(false);
                self.schedule.record(&key, Outcome::Success { active }, now);
            }
            Err(error) => {
                tracing::debug!(%error, provider = %key.target.provider(), "refresh failed");
                self.board.apply_failure(&key, error.stale_reason());
                self.schedule.record(&key, error.outcome(), now);
            }
        }
        self.publish();
    }

    /// Publishes the snapshots if they changed, and caches them when the
    /// [`SaveTracker`] says a restart would need them.
    fn publish(&mut self) {
        let snapshots = self.board.snapshots();
        if *self.publisher.borrow().as_ref() == snapshots[..] {
            return;
        }
        if self.saves.should_save(&snapshots, Instant::now()) {
            self.cache.save(&snapshots);
        }
        self.publisher.send_replace(Snapshots::from(snapshots));
    }
}

#[cfg(test)]
mod tests;
