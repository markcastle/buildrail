//! Launch at login, a per-device preference.
//!
//! [`Autostart`] is the small interface the settings UI uses. Each OS has a
//! hand-rolled implementation (no extra crates, and every file-writing one
//! takes its base directory so it is tested with temporary directories):
//!
//! * **Windows** – a `BuildRail` value under
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` holding the quoted
//!   executable path ([`RunKeyAutostart`]). If the user disables the entry
//!   in Task Manager's *Startup apps*, Windows records that separately under
//!   `StartupApproved` and the value stays, so `is_enabled` still reports
//!   `true`; the Task Manager switch wins at login.
//! * **macOS** – a LaunchAgent plist in `~/Library/LaunchAgents`
//!   ([`LaunchAgentAutostart`]) with `RunAtLoad`. macOS 13+ shows a
//!   "background item added" notification the first time and lists the
//!   app under *Login Items*, where the user can switch it off.
//! * **Linux/BSD** – an XDG autostart entry
//!   `$XDG_CONFIG_HOME/autostart/buildrail.desktop` ([`XdgAutostart`]),
//!   honoured by GNOME, KDE, Xfce, Cinnamon, MATE, LXQt and others. Inside
//!   Flatpak the sandbox cannot write there; use the Background portal
//!   instead (not implemented). For AppImages, [`LaunchCommand::current`]
//!   uses `$APPIMAGE` so the entry survives the mount point changing.
//!
//! [`InMemoryAutostart`] is the fake for tests.

mod launchd;
mod run_key;
mod xdg;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use thiserror::Error;

pub use launchd::{launch_agent_plist, LaunchAgentAutostart};
pub use run_key::{
    quote_windows_arg, windows_command_line, InMemoryRunKey, RunKeyAutostart, RunKeyStore,
    RUN_KEY_PATH,
};
pub use xdg::{desktop_entry, quote_exec_arg, xdg_autostart_dir, XdgAutostart};

/// Starts, stops and reports launching at login.
pub trait Autostart: Send + Sync {
    /// Registers the app to start at login (idempotent; rewrites a stale
    /// entry).
    fn enable(&self) -> Result<(), AutostartError>;
    /// Removes the registration (a missing one is `Ok`).
    fn disable(&self) -> Result<(), AutostartError>;
    /// `true` when an up-to-date registration exists.
    fn is_enabled(&self) -> Result<bool, AutostartError>;
}

/// Changing the login item failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AutostartError {
    /// Reading or writing a file failed.
    #[error("cannot update {}: {message}", path.display())]
    Io {
        /// The file or directory involved.
        path: PathBuf,
        /// The OS error.
        message: String,
    },
    /// A Windows registry call failed.
    #[error("cannot update the Windows startup entry: {0}")]
    Registry(String),
    /// The command cannot be written safely (for example a newline in the
    /// executable path).
    #[error("cannot register this program for login: {0}")]
    InvalidCommand(String),
    /// The autostart location could not be determined (no home directory).
    #[error("cannot find the login items location: {0}")]
    NoLocation(String),
}

impl AutostartError {
    fn io(path: &Path, err: &std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        }
    }
}

/// What to run at login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    /// Absolute path of the executable.
    pub executable: PathBuf,
    /// Extra arguments (for example `--minimised`).
    pub args: Vec<String>,
}

impl LaunchCommand {
    /// A command running `executable` with no arguments.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
        }
    }

    /// Adds an argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// The running executable, preferring `$APPIMAGE` (the AppImage file
    /// itself rather than its temporary mount).
    pub fn current() -> Result<Self, AutostartError> {
        Self::current_with(|key| std::env::var_os(key), std::env::current_exe)
    }

    fn current_with(
        env: impl Fn(&str) -> Option<std::ffi::OsString>,
        current_exe: impl FnOnce() -> std::io::Result<PathBuf>,
    ) -> Result<Self, AutostartError> {
        if let Some(appimage) = env("APPIMAGE").filter(|v| !v.is_empty()) {
            return Ok(Self::new(appimage));
        }
        current_exe()
            .map(Self::new)
            .map_err(|err| AutostartError::NoLocation(err.to_string()))
    }
}

/// Display name used in login-item entries.
pub const APP_NAME: &str = "BuildRail";
/// Identifier used for file names and the Run-key value.
pub const APP_ID: &str = "buildrail";
/// LaunchAgent label (reverse-DNS, as launchd expects).
pub const LAUNCH_AGENT_LABEL: &str = "dev.buildrail.BuildRail";

/// The autostart implementation for the running OS, for `command`.
pub fn platform_autostart(command: LaunchCommand) -> Result<Box<dyn Autostart>, AutostartError> {
    #[cfg(windows)]
    {
        Ok(Box::new(RunKeyAutostart::new(
            crate::win32::registry::CurrentUserRunKey,
            APP_NAME,
            command,
        )))
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .ok_or_else(|| AutostartError::NoLocation("HOME is not set".into()))?;
        let dir = PathBuf::from(home).join("Library").join("LaunchAgents");
        Ok(Box::new(LaunchAgentAutostart::new(
            dir,
            LAUNCH_AGENT_LABEL,
            command,
        )))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let dir = xdg_autostart_dir(|key| std::env::var_os(key))?;
        Ok(Box::new(XdgAutostart::new(dir, APP_ID, APP_NAME, command)))
    }
}

/// An in-memory autostart switch for tests.
#[derive(Debug, Default)]
pub struct InMemoryAutostart {
    enabled: Mutex<bool>,
}

impl InMemoryAutostart {
    /// A switch starting at `enabled`.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: Mutex::new(enabled),
        }
    }

    fn set(&self, value: bool) {
        *self.enabled.lock().unwrap_or_else(PoisonError::into_inner) = value;
    }
}

impl Autostart for InMemoryAutostart {
    fn enable(&self) -> Result<(), AutostartError> {
        self.set(true);
        Ok(())
    }

    fn disable(&self) -> Result<(), AutostartError> {
        self.set(false);
        Ok(())
    }

    fn is_enabled(&self) -> Result<bool, AutostartError> {
        Ok(*self.enabled.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Writes `contents` to `path` (creating parent directories), via a
/// temporary file and rename so a crash never leaves a half-written entry.
fn write_file(path: &Path, contents: &str) -> Result<(), AutostartError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AutostartError::io(parent, &e))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).map_err(|e| AutostartError::io(&tmp, &e))?;
    std::fs::rename(&tmp, path).map_err(|e| AutostartError::io(path, &e))
}

/// Removes `path`; a missing file is fine.
fn remove_file(path: &Path) -> Result<(), AutostartError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AutostartError::io(path, &e)),
    }
}

/// Reads `path`; a missing file is `None`.
fn read_file(path: &Path) -> Result<Option<String>, AutostartError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AutostartError::io(path, &e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_switch() {
        let a = InMemoryAutostart::new(false);
        assert_eq!(a.is_enabled(), Ok(false));
        a.enable().unwrap();
        assert_eq!(a.is_enabled(), Ok(true));
        a.disable().unwrap();
        assert_eq!(a.is_enabled(), Ok(false));
    }

    #[test]
    fn launch_command_prefers_appimage() {
        let cmd = LaunchCommand::current_with(
            |k| (k == "APPIMAGE").then(|| "/home/u/BuildRail.AppImage".into()),
            || Ok(PathBuf::from("/tmp/.mount_x/usr/bin/buildrail")),
        )
        .unwrap();
        assert_eq!(cmd, LaunchCommand::new("/home/u/BuildRail.AppImage"));
        let cmd = LaunchCommand::current_with(
            |_| Some("".into()),
            || Ok(PathBuf::from("/usr/bin/buildrail")),
        )
        .unwrap();
        assert_eq!(cmd.executable, PathBuf::from("/usr/bin/buildrail"));
        let err = LaunchCommand::current_with(|_| None, || Err(std::io::Error::other("gone")))
            .unwrap_err();
        assert!(matches!(err, AutostartError::NoLocation(_)));
        assert!(LaunchCommand::current().is_ok());
        assert_eq!(LaunchCommand::new("/x").arg("--a").args, vec!["--a"]);
    }

    #[test]
    fn file_helpers_handle_missing_files_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c.txt");
        assert_eq!(read_file(&path), Ok(None));
        write_file(&path, "hi").unwrap();
        assert_eq!(read_file(&path), Ok(Some("hi".into())));
        remove_file(&path).unwrap();
        remove_file(&path).unwrap();
        // A directory where a file is expected gives an Io error.
        let err = read_file(dir.path()).unwrap_err();
        assert!(err.to_string().starts_with("cannot update"), "{err}");
        assert!(remove_file(dir.path()).is_err());
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        assert!(write_file(&blocker.join("sub/x"), "y").is_err());
    }

    #[test]
    fn errors_read_well() {
        assert!(AutostartError::Registry("5".into())
            .to_string()
            .contains("Windows"));
        assert!(AutostartError::InvalidCommand("x".into())
            .to_string()
            .contains("login"));
        assert!(AutostartError::NoLocation("x".into())
            .to_string()
            .contains("location"));
    }

    #[test]
    fn platform_autostart_builds() {
        assert!(platform_autostart(LaunchCommand::new("/usr/bin/buildrail")).is_ok());
    }
}
