//! "Is BuildRail already running?"
//!
//! Starting BuildRail a second time (a second shortcut, a login item plus a
//! manual start) must not put a second strip on the screen. The first
//! instance holds a lock for as long as it runs; a later one fails to take
//! it with [`InstanceError::AlreadyRunning`] and exits quietly.
//!
//! | OS | Lock | Released |
//! |---|---|---|
//! | Windows | named mutex `Local\BuildRail.<name>` (per user session) | on drop, or by Windows when the process ends |
//! | elsewhere | an advisory lock on `<dir>/<name>.lock` (`File::try_lock`) | on drop, or by the kernel when the process ends |
//!
//! Either way a crashed or killed instance never leaves a stale lock.
//! Different names do not exclude each other (the `--demo` strip uses its
//! own). [`InMemoryInstances`] is the fake.

use std::any::Any;
use std::collections::HashSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use thiserror::Error;

/// Why the lock was not taken.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InstanceError {
    /// Another instance holds it.
    #[error("BuildRail is already running")]
    AlreadyRunning,
    /// The lock could not be checked (the caller may carry on unguarded).
    #[error("could not check whether BuildRail is already running: {0}")]
    Io(String),
}

/// Held for as long as this instance runs; dropping it lets the next one
/// start.
pub struct InstanceGuard {
    _held: Box<dyn Any + Send>,
}

impl InstanceGuard {
    /// Wraps whatever keeps the lock alive.
    pub fn new(held: impl Any + Send) -> Self {
        Self {
            _held: Box::new(held),
        }
    }
}

impl std::fmt::Debug for InstanceGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InstanceGuard")
    }
}

/// Takes the one-instance lock called `name`.
pub trait SingleInstance: Send + Sync {
    /// Takes the lock, or says another instance holds it.
    fn acquire(&self, name: &str) -> Result<InstanceGuard, InstanceError>;
}

/// `name` reduced to characters that are safe in a file or kernel object
/// name (letters, digits, `.`, `_`, `-`; anything else becomes `_`).
pub fn lock_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "BuildRail".to_owned()
    } else {
        cleaned
    }
}

/// The lock-file implementation (used everywhere but Windows, and
/// testable everywhere).
#[derive(Debug, Clone)]
pub struct LockFileInstance {
    dir: PathBuf,
}

impl LockFileInstance {
    /// Lock files go in `dir` (a per-user directory), created if missing.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The lock file for `name`.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{}.lock", lock_name(name)))
    }
}

impl SingleInstance for LockFileInstance {
    fn acquire(&self, name: &str) -> Result<InstanceGuard, InstanceError> {
        let io = |e: std::io::Error| InstanceError::Io(e.to_string());
        std::fs::create_dir_all(&self.dir).map_err(io)?;
        let mut file: File = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.path(name))
            .map_err(io)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(InstanceError::AlreadyRunning),
            Err(TryLockError::Error(error)) => return Err(io(error)),
        }
        // For whoever looks: which process holds it.
        let _ = file.set_len(0);
        let _ = write!(file, "{}", std::process::id());
        Ok(InstanceGuard::new(file))
    }
}

/// The lock this OS uses: a named mutex on Windows, a lock file in
/// `lock_dir` elsewhere.
pub fn platform_single_instance(lock_dir: &Path) -> Box<dyn SingleInstance> {
    #[cfg(windows)]
    {
        let _ = lock_dir;
        Box::new(crate::win32::instance::NamedMutexInstance)
    }
    #[cfg(not(windows))]
    {
        Box::new(LockFileInstance::new(lock_dir))
    }
}

/// In-memory fake: names held by live guards of this value (and its
/// clones).
#[derive(Debug, Clone, Default)]
pub struct InMemoryInstances {
    held: Arc<Mutex<HashSet<String>>>,
}

impl InMemoryInstances {
    /// No lock held.
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` while a guard for `name` is alive.
    pub fn is_held(&self, name: &str) -> bool {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(name)
    }
}

struct Release {
    held: Arc<Mutex<HashSet<String>>>,
    name: String,
}

impl Drop for Release {
    fn drop(&mut self) {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.name);
    }
}

impl SingleInstance for InMemoryInstances {
    fn acquire(&self, name: &str) -> Result<InstanceGuard, InstanceError> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if !held.insert(name.to_owned()) {
            return Err(InstanceError::AlreadyRunning);
        }
        Ok(InstanceGuard::new(Release {
            held: self.held.clone(),
            name: name.to_owned(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(lock_name("BuildRail-demo"), "BuildRail-demo");
        assert_eq!(lock_name("a/b\\c d"), "a_b_c_d");
        assert_eq!(lock_name(""), "BuildRail");
    }

    #[test]
    fn fake_excludes_a_second_holder_until_released() {
        let instances = InMemoryInstances::new();
        let first = instances.acquire("BuildRail").unwrap();
        assert!(instances.is_held("BuildRail"));
        assert_eq!(
            instances.acquire("BuildRail").unwrap_err(),
            InstanceError::AlreadyRunning
        );
        // Another name is independent.
        let demo = instances.clone().acquire("BuildRail-demo").unwrap();
        drop(first);
        assert!(!instances.is_held("BuildRail"));
        let _again = instances.acquire("BuildRail").unwrap();
        assert!(format!("{demo:?}").contains("InstanceGuard"));
    }

    #[test]
    fn lock_file_excludes_a_second_holder_until_released() {
        let dir = tempfile::tempdir().unwrap();
        let locks = LockFileInstance::new(dir.path().join("run"));
        let first = locks.acquire("BuildRail").unwrap();
        assert!(locks.path("BuildRail").exists());
        assert_eq!(
            locks.acquire("BuildRail").unwrap_err(),
            InstanceError::AlreadyRunning
        );
        let _demo = locks.acquire("BuildRail-demo").unwrap();
        drop(first);
        let _again = locks.acquire("BuildRail").unwrap();
    }

    #[test]
    fn unusable_lock_directory_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        let locks = LockFileInstance::new(&file);
        assert!(matches!(
            locks.acquire("BuildRail"),
            Err(InstanceError::Io(_))
        ));
    }

    #[test]
    fn platform_lock_works() {
        let dir = tempfile::tempdir().unwrap();
        let name = format!("BuildRail-test-{}", std::process::id());
        let locks = platform_single_instance(dir.path());
        let first = locks.acquire(&name).unwrap();
        assert_eq!(
            locks.acquire(&name).unwrap_err(),
            InstanceError::AlreadyRunning
        );
        drop(first);
        let _again = locks.acquire(&name).unwrap();
    }
}
