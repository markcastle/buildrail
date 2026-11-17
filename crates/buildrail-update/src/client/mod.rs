//! The client side (feature `client`): feed, verification, install, restart, schedule.

mod feed;
mod install;
mod restart;
mod schedule;
pub mod testing;
mod updater;

pub use feed::{Feed, FetchError, ReqwestFeed, DOWNLOAD_TIMEOUT, MANIFEST_TIMEOUT};
pub use install::{
    cleanup_after_update, cleanup_after_update_at, ExeReplacer, InstallError, Installer,
    SwapStrategy,
};
pub use restart::{ProcessRestarter, RestartError, Restarter};
pub use schedule::{
    decide, Action, UpdatePrefs, UpdateSchedule, CHECK_INTERVAL, FIRST_CHECK_DELAY,
    RETRY_AFTER_FAILURE,
};
pub use updater::{CheckError, Limits, SetupError, Update, UpdateStatus, Updater};
