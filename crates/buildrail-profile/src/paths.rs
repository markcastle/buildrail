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
///
/// Device preferences use the *local* config directory so a roaming Windows
/// profile cannot carry one machine's monitor choice to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    profile_dir: PathBuf,
    device_dir: PathBuf,
    cache_dir: PathBuf,
}

impl AppPaths {
    /// The standard per-user locations for this OS.
    pub fn discover() -> Result<Self, NoHomeDir> {
        let dirs = ProjectDirs::from("dev", "BuildRail", "BuildRail").ok_or(NoHomeDir)?;
        Ok(Self::from_dirs(
            dirs.config_dir(),
            dirs.config_local_dir(),
            dirs.cache_dir(),
        ))
    }

    /// Explicit directories, for tests and portable installs.
    pub fn from_dirs(
        profile_dir: impl Into<PathBuf>,
        device_dir: impl Into<PathBuf>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            profile_dir: profile_dir.into(),
            device_dir: device_dir.into(),
            cache_dir: cache_dir.into(),
        }
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
            }
            Err(err) => assert_eq!(err, NoHomeDir),
        }
    }
}
