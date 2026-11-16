//! Crash-safe file storage.
//!
//! [`AtomicFile`] is the one place that knows how to replace a file safely:
//! write a temporary file in the same directory, flush and fsync it, then
//! rename it over the target (on Windows `MoveFileEx` with
//! `MOVEFILE_REPLACE_EXISTING`, via [`tempfile::NamedTempFile::persist`]).
//! Readers therefore see either the old or the new contents, never a mix,
//! and a failure at any step leaves the old file byte-for-byte intact.
//!
//! [`DocumentStore`] layers a versioned text document on top of it and is
//! used for both the profile ([`ProfileStore`]) and the device preferences
//! ([`crate::device::DevicePrefsStore`]).

use std::fs;
use std::io::{self, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;
use thiserror::Error;

use crate::format::{parse_profile, profile_to_toml, DocumentError};
use crate::profile::Profile;

/// A file that is only ever replaced as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicFile {
    path: PathBuf,
}

impl AtomicFile {
    /// A handle for `path`. Nothing is touched until [`read`](Self::read) or
    /// [`write`](Self::write).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The target path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The current contents, or `None` when the file does not exist.
    pub fn read(&self) -> io::Result<Option<Vec<u8>>> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Atomically replaces the file with `contents`, creating parent
    /// directories as needed.
    pub fn write(&self, contents: &[u8]) -> io::Result<()> {
        self.write_with(|file| file.write_all(contents))
    }

    /// The replacement procedure, with the content-writing step injectable
    /// so tests can fail half-way through.
    fn write_with(&self, fill: impl FnOnce(&mut fs::File) -> io::Result<()>) -> io::Result<()> {
        let dir = match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        fs::create_dir_all(dir)?;
        // Dropping `temp` on any early return deletes the temporary file.
        let mut temp = NamedTempFile::new_in(dir)?;
        fill(temp.as_file_mut())?;
        temp.as_file_mut().flush()?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path).map_err(|err| err.error)?;
        sync_dir(dir);
        Ok(())
    }
}

/// Makes the rename itself durable on Unix. Best effort: the data is already
/// safe, and some file systems refuse to sync directories.
#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Err(err) = fs::File::open(dir).and_then(|d| d.sync_all()) {
        tracing::debug!(dir = %dir.display(), %err, "could not fsync directory");
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

/// A value stored as a text document by [`DocumentStore`].
pub trait StoredDocument: Sized {
    /// The value used when the file does not exist yet (first launch).
    fn initial() -> Self;
    /// Parses and validates the file contents.
    fn decode(text: &str) -> Result<Self, DocumentError>;
    /// Validates and serialises the value. An `Err` means nothing is
    /// written.
    fn encode(&self) -> Result<String, DocumentError>;
}

impl StoredDocument for Profile {
    fn initial() -> Self {
        Profile::empty()
    }

    fn decode(text: &str) -> Result<Self, DocumentError> {
        parse_profile(text)
    }

    fn encode(&self) -> Result<String, DocumentError> {
        Ok(profile_to_toml(self))
    }
}

/// Why loading or saving a document failed.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Reading or replacing the file failed. On save, the previous file is
    /// unchanged.
    #[error("could not access {path}: {source}")]
    Io {
        /// File involved.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// The file exists but is not valid UTF-8.
    #[error("{path} is not UTF-8 text")]
    NotText {
        /// File involved.
        path: PathBuf,
    },
    /// The contents (on load) or the value (on save) are invalid. On save,
    /// nothing was written.
    #[error("{path}: {source}")]
    Document {
        /// File involved.
        path: PathBuf,
        /// What is wrong.
        source: DocumentError,
    },
}

/// Loads and atomically saves one document of type `D` at a fixed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentStore<D> {
    file: AtomicFile,
    _document: PhantomData<fn() -> D>,
}

/// The store for the user's profile.
pub type ProfileStore = DocumentStore<Profile>;

