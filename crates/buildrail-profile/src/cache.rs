//! A JSON file implementation of [`SnapshotCache`].

use std::path::{Path, PathBuf};

use buildrail_core::domain::WatchSnapshot;
use buildrail_core::ports::SnapshotCache;
use serde::{Deserialize, Serialize};

use crate::store::AtomicFile;

/// Cache format version. A file with any other version is ignored (the
/// cache is rebuilt by the next refresh), so no migrations are needed.
const CACHE_VERSION: u32 = 1;

/// Keeps the last known snapshots in a JSON file so the strip can show
/// cached state at startup and while offline.
///
/// Writes are atomic. Every failure – missing, unreadable, corrupt or
/// outdated file, failed write – is logged with `tracing` and otherwise
/// ignored, as [`SnapshotCache`] requires.
#[derive(Debug, Clone)]
pub struct FileSnapshotCache {
    file: AtomicFile,
}

#[derive(Serialize)]
struct CacheFileOut<'a> {
    version: u32,
    snapshots: &'a [WatchSnapshot],
}

#[derive(Deserialize)]
struct CacheFileIn {
    version: u32,
    snapshots: Vec<WatchSnapshot>,
}

impl FileSnapshotCache {
    /// A cache stored at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            file: AtomicFile::new(path),
        }
    }

    /// The file path.
    pub fn path(&self) -> &Path {
        self.file.path()
    }
}

impl SnapshotCache for FileSnapshotCache {
    fn load(&self) -> Vec<WatchSnapshot> {
        let path = self.path().display();
        let bytes = match self.file.read() {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Vec::new(),
            Err(err) => {
                tracing::warn!(%path, %err, "could not read snapshot cache");
                return Vec::new();
            }
        };
        match serde_json::from_slice::<CacheFileIn>(&bytes) {
            Ok(file) if file.version == CACHE_VERSION => file.snapshots,
            Ok(file) => {
                tracing::info!(%path, version = file.version, "ignoring snapshot cache of another version");
                Vec::new()
            }
            Err(err) => {
                tracing::warn!(%path, %err, "ignoring corrupt snapshot cache");
                Vec::new()
            }
        }
    }

    fn save(&self, snapshots: &[WatchSnapshot]) {
        let document = CacheFileOut {
            version: CACHE_VERSION,
            snapshots,
        };
        let result = serde_json::to_vec(&document)
            .map_err(std::io::Error::other)
            .and_then(|bytes| self.file.write(&bytes));
        if let Err(err) = result {
            tracing::warn!(path = %self.path().display(), %err, "could not save snapshot cache");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use buildrail_core::domain::{
        BuildRecord, BuildStatus, Freshness, RunId, StaleReason, WatchId,
    };
    use chrono::{TimeZone, Utc};

    use super::*;

    fn snapshots() -> Vec<WatchSnapshot> {
        let at = Utc.with_ymd_and_hms(2026, 11, 16, 9, 30, 0).unwrap();
        vec![
            WatchSnapshot::pending(WatchId::new("a")),
            WatchSnapshot {
                watch_id: WatchId::new("b"),
                record: Some(BuildRecord {
                    run_id: RunId::new("123"),
                    attempt: 2,
                    status: BuildStatus::Failed,
                    url: "https://example.invalid/run/123".into(),
                    branch: Some("main".into()),
                    commit: Some("abc1234".into()),
                    failing_job: Some("test".into()),
                    created_at: at,
                    updated_at: at,
                    observed_at: at,
                }),
                freshness: Freshness::Stale {
                    reason: StaleReason::Offline,
                },
                last_success: Some(at),
            },
        ]
    }

    #[test]
    fn missing_is_empty_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = FileSnapshotCache::new(dir.path().join("cache/snapshots.json"));
        assert!(cache.load().is_empty());
        cache.save(&snapshots());
        assert_eq!(cache.load(), snapshots());
        cache.save(&[]);
        assert!(cache.load().is_empty());
    }

    #[test]
    fn corrupt_unreadable_or_other_version_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshots.json");
        let cache = FileSnapshotCache::new(&path);
        std::fs::write(&path, "{ not json").unwrap();
        assert!(cache.load().is_empty());
        std::fs::write(&path, r#"{"version": 99, "snapshots": []}"#).unwrap();
        assert!(cache.load().is_empty());
        // A directory cannot be read as a file.
        assert!(FileSnapshotCache::new(dir.path()).load().is_empty());
    }

    #[test]
    fn failed_save_is_swallowed_and_keeps_old_cache() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("keep"), b"x").unwrap();
        // The target is a non-empty directory, so the rename fails.
        let cache = FileSnapshotCache::new(&blocked);
        cache.save(&snapshots());
        assert_eq!(cache.path(), blocked);
        assert!(blocked.join("keep").exists());
    }
}
