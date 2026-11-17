//! Where BuildRail keeps its files on each OS.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use thiserror::Error;

use crate::cache::FileSnapshotCache;
use crate::device::DevicePrefsStore;
use crate::store::ProfileStore;

/// File name of the profile inside [`AppPaths::profile_dir`].
pub const PROFILE_FILE: &str = "profile.toml";
/// File name of the device preferences inside [`AppPaths::device_dir`].
pub const DEVICE_PREFS_FILE: &str = "device.toml";
/// File name of the snapshot cache inside [`AppPaths::cache_dir`].
pub const SNAPSHOT_CACHE_FILE: &str = "snapshots.json";
/// Name of the log folder inside the local data (or given) directory.
pub const LOGS_DIR: &str = "logs";

/// The OS gave no home directory, so no standard locations exist.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("could not determine the user's configuration directories")]
pub struct NoHomeDir;

/// Resolved file locations.
///
/// | | Linux | macOS | Windows |
/// |---|---|---|---|
/// | profile | `~/.config/buildrail` | `~/Library/Application Support/dev.BuildRail.BuildRail` | `%APPDATA%\BuildRail\BuildRail\config` (roaming) |
/// | device prefs | `~/.config/buildrail` | same as profile | `%LOCALAPPDATA%\BuildRail\BuildRail\config` (never roams) |
/// | cache | `~/.cache/buildrail` | `~/Library/Caches/dev.BuildRail.BuildRail` | `%LOCALAPPDATA%\BuildRail\BuildRail\cache` |
/// | logs | `~/.local/share/buildrail/logs` | `~/Library/Application Support/dev.BuildRail.BuildRail/logs` | `%LOCALAPPDATA%\BuildRail\BuildRail\data\logs` |
///
/// Device preferences use the *local* config directory so a roaming Windows
/// profile cannot carry one machine's monitor choice to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    profile_dir: PathBuf,
    device_dir: PathBuf,
    cache_dir: PathBuf,
    logs_dir: PathBuf,
}

impl AppPaths {
    /// The standard per-user locations for this OS.
    pub fn discover() -> Result<Self, NoHomeDir> {
        let dirs = ProjectDirs::from("dev", "BuildRail", "BuildRail").ok_or(NoHomeDir)?;
        Ok(
            Self::from_dirs(dirs.config_dir(), dirs.config_local_dir(), dirs.cache_dir())
                .with_logs_dir(dirs.data_local_dir().join(LOGS_DIR)),
        )
    }

    /// Explicit directories, for tests and portable installs. Logs go in
    /// `logs` under `device_dir` unless [`with_logs_dir`](Self::with_logs_dir)
    /// says otherwise.
    pub fn from_dirs(
        profile_dir: impl Into<PathBuf>,
        device_dir: impl Into<PathBuf>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        let device_dir = device_dir.into();
        Self {
            profile_dir: profile_dir.into(),
            logs_dir: device_dir.join(LOGS_DIR),
            device_dir,
            cache_dir: cache_dir.into(),
        }
    }

    /// The same locations with the log folder at `logs_dir`.
    pub fn with_logs_dir(mut self, logs_dir: impl Into<PathBuf>) -> Self {
        self.logs_dir = logs_dir.into();
        self
    }

    /// Directory holding the log files (never synchronised, never
    /// exported).
    pub fn logs_dir(&self) -> &Path {
        &self.logs_dir
    }

    /// Directory holding the profile.
    pub fn profile_dir(&self) -> &Path {
        &self.profile_dir
    }

    /// Directory holding the device preferences.
    pub fn device_dir(&self) -> &Path {
        &self.device_dir
    }

    /// Directory holding disposable caches.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Full path of the profile file.
    pub fn profile_file(&self) -> PathBuf {
        self.profile_dir.join(PROFILE_FILE)
    }

    /// Full path of the device preferences file.
    pub fn device_prefs_file(&self) -> PathBuf {
        self.device_dir.join(DEVICE_PREFS_FILE)
    }

    /// Full path of the snapshot cache.
    pub fn snapshot_cache_file(&self) -> PathBuf {
        self.cache_dir.join(SNAPSHOT_CACHE_FILE)
    }

    /// A store for the profile file.
    pub fn profile_store(&self) -> ProfileStore {
        ProfileStore::new(self.profile_file())
    }

    /// A store for the device preferences file.
    pub fn device_prefs_store(&self) -> DevicePrefsStore {
        DevicePrefsStore::new(self.device_prefs_file())
    }

    /// The snapshot cache.
    pub fn snapshot_cache(&self) -> FileSnapshotCache {
        FileSnapshotCache::new(self.snapshot_cache_file())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use buildrail_core::ports::SnapshotCache;

    use super::*;
    use crate::device::DevicePrefs;
    use crate::profile::Profile;

    #[test]
    fn explicit_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_dirs(
            dir.path().join("p"),
            dir.path().join("d"),
            dir.path().join("c"),
        );
        assert_eq!(paths.profile_dir(), dir.path().join("p"));
        assert_eq!(paths.device_dir(), dir.path().join("d"));
        assert_eq!(paths.cache_dir(), dir.path().join("c"));
        assert_eq!(paths.logs_dir(), dir.path().join("d/logs"));
        assert_eq!(
            paths.clone().with_logs_dir(dir.path().join("l")).logs_dir(),
            dir.path().join("l")
        );
        assert_eq!(paths.profile_file(), dir.path().join("p/profile.toml"));
        assert_eq!(paths.device_prefs_file(), dir.path().join("d/device.toml"));
        assert_eq!(
            paths.snapshot_cache_file(),
            dir.path().join("c/snapshots.json")
        );
        assert_eq!(paths.profile_store().load().unwrap(), Profile::empty());
        assert_eq!(
            paths.device_prefs_store().load().unwrap(),
            DevicePrefs::default()
        );
        assert!(paths.snapshot_cache().load().is_empty());
    }

    #[test]
    fn discover_uses_os_locations() {
        // Containers and CI normally have a home directory; if not, the
        // error is the documented one.
        match AppPaths::discover() {
            Ok(paths) => {
                assert!(paths.profile_file().ends_with(PROFILE_FILE));
                assert!(paths.profile_dir().is_absolute());
                assert!(paths.logs_dir().ends_with(LOGS_DIR));
            }
            Err(err) => assert_eq!(err, NoHomeDir),
        }
    }
}
