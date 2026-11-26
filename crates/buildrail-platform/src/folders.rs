//! Showing a local folder (the log folder) in the system file manager.
//!
//! Only directories are opened, and only by handing the path as a single
//! argument to the platform's file manager (`explorer.exe`, `open`,
//! `xdg-open`); no shell is involved, so a path cannot run anything.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, PoisonError};

use thiserror::Error;

/// Why a folder was not opened.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpenFolderError {
    /// The path is not an existing directory.
    #[error("{0} is not a folder")]
    NotAFolder(PathBuf),
    /// The file manager could not be started.
    #[error("could not open the file manager: {0}")]
    Launch(String),
}

/// Opens folders for the user.
pub trait FolderOpener: Send + Sync {
    /// Shows `path` (an existing directory) in the file manager.
    fn open_folder(&self, path: &Path) -> Result<(), OpenFolderError>;
}

/// The program that shows a folder on this OS.
pub fn file_manager() -> &'static str {
    if cfg!(windows) {
        "explorer.exe"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Opens folders with [`file_manager`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemFolderOpener;

impl FolderOpener for SystemFolderOpener {
    fn open_folder(&self, path: &Path) -> Result<(), OpenFolderError> {
        if !path.is_dir() {
            return Err(OpenFolderError::NotAFolder(path.to_owned()));
        }
        let mut child = Command::new(file_manager())
            .arg(path)
            .spawn()
            .map_err(|error| OpenFolderError::Launch(error.to_string()))?;
        // Reap it without blocking the caller (explorer.exe returns at once;
        // xdg-open may wait for the file manager).
        std::thread::Builder::new()
            .name("buildrail-open-folder".into())
            .spawn(move || {
                let _ = child.wait();
            })
            .map(|_| ())
            .map_err(|error| OpenFolderError::Launch(error.to_string()))
    }
}

/// Records folders instead of opening them, after the same check as
/// [`SystemFolderOpener`].
#[derive(Debug, Default)]
pub struct RecordingFolderOpener {
    opened: Mutex<Vec<PathBuf>>,
}

impl RecordingFolderOpener {
    /// Nothing opened yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Folders opened so far.
    pub fn opened(&self) -> Vec<PathBuf> {
        self.opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl FolderOpener for RecordingFolderOpener {
    fn open_folder(&self, path: &Path) -> Result<(), OpenFolderError> {
        if !path.is_dir() {
            return Err(OpenFolderError::NotAFolder(path.to_owned()));
        }
        self.opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(path.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_folders_are_opened() {
        let dir = tempfile::tempdir().unwrap();
        let opener = RecordingFolderOpener::new();
        opener.open_folder(dir.path()).unwrap();
        assert_eq!(opener.opened(), vec![dir.path().to_owned()]);
        let file = dir.path().join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(matches!(
            opener.open_folder(&file),
            Err(OpenFolderError::NotAFolder(_))
        ));
        assert!(matches!(
            SystemFolderOpener.open_folder(&file),
            Err(OpenFolderError::NotAFolder(_))
        ));
        assert!(!file_manager().is_empty());
    }
}
