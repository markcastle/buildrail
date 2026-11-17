//! The system's open and save dialogs, through `rfd`, behind
//! [`FileDialog`]. The only adapter here; tests use
//! [`ScriptedFileDialog`](crate::fakes::ScriptedFileDialog).

use std::path::PathBuf;

use tokio::runtime::Handle;

use crate::app::FileDialog;

/// Profile files: TOML, with "all files" as the escape hatch.
const FILTER_NAME: &str = "BuildRail profile";
const FILTER_EXTENSIONS: &[&str] = &["toml"];

/// Native dialogs. On Linux `rfd` talks to the XDG desktop portal over
/// D-Bus, which runs on the tokio runtime; the handle is entered for the
/// duration of each (modal, blocking) dialog.
#[derive(Debug, Clone)]
pub struct RfdFileDialog {
    runtime: Handle,
}

impl RfdFileDialog {
    /// Dialogs whose background work runs on `runtime`.
    pub fn new(runtime: Handle) -> Self {
        Self { runtime }
    }

    fn dialog(&self) -> rfd::FileDialog {
        rfd::FileDialog::new()
            .add_filter(FILTER_NAME, FILTER_EXTENSIONS)
            .add_filter("All files", &["*"])
    }
}

impl FileDialog for RfdFileDialog {
    fn open_profile(&self) -> Option<PathBuf> {
        let _runtime = self.runtime.enter();
        self.dialog()
            .set_title("Import a BuildRail profile")
            .pick_file()
    }

    fn save_profile(&self, suggested_name: &str) -> Option<PathBuf> {
        let _runtime = self.runtime.enter();
        self.dialog()
            .set_title("Export your BuildRail profile")
            .set_file_name(suggested_name)
            .save_file()
    }
}
