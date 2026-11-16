//! Windows `HKCU\...\CurrentVersion\Run` entry, behind a small store trait
//! so the logic is tested on every OS.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use super::{Autostart, AutostartError, LaunchCommand};

/// Registry path of the per-user Run key (relative to `HKEY_CURRENT_USER`).
pub const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// String values under the Run key.
pub trait RunKeyStore: Send + Sync {
    /// The value called `name`, if present.
    fn get(&self, name: &str) -> Result<Option<String>, AutostartError>;
    /// Creates or replaces `name`.
    fn set(&self, name: &str, value: &str) -> Result<(), AutostartError>;
    /// Removes `name` (a missing value is `Ok`).
    fn delete(&self, name: &str) -> Result<(), AutostartError>;
}

/// Quotes one argument so `CommandLineToArgvW` (and the C runtime) read it
/// back unchanged.
pub fn quote_windows_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0_usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// The Run-key command line for `command`. The executable is always quoted
/// (an unquoted path with spaces is a classic hijack risk).
pub fn windows_command_line(command: &LaunchCommand) -> Result<String, AutostartError> {
    let exe = command.executable.to_str().ok_or_else(|| {
        AutostartError::InvalidCommand("the executable path is not valid Unicode".into())
    })?;
    if exe.contains('"') {
        return Err(AutostartError::InvalidCommand(
            "quote in the executable path".into(),
        ));
    }
    let mut line = format!("\"{exe}\"");
    for arg in &command.args {
        line.push(' ');
        line.push_str(&quote_windows_arg(arg));
    }
    Ok(line)
}

/// Launch at login through a Run-key value.
#[derive(Debug)]
pub struct RunKeyAutostart<S> {
    store: S,
    name: String,
    command: LaunchCommand,
}

impl<S: RunKeyStore> RunKeyAutostart<S> {
    /// Manages value `name` in `store`.
    pub fn new(store: S, name: &str, command: LaunchCommand) -> Self {
        Self {
            store,
            name: name.to_owned(),
            command,
        }
    }
}

impl<S: RunKeyStore> Autostart for RunKeyAutostart<S> {
    fn enable(&self) -> Result<(), AutostartError> {
        self.store
            .set(&self.name, &windows_command_line(&self.command)?)
    }

    fn disable(&self) -> Result<(), AutostartError> {
        self.store.delete(&self.name)
    }

    fn is_enabled(&self) -> Result<bool, AutostartError> {
        let expected = windows_command_line(&self.command)?;
        // Windows paths are case-insensitive.
        Ok(self
            .store
            .get(&self.name)?
            .is_some_and(|value| value.eq_ignore_ascii_case(&expected)))
    }
}

/// An in-memory Run key for tests.
#[derive(Debug, Default)]
pub struct InMemoryRunKey {
    values: Mutex<HashMap<String, String>>,
}

impl InMemoryRunKey {
    /// An empty key.
    pub fn new() -> Self {
        Self::default()
    }
}

impl RunKeyStore for InMemoryRunKey {
    fn get(&self, name: &str) -> Result<Option<String>, AutostartError> {
        Ok(self
            .values
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), AutostartError> {
        self.values
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), AutostartError> {
        self.values
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argument_quoting_matches_command_line_to_argv() {
        assert_eq!(quote_windows_arg("plain"), "plain");
        assert_eq!(quote_windows_arg(""), "\"\"");
        assert_eq!(quote_windows_arg("a b"), "\"a b\"");
        assert_eq!(quote_windows_arg(r#"a"b"#), r#""a\"b""#);
        assert_eq!(
            quote_windows_arg(r"C:\dir with space\"),
            r#""C:\dir with space\\""#
        );
        assert_eq!(quote_windows_arg(r#"a\"b c"#), r#""a\\\"b c""#);
        assert_eq!(quote_windows_arg(r"no\space"), r"no\space");
    }

    #[test]
    fn command_line_always_quotes_the_executable() {
        let cmd = LaunchCommand::new(r"C:\Program Files\BuildRail\buildrail.exe")
            .arg("--minimised")
            .arg("two words");
        assert_eq!(
            windows_command_line(&cmd).unwrap(),
            r#""C:\Program Files\BuildRail\buildrail.exe" --minimised "two words""#
        );
        assert!(windows_command_line(&LaunchCommand::new("C:\\a\"b.exe")).is_err());
    }

    #[test]
    fn run_key_round_trip() {
        let auto = RunKeyAutostart::new(
            InMemoryRunKey::new(),
            "BuildRail",
            LaunchCommand::new(r"C:\BuildRail\buildrail.exe"),
        );
        assert_eq!(auto.is_enabled(), Ok(false));
        auto.enable().unwrap();
        assert_eq!(
            auto.store.get("BuildRail").unwrap().as_deref(),
            Some(r#""C:\BuildRail\buildrail.exe""#)
        );
        assert_eq!(auto.is_enabled(), Ok(true));
        auto.store
            .set("BuildRail", r#""c:\buildrail\BUILDRAIL.EXE""#)
            .unwrap();
        assert_eq!(auto.is_enabled(), Ok(true), "case-insensitive");
        auto.store
            .set("BuildRail", r#""D:\old\buildrail.exe""#)
            .unwrap();
        assert_eq!(auto.is_enabled(), Ok(false), "stale path");
        auto.disable().unwrap();
        auto.disable().unwrap();
        assert_eq!(auto.is_enabled(), Ok(false));
    }
}
