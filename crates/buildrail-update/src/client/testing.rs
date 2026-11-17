//! Fakes for the client's traits, for this crate's tests and the app's.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;

use crate::client::feed::{Feed, FetchError};
use crate::client::install::{InstallError, Installer};
use crate::client::restart::{RestartError, Restarter};

/// A [`Feed`] serving fixed bytes. Unknown download URLs answer HTTP 404;
/// downloads over `max_bytes` fail with [`FetchError::TooLarge`] like the
/// real feed.
#[derive(Debug)]
pub struct StaticFeed {
    manifest: Result<Vec<u8>, FetchError>,
    payloads: HashMap<String, Vec<u8>>,
    downloads: Mutex<Vec<String>>,
}

impl StaticFeed {
    /// A feed whose `latest.json` is `manifest`.
    pub fn new(manifest: impl Into<Vec<u8>>) -> Self {
        Self {
            manifest: Ok(manifest.into()),
            payloads: HashMap::new(),
            downloads: Mutex::new(Vec::new()),
        }
    }

    /// A feed whose manifest fetch fails with `error`.
    pub fn failing(error: FetchError) -> Self {
        Self {
            manifest: Err(error),
            payloads: HashMap::new(),
            downloads: Mutex::new(Vec::new()),
        }
    }

    /// Serves `bytes` at `url`.
    pub fn with_payload(mut self, url: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        self.payloads.insert(url.into(), bytes.into());
        self
    }

    /// The URLs downloaded so far, in order.
    pub fn downloads(&self) -> Vec<String> {
        self.downloads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Feed for StaticFeed {
    async fn fetch_manifest(&self) -> Result<Vec<u8>, FetchError> {
        self.manifest.clone()
    }

    async fn download(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        self.downloads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(url.to_owned());
        let bytes = self.payloads.get(url).ok_or(FetchError::Status(404))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
            return Err(FetchError::TooLarge { max: max_bytes });
        }
        Ok(bytes.clone())
    }
}

/// An [`Installer`] that records what it was given instead of touching files.
#[derive(Debug, Default)]
pub struct RecordingInstaller {
    installed: Mutex<Vec<Vec<u8>>>,
    fail_with: Option<String>,
}

impl RecordingInstaller {
    /// Records and succeeds.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records and then fails with [`InstallError::Replace`].
    pub fn failing(message: impl Into<String>) -> Self {
        Self {
            installed: Mutex::new(Vec::new()),
            fail_with: Some(message.into()),
        }
    }

    /// Every program handed to [`Installer::install`], in order.
    pub fn installed(&self) -> Vec<Vec<u8>> {
        self.installed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Installer for RecordingInstaller {
    fn install(&self, new_exe: &[u8]) -> Result<(), InstallError> {
        self.installed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(new_exe.to_vec());
        match &self.fail_with {
            Some(message) => Err(InstallError::Replace(message.clone())),
            None => Ok(()),
        }
    }
}

/// A [`Restarter`] that counts calls.
#[derive(Debug, Default)]
pub struct RecordingRestarter {
    calls: AtomicUsize,
}

impl RecordingRestarter {
    /// A fresh counter.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many times [`Restarter::restart`] was called.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Restarter for RecordingRestarter {
    fn restart(&self) -> Result<(), RestartError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
