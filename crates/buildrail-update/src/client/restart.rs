//! Starting the freshly installed copy.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

/// The new copy could not be started.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Could not start the new version ({0}). Close and reopen BuildRail to finish updating.")]
pub struct RestartError(pub String);

/// Starts the updated program. The caller exits straight afterwards.
pub trait Restarter: Send + Sync {
    /// Spawns the new copy; does not wait for it.
    fn restart(&self) -> Result<(), RestartError>;
}

/// The real [`Restarter`]: spawns the same executable path with the same
/// arguments, inheriting the standard streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRestarter {
    exe: PathBuf,
    args: Vec<OsString>,
}

impl ProcessRestarter {
    /// This process's executable path and arguments. Build it at startup:
    /// after the file is replaced, Linux reports the running path as deleted.
    pub fn current() -> Result<Self, RestartError> {
        let exe = std::env::current_exe().map_err(|e| RestartError(e.to_string()))?;
        Ok(Self::new(exe, std::env::args_os().skip(1).collect()))
    }

    /// An explicit program and arguments.
    pub fn new(exe: impl Into<PathBuf>, args: Vec<OsString>) -> Self {
        Self {
            exe: exe.into(),
            args,
        }
    }
}

impl Restarter for ProcessRestarter {
    fn restart(&self) -> Result<(), RestartError> {
        Command::new(&self.exe)
            .args(&self.args)
            .spawn()
            .map(drop)
            .map_err(|e| RestartError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawns_the_given_program() {
        // Re-run this test binary asking for a test that does not exist: harmless.
        let exe = std::env::current_exe().unwrap();
        let restarter = ProcessRestarter::new(
            exe,
            vec![
                "--exact".into(),
                "no_such_test_exists".into(),
                "--quiet".into(),
            ],
        );
        assert_eq!(restarter.restart(), Ok(()));
    }

    #[test]
    fn reports_a_missing_program() {
        let dir = tempfile::tempdir().unwrap();
        let err = ProcessRestarter::new(dir.path().join("missing"), Vec::new())
            .restart()
            .unwrap_err();
        assert!(err
            .to_string()
            .starts_with("Could not start the new version ("));
    }

    #[test]
    fn current_captures_this_process() {
        let current = ProcessRestarter::current().unwrap();
        assert_eq!(current.exe, std::env::current_exe().unwrap());
    }
}