impl<D: StoredDocument> DocumentStore<D> {
    /// A store for the file at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            file: AtomicFile::new(path),
            _document: PhantomData,
        }
    }

    /// The file path.
    pub fn path(&self) -> &Path {
        self.file.path()
    }

    /// Reads the document. A missing file yields [`StoredDocument::initial`];
    /// a malformed one is an error (the file is left alone so the user can
    /// fix it).
    pub fn load(&self) -> Result<D, StoreError> {
        let bytes = self.file.read().map_err(|source| self.io(source))?;
        let Some(bytes) = bytes else {
            return Ok(D::initial());
        };
        let text = String::from_utf8(bytes).map_err(|_| StoreError::NotText {
            path: self.path().to_owned(),
        })?;
        D::decode(&text).map_err(|source| self.document(source))
    }

    /// Validates, encodes and atomically replaces the file. If anything
    /// fails the previous file is untouched.
    pub fn save(&self, document: &D) -> Result<(), StoreError> {
        let text = document.encode().map_err(|source| self.document(source))?;
        self.file
            .write(text.as_bytes())
            .map_err(|source| self.io(source))
    }

    fn io(&self, source: io::Error) -> StoreError {
        StoreError::Io {
            path: self.path().to_owned(),
            source,
        }
    }

    fn document(&self, source: DocumentError) -> StoreError {
        StoreError::Document {
            path: self.path().to_owned(),
            source,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::validate::WatchDraft;

    fn sample() -> Profile {
        Profile::from_drafts(&[
            WatchDraft::github("API", "work", "acme", "api", "ci.yml", None).with_id("a"),
            WatchDraft::cloudflare(
                "Site",
                "cf",
                "0123456789abcdef0123456789abcdef",
                "site",
                None,
            )
            .with_id("b"),
        ])
        .unwrap()
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn missing_file_is_empty_profile() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path().join("nested/profile.toml"));
        assert_eq!(store.load().unwrap(), Profile::empty());
    }

    #[test]
    fn save_then_load_creates_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/profile.toml");
        let store = ProfileStore::new(&path);
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), sample());
        assert_eq!(store.path(), path);
        assert_eq!(entries(&dir.path().join("a/b")), ["profile.toml"]);
    }

    #[test]
    fn save_replaces_existing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path().join("profile.toml"));
        store.save(&sample()).unwrap();
        store.save(&Profile::empty()).unwrap();
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn failed_write_leaves_original_intact_and_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.toml");
        fs::write(&path, b"original bytes").unwrap();
        let file = AtomicFile::new(&path);
        let err = file
            .write_with(|f| {
                f.write_all(b"half of the new")?;
                Err(io::Error::other("disk full"))
            })
            .unwrap_err();
        assert_eq!(err.to_string(), "disk full");
        assert_eq!(fs::read(&path).unwrap(), b"original bytes");
        assert_eq!(entries(dir.path()), ["profile.toml"]);
    }

    #[test]
    fn failed_rename_leaves_target_intact() {
        // The target is a non-empty directory, so the final rename fails.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.toml");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), b"x").unwrap();
        let err = ProfileStore::new(&path).save(&sample()).unwrap_err();
        assert!(matches!(err, StoreError::Io { .. }), "{err}");
        assert_eq!(fs::read(path.join("keep")).unwrap(), b"x");
        assert_eq!(entries(dir.path()), ["profile.toml"]);
    }

    #[test]
    fn malformed_file_is_reported_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.toml");
        fs::write(&path, "version = 1\n[[watch]]\nlabel = 3\n").unwrap();
        let err = ProfileStore::new(&path).load().unwrap_err();
        assert!(matches!(err, StoreError::Document { .. }));
        assert!(err.to_string().contains("label must be a string"), "{err}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "version = 1\n[[watch]]\nlabel = 3\n"
        );
    }

    #[test]
    fn non_utf8_and_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.toml");
        fs::write(&path, [0xff, 0xfe]).unwrap();
        let err = ProfileStore::new(&path).load().unwrap_err();
        assert!(matches!(err, StoreError::NotText { .. }), "{err}");
        assert!(err.to_string().contains("not UTF-8"));

        // A directory where the file should be cannot be read.
        let err = ProfileStore::new(dir.path()).load().unwrap_err();
        assert!(matches!(err, StoreError::Io { .. }), "{err}");
    }
}
