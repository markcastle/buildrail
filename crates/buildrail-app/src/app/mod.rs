//! The application service layer: the [`AppController`] and the bundle of
//! ports ([`Services`]) it is built from.
//!
//! The controller is the only place that mutates the profile, the device
//! preferences and the credential store, and the only owner of the refresh
//! engine handle. The UI reads from it and sends it settings
//! [`Command`](crate::settings::Command)s; it never touches a port directly.

pub mod controller;
pub mod lifecycle;
pub mod ports;
pub mod updates;

pub use controller::{AppController, RepaintFn, RestartError};
pub use lifecycle::{Held, InstanceLock, Lifecycle, ReleaseOnDrop};
pub use ports::{Catalogs, CredentialStore, FileDialog, Services};
pub use updates::{run_update_loop, NoUpdates, SharedUpdates, UpdateService};
