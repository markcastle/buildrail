//! Window placement policy, independent of the UI toolkit.
//!
//! [`DockController`] decides *where the strip goes and whether it is
//! visible*: docked at the bottom edge where the desktop can reserve it,
//! a compact movable window otherwise, hidden while a fullscreen app covers
//! its monitor. It talks to the platform crate's ports (`EdgeDock`,
//! `FullscreenProbe`, `MonitorSource`) and emits toolkit-neutral
//! [`ShellCommand`]s that a few lines of eframe glue turn into viewport
//! commands. Everything here is tested with the platform fakes.
//!
//! [`ShellWorker`] runs the controller on its own thread, so dock
//! re-applies and fullscreen probes never wake the UI unless they change
//! something.

mod dock;
mod worker;

pub use dock::{
    DockController, FloatReason, ShellCommand, ShellMode, DOCK_UPDATE_INTERVAL, FLOAT_SIZE,
};
pub use worker::{Notify, ShellLink, ShellUpdate, ShellWorker, RELEASE_TIMEOUT, TICK};
