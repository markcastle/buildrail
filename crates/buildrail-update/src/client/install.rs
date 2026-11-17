//! Replacing the running executable.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use crate::client::feed::FetchError;
use crate::policy::UrlError;
use crate::signing::SignatureError;

/// Suffix of the new file while it is being written beside the executable.
const NEW_SUFFIX: &str = ".update-new";
/// Suffix of the previous executable after a move-aside swap (Windows).
const OLD_SUFFIX: &str = ".update-old";

/// Why an update was not installed. `Display` is a full sentence for a log
/// line or Settings. In every case except [`InstallError::Replace`] the
/// installed program was not touched.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    /// No embedded public key (or no release asset for this platform).
    #[error("Update not installed: this build does not update itself.")]
    NotConfigured,
    /// The release was signed with a key this build does not carry.
    #[error("Update not installed: it was signed with a different key. Download the latest release by hand.")]
    DifferentKey,
    /// The manifest's download address is not acceptable.
    #[error("Update not installed: {0}.")]
    UnsafeUrl(UrlError),
    /// The announced payload size is zero or over the cap.
    #[error("Update not installed: the download is empty or larger than allowed.")]
    TooLarge,
    /// The download failed.
    #[error("Update not installed: download failed: {0}.")]
    Download(FetchError),
    /// The download is not the announced size.
    #[error("Update not installed: the download is not the size the update feed announced.")]
    SizeMismatch,
    /// The download's SHA-256 is not the announced one.
    #[error(
        "Update not installed: the download does not match the update feed (SHA-256 differs)."
    )]
    HashMismatch,
    /// The signature does not verify against this build's key.
    #[error("Update not installed: the download is not signed by BuildRail ({0}).")]
    BadSignature(SignatureError),
    /// The payload is not valid gzip.
    #[error("Update not installed: the download could not be decompressed.")]
    Decompress,
    /// The decompressed program is over [`crate::MAX_EXECUTABLE_BYTES`].
    #[error("Update not installed: the program inside the download is larger than allowed.")]
    ExecutableTooLarge,
    /// The decompressed file is not a program for this platform.
    #[error("Update not installed: the download is not a {0} program.")]
    NotAnExecutable(&'static str),
    /// Writing or swapping the file failed.
    #[error("Update not installed: could not replace the program file ({0}).")]
    Replace(String),
}

impl From<io::Error> for InstallError {
    fn from(error: io::Error) -> Self {
        Self::Replace(error.to_string())
    }
}

/// Puts a verified executable in place of the current one.
pub trait Installer: Send + Sync {
    /// Installs `new_exe` (the decompressed, verified program).
    fn install(&self, new_exe: &[u8]) -> Result<(), InstallError>;
}

/// How [`ExeReplacer`] swaps the files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapStrategy {
    /// Rename the new file over the old one: one atomic step (Unix).
    RenameOver,
    /// Rename the running file aside to `<exe>.update-old`, then rename the
    /// new file into place (Windows, which will not overwrite a running
    /// executable but will rename it). [`cleanup_after_update`] deletes the
    /// old copy on the next start.
    MoveAside,
}

impl SwapStrategy {
    /// The right strategy for the platform this was compiled for.
    pub fn for_this_platform() -> Self {
        if cfg!(windows) {
            Self::MoveAside
        } else {
            Self::RenameOver
        }
    }
}

/// The real [`Installer`]: writes the new program beside the target,
/// makes it executable (0o755 on Unix), flushes it to disk, and swaps it
/// in with a rename, so the target is never half-written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExeReplacer {
    target: PathBuf,
    strategy: SwapStrategy,
}

impl ExeReplacer {
    /// Replaces the running executable. Call at startup: on Linux, once the
    /// file is replaced the running process's own path reads as deleted.
    pub fn current_exe() -> Result<Self, InstallError> {
        let exe = std::env::current_exe()?;
        // Replace the real file, not a symlink pointing at it.
        let exe = fs::canonicalize(&exe).unwrap_or(exe);
        Ok(Self::at(exe))
    }

    /// Replaces the file at `target` (tests point this at a temporary file).
    pub fn at(target: impl Into<PathBuf>) -> Self {
        Self {
            target: target.into(),
            strategy: SwapStrategy::for_this_platform(),
        }
    }

    /// Overrides the swap strategy.
    pub fn with_strategy(mut self, strategy: SwapStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// The file this replaces.
    pub fn target(&self) -> &Path {
        &self.target
    }

    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = File::create(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        make_executable(path)
    }

    fn swap(&self, new: &Path) -> io::Result<()> {
        match self.strategy {
            SwapStrategy::RenameOver => fs::rename(new, &self.target),
            SwapStrategy::MoveAside => {
                let old = sibling(&self.target, OLD_SUFFIX)?;
                remove_if_present(&old)?;
                fs::rename(&self.target, &old)?;
                fs::rename(new, &self.target).inspect_err(|_| {
                    // Put the original back so the app still starts.
                    let _ = fs::rename(&old, &self.target);
                })
            }
        }
    }
}

impl Installer for ExeReplacer {
    fn install(&self, new_exe: &[u8]) -> Result<(), InstallError> {
        let new = sibling(&self.target, NEW_SUFFIX)?;
        let result = self.write_new(&new, new_exe).and_then(|()| self.swap(&new));
        if result.is_err() {
            let _ = fs::remove_file(&new);
        }
        Ok(result?)
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn sibling(exe: &Path, suffix: &str) -> io::Result<PathBuf> {
    let name = exe.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the program path has no file name",
        )
    })?;
    let mut name = name.to_os_string();
    name.push(OsStr::new(suffix));
    Ok(exe.with_file_name(name))
}

