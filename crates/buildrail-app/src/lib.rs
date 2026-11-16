//! # BuildRail desktop app
//!
//! The thin (24 logical px) strip that shows pinned GitHub Actions
//! workflows and Cloudflare Workers Builds, its Settings window, and the
//! composition root that wires the other BuildRail crates together.
//!
//! ## How the pieces fit
//!
//! ```text
//!                 ┌──────────────── compose (picks adapters, starts eframe) ───────────────┐
//!                 │                                                                         │
//!   ui::window ── ui::app::BuildRailUi ── ui::strip / ui::settings      shell::DockController
//!   (eframe glue)        │  draws                    │ inputs                (dock, fullscreen,
//!                        ▼                           ▼                       monitors → commands)
//!            presentation::StripModel      settings::{wizard, manage, import, connect}
//!                        ▲                           │ Command        ▲ Event
//!                        │ snapshots                 ▼                │
//!                 app::AppController ── profile + device prefs + credential store + RefreshHandle
//!                        │ jobs on tokio
//!                        ▼
//!            ports: RunSource, catalogues, CredentialStore, UrlOpener, Autostart, …
//! ```
//!
//! * [`presentation`] – pure view-models: what each strip entry says, how
//!   it is announced, what fits. No egui.
//! * [`settings`] – the Add-build wizard, the manage list and the import
//!   flow as explicit state machines (inputs in, [`settings::Command`]s
//!   out, [`settings::Event`]s back). No egui, no I/O.
//! * [`app`] – [`app::AppController`], the application service that owns
//!   the profile, the device preferences and the refresh engine handle,
//!   and executes commands (async work on tokio, results over a channel).
//! * [`shell`] – [`shell::DockController`]: dock or float, hide for
//!   fullscreen apps, remember the floating position. Toolkit neutral.
//! * [`ui`] – the egui widgets and the thin eframe glue.
//! * [`fakes`] / [`demo`] – in-memory providers for tests and `--demo`.
//! * [`compose`] – the only place that names concrete adapters.
//!
//! Everything except `ui::window` and `compose::run` is exercised by tests
//! without a real window; the UI itself is tested with `egui_kittest`.

pub mod app;
pub mod compose;
pub mod demo;
pub mod fakes;
pub mod presentation;
pub mod settings;
pub mod shell;
pub mod ui;