fn remove_if_present(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Deletes what an earlier update left beside the running executable (the
/// moved-aside old copy on Windows, or a half-written new file after a
/// crash). Call once at startup; returns whether anything was removed.
pub fn cleanup_after_update() -> io::Result<bool> {
    let exe = std::env::current_exe()?;
    cleanup_after_update_at(&fs::canonicalize(&exe).unwrap_or(exe))
}

/// [`cleanup_after_update`] for an explicit executable path.
pub fn cleanup_after_update_at(exe: &Path) -> io::Result<bool> {
    let old = remove_if_present(&sibling(exe, OLD_SUFFIX)?)?;
    let new = remove_if_present(&sibling(exe, NEW_SUFFIX)?)?;
    Ok(old || new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_exe(dir: &Path) -> PathBuf {
        let exe = dir.join("buildrail-fake");
        fs::write(&exe, b"old program").unwrap();
        exe
    }

    #[test]
    fn rename_over_replaces_the_file_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake_exe(dir.path());
        let replacer = ExeReplacer::at(&exe).with_strategy(SwapStrategy::RenameOver);
        assert_eq!(replacer.target(), exe.as_path());
        replacer.install(b"new program").unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new program");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(!cleanup_after_update_at(&exe).unwrap());
    }

    #[test]
    fn move_aside_keeps_the_old_copy_until_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake_exe(dir.path());
        let replacer = ExeReplacer::at(&exe).with_strategy(SwapStrategy::MoveAside);
        replacer.install(b"v2").unwrap();
        // A second update before a restart replaces the stale old copy.
        replacer.install(b"v3").unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"v3");
        let old = dir.path().join("buildrail-fake.update-old");
        assert_eq!(fs::read(&old).unwrap(), b"v2");
        assert!(cleanup_after_update_at(&exe).unwrap());
        assert!(!old.exists());
        assert!(!cleanup_after_update_at(&exe).unwrap());
    }

    #[test]
    fn cleanup_removes_a_half_written_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake_exe(dir.path());
        fs::write(dir.path().join("buildrail-fake.update-new"), b"partial").unwrap();
        assert!(cleanup_after_update_at(&exe).unwrap());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn real_cleanup_runs_against_the_test_binary_harmlessly() {
        // Nothing named `<test binary>.update-old` exists, so nothing is removed.
        assert!(!cleanup_after_update().unwrap());
        assert!(ExeReplacer::current_exe().unwrap().target().is_file());
    }

    #[cfg(unix)]
    #[test]
    fn the_new_file_is_executable() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let exe = fake_exe(dir.path());
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o600)).unwrap();
        ExeReplacer::at(&exe).install(b"new").unwrap();
        let mode = fs::metadata(&exe).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn failures_leave_the_target_alone_and_clean_up() {
        let dir = tempfile::tempdir().unwrap();
        // Directory that does not exist: the new file cannot be created.
        let missing = dir.path().join("nope").join("exe");
        let err = ExeReplacer::at(&missing).install(b"x").unwrap_err();
        assert!(matches!(err, InstallError::Replace(_)));
        assert!(err
            .to_string()
            .starts_with("Update not installed: could not replace"));

        // Target is missing, so moving it aside fails; the new file is removed.
        let gone = dir.path().join("gone");
        let err = ExeReplacer::at(&gone)
            .with_strategy(SwapStrategy::MoveAside)
            .install(b"x")
            .unwrap_err();
        assert!(matches!(err, InstallError::Replace(_)));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

        // A path with no file name.
        let err = ExeReplacer::at("/").install(b"x").unwrap_err();
        assert!(matches!(err, InstallError::Replace(_)));
        assert!(cleanup_after_update_at(Path::new("/")).is_err());
    }

    #[test]
    fn cleanup_reports_errors_other_than_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake_exe(dir.path());
        // A directory where the old copy would be cannot be removed as a file.
        fs::create_dir(dir.path().join("buildrail-fake.update-old")).unwrap();
        assert!(cleanup_after_update_at(&exe).is_err());
    }

    #[test]
    fn strategy_matches_the_platform() {
        let expected = if cfg!(windows) {
            SwapStrategy::MoveAside
        } else {
            SwapStrategy::RenameOver
        };
        assert_eq!(SwapStrategy::for_this_platform(), expected);
    }
}
