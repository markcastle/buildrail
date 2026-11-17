//! The update service: check the feed, then verify and install.

use std::fmt;
use std::io::Read as _;
use std::sync::Arc;

use semver::Version;

use crate::client::feed::{Feed, FetchError, ReqwestFeed};
use crate::client::install::{ExeReplacer, InstallError, Installer};
use crate::client::restart::{ProcessRestarter, RestartError, Restarter};
use crate::keys::{embedded_public_key, PublicKey};
use crate::manifest::{Manifest, ManifestError, TargetEntry};
use crate::policy::{
    current_target, is_newer, validate_asset_url, UrlError, MAX_EXECUTABLE_BYTES,
    MAX_MANIFEST_BYTES, MAX_PAYLOAD_BYTES,
};
use crate::signing::{sha256_hex, verify};

/// Release notes must live under the repository; anything else is replaced
/// by the releases page so the app never opens an arbitrary link.
const REPOSITORY_PREFIX: &str = "https://github.com/markcastle/buildrail/";
const RELEASES_PAGE: &str = "https://github.com/markcastle/buildrail/releases";

/// Size caps applied by [`Updater`]. The defaults are the crate constants;
/// tests lower them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest `latest.json`.
    pub manifest_bytes: u64,
    /// Largest compressed payload a manifest may announce.
    pub payload_bytes: u64,
    /// Largest decompressed executable.
    pub executable_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            manifest_bytes: MAX_MANIFEST_BYTES,
            payload_bytes: MAX_PAYLOAD_BYTES,
            executable_bytes: MAX_EXECUTABLE_BYTES,
        }
    }
}

/// A newer release this build can install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// The new version.
    pub version: Version,
    /// The release page (always under the BuildRail repository).
    pub notes_url: String,
    pub(crate) target: &'static str,
    pub(crate) entry: TargetEntry,
    pub(crate) key_id: String,
}

/// Why a check failed. `Display` is a lowercase phrase; see [`UpdateStatus`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckError {
    /// The feed could not be fetched.
    #[error("{0}")]
    Fetch(FetchError),
    /// `latest.json` is over the size cap.
    #[error("the update feed is larger than allowed")]
    ManifestTooLarge,
    /// `latest.json` is unreadable, or needs a newer updater.
    #[error("{0}")]
    Manifest(ManifestError),
    /// The manifest's version is not semantic versioning.
    #[error("the update feed names an invalid version")]
    BadVersion,
    /// The newer release was signed with a key this build does not carry.
    #[error("the latest release was signed with a different key; download it by hand")]
    DifferentKey,
    /// The newer release's download address is not acceptable.
    #[error("{0}")]
    UnsafeUrl(UrlError),
    /// The announced download is empty or over the size cap.
    #[error("the latest release's download is empty or larger than allowed")]
    PayloadTooLarge,
}

/// The result of [`Updater::check`]. `Display` gives the one line for the
/// log and for Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// This build has no embedded key or no release asset for its platform.
    NotConfigured,
    /// Nothing newer to install. This includes a newer release that has no
    /// build for this platform: macOS and Linux are built only for some
    /// releases, so that is normal, not a failure, and the next check simply
    /// looks again (the reason is logged at debug level).
    UpToDate {
        /// The running version.
        current: Version,
    },
    /// A newer release can be installed.
    Available(Update),
    /// The check did not complete.
    Failed(CheckError),
}

impl fmt::Display for UpdateStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured => f.write_str("This build does not update itself."),
            Self::UpToDate { .. } => f.write_str("Up to date."),
            Self::Available(update) => write!(f, "BuildRail {} is available.", update.version),
            Self::Failed(error) => write!(f, "Update check failed: {error}."),
        }
    }
}

/// [`Updater`] could not be set up.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetupError {
    /// The running version is not semantic versioning.
    #[error("the running version {0:?} is not a semantic version")]
    InvalidVersion(String),
    /// The running executable could not be located.
    #[error("could not locate the running program: {0}")]
    Exe(String),
}

/// What [`Updater::find_update`] found in the feed.
#[derive(Debug)]
enum Found {
    /// A newer release this build can install.
    Update(Update),
    /// The feed's release is not newer than this build.
    NothingNewer,
    /// The feed's release is newer but has no build for this platform
    /// (macOS and Linux are built only for some releases).
    NoBuildForTarget {
        /// The release's version.
        version: Version,
    },
}

/// Checks for, verifies and installs updates.
///
/// Built once at startup and shared (`Arc<Updater>`); every method takes
/// `&self`.
pub struct Updater {
    current: Version,
    feed: Arc<dyn Feed>,
    installer: Arc<dyn Installer>,
    restarter: Arc<dyn Restarter>,
    public_key: Option<PublicKey>,
    target: Option<&'static str>,
    limits: Limits,
}

impl fmt::Debug for Updater {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Updater")
            .field("current", &self.current.to_string())
            .field("public_key", &self.public_key)
            .field("target", &self.target)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Updater {
    /// The production wiring: GitHub feed over `client`, replacing the
    /// running executable, restarting with the same arguments. Call at
    /// startup (it records the executable path before any update).
    pub fn for_this_app(
        client: reqwest::Client,
        current_version: &str,
    ) -> Result<Self, SetupError> {
        let installer = ExeReplacer::current_exe().map_err(|e| SetupError::Exe(e.to_string()))?;
        let restarter = ProcessRestarter::current().map_err(|e| SetupError::Exe(e.0))?;
        Self::new(
            current_version,
            Arc::new(ReqwestFeed::new(client)),
            Arc::new(installer),
            Arc::new(restarter),
        )
    }

    /// An updater over any feed, installer and restarter, using the key
    /// embedded at compile time and this build's target.
    pub fn new(
        current_version: &str,
        feed: Arc<dyn Feed>,
        installer: Arc<dyn Installer>,
        restarter: Arc<dyn Restarter>,
    ) -> Result<Self, SetupError> {
        let current = Version::parse(current_version)
            .map_err(|_| SetupError::InvalidVersion(current_version.to_owned()))?;
        Ok(Self {
            current,
            feed,
            installer,
            restarter,
            public_key: embedded_public_key(),
            target: current_target(),
            limits: Limits::default(),
        })
    }

    /// Overrides the verifying key (`None` behaves like a build without one).
    pub fn with_public_key(mut self, key: Option<PublicKey>) -> Self {
        self.public_key = key;
        self
    }

    /// Overrides the target name (`None` behaves like an unknown platform).
    pub fn with_target(mut self, target: Option<&'static str>) -> Self {
        self.target = target;
        self
    }

    /// Overrides the size caps.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The running version.
    pub fn current_version(&self) -> &Version {
        &self.current
    }

    /// Fetches `latest.json` and says whether a newer, installable release exists.
    pub async fn check(&self) -> UpdateStatus {
        let (Some(key), Some(target)) = (self.public_key, self.target) else {
            return UpdateStatus::NotConfigured;
        };
        match self.find_update(&key, target).await {
            Ok(Found::Update(update)) => UpdateStatus::Available(update),
            Ok(Found::NothingNewer) => UpdateStatus::UpToDate {
                current: self.current.clone(),
            },
            Ok(Found::NoBuildForTarget { version }) => {
                tracing::debug!(
                    "release {version} has no build for {target}; staying on {}",
                    self.current
                );
                UpdateStatus::UpToDate {
                    current: self.current.clone(),
                }
            }
            Err(error) => UpdateStatus::Failed(error),
        }
    }

    async fn find_update(
        &self,
        key: &PublicKey,
        target: &'static str,
    ) -> Result<Found, CheckError> {
        let bytes = self
            .feed
            .fetch_manifest()
            .await
            .map_err(CheckError::Fetch)?;
        if len_u64(&bytes) > self.limits.manifest_bytes {
            return Err(CheckError::ManifestTooLarge);
        }
        let manifest = Manifest::parse(&bytes).map_err(CheckError::Manifest)?;
        let version = Version::parse(&manifest.version).map_err(|_| CheckError::BadVersion)?;
        if !is_newer(&self.current, &version) {
            return Ok(Found::NothingNewer);
        }
        // Before the key: a release with nothing for this platform is not
        // for this copy at all, whoever signed it.
        let Some(entry) = manifest.targets.get(target) else {
            return Ok(Found::NoBuildForTarget { version });
        };
        if manifest.key_id != key.key_id() {
            return Err(CheckError::DifferentKey);
        }
        validate_asset_url(&entry.url).map_err(CheckError::UnsafeUrl)?;
        if entry.size == 0 || entry.size > self.limits.payload_bytes {
            return Err(CheckError::PayloadTooLarge);
        }
        Ok(Found::Update(Update {
            version,
            notes_url: safe_notes_url(&manifest.notes_url),
            target,
            entry: entry.clone(),
            key_id: manifest.key_id,
        }))
    }

    /// Downloads, verifies and installs `update`.
    ///
    /// Order: size cap → download (capped at the announced size) → exact
    /// size → SHA-256 → Ed25519 signature against the *embedded* key over
    /// version, target and hash → gunzip (capped) → executable sanity
    /// check → [`Installer::install`]. Any failure before the last step
    /// leaves the installed program untouched.
    pub async fn install(&self, update: &Update) -> Result<(), InstallError> {
        let key = self.public_key.ok_or(InstallError::NotConfigured)?;
        if update.key_id != key.key_id() {
            return Err(InstallError::DifferentKey);
        }
        let entry = &update.entry;
        validate_asset_url(&entry.url).map_err(InstallError::UnsafeUrl)?;
        if entry.size == 0 || entry.size > self.limits.payload_bytes {
            return Err(InstallError::TooLarge);
        }
        let payload = self
            .feed
            .download(&entry.url, entry.size)
            .await
            .map_err(InstallError::Download)?;
        if len_u64(&payload) != entry.size {
            return Err(InstallError::SizeMismatch);
        }
        let hash = sha256_hex(&payload);
        if !hash.eq_ignore_ascii_case(&entry.sha256) {
            return Err(InstallError::HashMismatch);
        }
        verify(
            &key,
            &update.version.to_string(),
            update.target,
            &hash,
            &entry.signature,
        )
        .map_err(InstallError::BadSignature)?;
        let program = gunzip_capped(&payload, self.limits.executable_bytes)?;
        check_executable(update.target, &program)?;
        self.installer.install(&program)
    }

    /// Starts the newly installed copy; the caller should exit straight after.
    pub fn restart(&self) -> Result<(), RestartError> {
        self.restarter.restart()
    }
}

fn len_u64(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

fn safe_notes_url(url: &str) -> String {
    let plain = url.chars().all(|c| c.is_ascii_graphic());
    if url.starts_with(REPOSITORY_PREFIX) && plain {
        url.to_owned()
    } else {
        RELEASES_PAGE.to_owned()
    }
}

fn gunzip_capped(gz: &[u8], max: u64) -> Result<Vec<u8>, InstallError> {
    let mut program = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .take(max.saturating_add(1))
        .read_to_end(&mut program)
        .map_err(|_| InstallError::Decompress)?;
    if len_u64(&program) > max {
        return Err(InstallError::ExecutableTooLarge);
    }
    Ok(program)
}

/// Checks the file starts like a program for `target`: `MZ` on Windows,
/// ELF on Linux, Mach-O (thin or universal) on macOS.
fn check_executable(target: &'static str, program: &[u8]) -> Result<(), InstallError> {
    const MACH_O: [[u8; 4]; 4] = [
        [0xcf, 0xfa, 0xed, 0xfe], // 64-bit, little-endian
        [0xce, 0xfa, 0xed, 0xfe], // 32-bit, little-endian
        [0xca, 0xfe, 0xba, 0xbe], // universal
        [0xfe, 0xed, 0xfa, 0xcf], // 64-bit, big-endian
    ];
    let ok = if target.starts_with("windows-") {
        program.starts_with(b"MZ")
    } else if target.starts_with("macos-") {
        MACH_O.iter().any(|magic| program.starts_with(magic))
    } else {
        program.starts_with(b"\x7fELF")
    };
    if ok {
        Ok(())
    } else {
        Err(InstallError::NotAnExecutable(target))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;
    use std::io::Write as _;

    use super::*;
    use crate::client::testing::{RecordingInstaller, RecordingRestarter, StaticFeed};
    use crate::keys::SigningKey;
    use crate::policy::payload_file_name;
    use crate::signing::sign;

    const TARGET: &str = "linux-x86_64";
    const PROGRAM: &[u8] = b"\x7fELF new buildrail";

    fn key_a() -> SigningKey {
        SigningKey::from_seed(&[1; 32])
    }

    fn key_b() -> SigningKey {
        SigningKey::from_seed(&[2; 32])
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn url(version: &str, target: &str) -> String {
        format!(
            "https://github.com/markcastle/buildrail/releases/download/v{version}/{}",
            payload_file_name(version, target)
        )
    }

    /// A correctly signed entry for `gz` as `version`/`target`.
    fn entry(key: &SigningKey, version: &str, target: &str, gz: &[u8]) -> TargetEntry {
        let sha256 = sha256_hex(gz);
        TargetEntry {
            url: url(version, target),
            signature: sign(key, version, target, &sha256),
            sha256,
            size: len_u64(gz),
        }
    }

    fn manifest(key: &SigningKey, version: &str, targets: Vec<(&str, TargetEntry)>) -> Manifest {
        Manifest {
            schema: 1,
            version: version.to_owned(),
            notes_url: format!("https://github.com/markcastle/buildrail/releases/tag/v{version}"),
            key_id: key.public_key().key_id(),
            targets: targets
                .into_iter()
                .map(|(t, e)| (t.to_owned(), e))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    struct Rig {
        updater: Updater,
        feed: Arc<StaticFeed>,
        installer: Arc<RecordingInstaller>,
        restarter: Arc<RecordingRestarter>,
    }

    fn rig_with(current: &str, feed: StaticFeed, key: Option<&SigningKey>) -> Rig {
        let feed = Arc::new(feed);
        let installer = Arc::new(RecordingInstaller::new());
        let restarter = Arc::new(RecordingRestarter::new());
        let updater = Updater::new(current, feed.clone(), installer.clone(), restarter.clone())
            .unwrap()
            .with_public_key(key.map(SigningKey::public_key))
            .with_target(Some(TARGET));
        Rig {
            updater,
            feed,
            installer,
            restarter,
        }
    }

    /// A feed with a properly signed 0.3.0 release of `PROGRAM`, plus the payload.
    fn good_release() -> (Manifest, Vec<u8>) {
        let gz = gzip(PROGRAM);
        let m = manifest(
            &key_a(),
            "0.3.0",
            vec![(TARGET, entry(&key_a(), "0.3.0", TARGET, &gz))],
        );
        (m, gz)
    }

    fn feed_for(m: &Manifest, gz: &[u8]) -> StaticFeed {
        let url = m.targets[TARGET].url.clone();
        StaticFeed::new(m.to_json()).with_payload(url, gz.to_vec())
    }

    async fn available(rig: &Rig) -> Update {
        match rig.updater.check().await {
            UpdateStatus::Available(update) => update,
            other => panic!("expected an update, got {other:?}"),
        }
    }

    pub(crate) fn sample_update() -> Update {
        let (m, _) = good_release();
        Update {
            version: Version::new(0, 3, 0),
            notes_url: m.notes_url.clone(),
            target: TARGET,
            entry: m.targets[TARGET].clone(),
            key_id: m.key_id,
        }
    }

    #[tokio::test]
    async fn round_trip_check_install_restart() {
        let (m, gz) = good_release();
        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        let update = available(&rig).await;
        assert_eq!(update.version, Version::new(0, 3, 0));
        assert_eq!(update.notes_url, m.notes_url);
        assert_eq!(
            UpdateStatus::Available(update.clone()).to_string(),
            "BuildRail 0.3.0 is available."
        );
        rig.updater.install(&update).await.unwrap();
        assert_eq!(rig.installer.installed(), vec![PROGRAM.to_vec()]);
        assert_eq!(rig.feed.downloads(), vec![m.targets[TARGET].url.clone()]);
        rig.updater.restart().unwrap();
        assert_eq!(rig.restarter.calls(), 1);
        assert_eq!(rig.updater.current_version(), &Version::new(0, 2, 0));
        assert!(format!("{:?}", rig.updater).contains("0.2.0"));
    }

    #[tokio::test]
    async fn not_configured_without_key_or_target() {
        let (m, gz) = good_release();
        let rig = rig_with("0.2.0", feed_for(&m, &gz), None);
        let status = rig.updater.check().await;
        assert_eq!(status, UpdateStatus::NotConfigured);
        assert_eq!(status.to_string(), "This build does not update itself.");
        assert_eq!(
            rig.updater.install(&sample_update()).await,
            Err(InstallError::NotConfigured)
        );
        assert!(rig.feed.downloads().is_empty());

        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        let updater = rig.updater.with_target(None);
        assert_eq!(updater.check().await, UpdateStatus::NotConfigured);
        // The default constructor uses the embedded key, absent in tests.
        let plain = Updater::new(
            "0.2.0",
            Arc::new(StaticFeed::new("{}")),
            Arc::new(RecordingInstaller::new()),
            Arc::new(RecordingRestarter::new()),
        )
        .unwrap();
        if embedded_public_key().is_none() {
            assert_eq!(plain.check().await, UpdateStatus::NotConfigured);
        }
    }

    #[tokio::test]
    async fn equal_and_older_versions_are_not_offered() {
        let (m, gz) = good_release();
        for current in ["0.3.0", "0.4.0", "0.3.0+local"] {
            let rig = rig_with(current, feed_for(&m, &gz), Some(&key_a()));
            let status = rig.updater.check().await;
            assert!(matches!(status, UpdateStatus::UpToDate { .. }), "{current}");
            assert_eq!(status.to_string(), "Up to date.");
        }
    }

    #[tokio::test]
    async fn tampered_payload_is_rejected_and_nothing_installed() {
        let (m, gz) = good_release();
        let mut evil = gz.clone();
        let last = evil.len() - 1;
        evil[last] ^= 1;
        let rig = rig_with("0.2.0", feed_for(&m, &evil), Some(&key_a()));
        let update = available(&rig).await;
        assert_eq!(
            rig.updater.install(&update).await,
            Err(InstallError::HashMismatch)
        );
        assert!(rig.installer.installed().is_empty());
    }

    #[tokio::test]
    async fn swapped_payload_with_matching_hash_fails_the_signature() {
        // The attacker controls the manifest's hash and size too, but not the signature.
        let (mut m, _) = good_release();
        let evil = gzip(b"\x7fELF evil");
        let entry = m.targets.get_mut(TARGET).unwrap();
        entry.sha256 = sha256_hex(&evil);
        entry.size = len_u64(&evil);
        let rig = rig_with("0.2.0", feed_for(&m, &evil), Some(&key_a()));
        let update = available(&rig).await;
        let err = rig.updater.install(&update).await.unwrap_err();
        assert!(matches!(err, InstallError::BadSignature(_)));
        assert!(rig.installer.installed().is_empty());
    }

    #[tokio::test]
    async fn relabelled_old_payload_is_rejected() {
        // 0.2.5's genuine signed payload, presented as 0.3.0.
        let gz = gzip(b"\x7fELF old 0.2.5");
        let mut old = entry(&key_a(), "0.2.5", TARGET, &gz);
        old.url = url("0.3.0", TARGET);
        let m = manifest(&key_a(), "0.3.0", vec![(TARGET, old)]);
        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        let update = available(&rig).await;
        assert!(matches!(
            rig.updater.install(&update).await,
            Err(InstallError::BadSignature(_))
        ));
        assert!(rig.installer.installed().is_empty());
    }

    #[tokio::test]
    async fn another_platforms_payload_is_rejected() {
        let gz = gzip(PROGRAM);
        let windows = entry(&key_a(), "0.3.0", "windows-x86_64", &gz);
        let m = manifest(&key_a(), "0.3.0", vec![(TARGET, windows)]);
        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        let update = available(&rig).await;
        assert!(matches!(
            rig.updater.install(&update).await,
            Err(InstallError::BadSignature(_))
        ));
    }

    #[tokio::test]
    async fn manifest_version_bumped_after_signing_is_rejected() {
        let (mut m, gz) = good_release();
        m.version = "0.4.0".into();
        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        let update = available(&rig).await;
        assert!(matches!(
            rig.updater.install(&update).await,
            Err(InstallError::BadSignature(_))
        ));
    }

    #[tokio::test]
    async fn signed_by_another_key() {
        let gz = gzip(PROGRAM);
        // Signed with B, honestly labelled B: a clear "different key" at check time.
        let m = manifest(
            &key_b(),
            "0.3.0",
            vec![(TARGET, entry(&key_b(), "0.3.0", TARGET, &gz))],
        );
        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        let status = rig.updater.check().await;
        assert_eq!(status, UpdateStatus::Failed(CheckError::DifferentKey));
        assert!(status.to_string().contains("signed with a different key"));

        // Signed with B but claiming A's id: the signature itself fails.
        let mut lying = m.clone();
        lying.key_id = key_a().public_key().key_id();
        let rig = rig_with("0.2.0", feed_for(&lying, &gz), Some(&key_a()));
        let update = available(&rig).await;
        assert!(matches!(
            rig.updater.install(&update).await,
            Err(InstallError::BadSignature(_))
        ));

        // An update found by a build with key A, installed by a build with key B.
        let (m, gz) = good_release();
        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        let update = available(&rig).await;
        let rig_b = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_b()));
        let err = rig_b.updater.install(&update).await.unwrap_err();
        assert_eq!(err, InstallError::DifferentKey);
        assert!(err.to_string().contains("signed with a different key"));
    }

    #[tokio::test]
    async fn non_github_urls_are_refused_before_fetching() {
        let (mut m, gz) = good_release();
        m.targets.get_mut(TARGET).unwrap().url = "https://evil.example/buildrail.gz".into();
        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        assert_eq!(
            rig.updater.check().await,
            UpdateStatus::Failed(CheckError::UnsafeUrl(UrlError::NotBuildRailRelease))
        );
        let mut update = sample_update();
        update.entry.url = "https://evil.example/buildrail.gz".into();
        let err = rig.updater.install(&update).await.unwrap_err();
        assert_eq!(err, InstallError::UnsafeUrl(UrlError::NotBuildRailRelease));
        assert!(rig.feed.downloads().is_empty());
    }

    #[tokio::test]
    async fn oversized_manifest_is_refused() {
        let (m, gz) = good_release();
        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        let updater = rig.updater.with_limits(Limits {
            manifest_bytes: 10,
            ..Limits::default()
        });
        assert_eq!(
            updater.check().await,
            UpdateStatus::Failed(CheckError::ManifestTooLarge)
        );
    }

    #[tokio::test]
    async fn oversized_or_empty_payloads_are_refused() {
        let (m, gz) = good_release();
        let small = Limits {
            payload_bytes: 5,
            ..Limits::default()
        };
        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        let update = available(&rig).await;
        let updater = rig.updater.with_limits(small);
        assert_eq!(
            updater.check().await,
            UpdateStatus::Failed(CheckError::PayloadTooLarge)
        );
        assert_eq!(updater.install(&update).await, Err(InstallError::TooLarge));

        let mut empty = m.clone();
        empty.targets.get_mut(TARGET).unwrap().size = 0;
        let rig = rig_with("0.2.0", feed_for(&empty, &gz), Some(&key_a()));
        assert_eq!(
            rig.updater.check().await,
            UpdateStatus::Failed(CheckError::PayloadTooLarge)
        );
    }

    #[tokio::test]
    async fn download_bigger_or_smaller_than_announced() {
        let (m, gz) = good_release();
        let mut longer = gz.clone();
        longer.push(0);
        let rig = rig_with("0.2.0", feed_for(&m, &longer), Some(&key_a()));
        let update = available(&rig).await;
        let size = m.targets[TARGET].size;
        assert_eq!(
            rig.updater.install(&update).await,
            Err(InstallError::Download(FetchError::TooLarge { max: size }))
        );
        let rig = rig_with("0.2.0", feed_for(&m, &gz[..gz.len() - 1]), Some(&key_a()));
        assert_eq!(
            rig.updater.install(&update).await,
            Err(InstallError::SizeMismatch)
        );
        assert!(rig.installer.installed().is_empty());
    }

    /// Signs arbitrary payload bytes for `TARGET` and runs check + install.
    async fn install_signed(gz: Vec<u8>, limits: Limits) -> Result<Vec<Vec<u8>>, InstallError> {
        let m = manifest(
            &key_a(),
            "0.3.0",
            vec![(TARGET, entry(&key_a(), "0.3.0", TARGET, &gz))],
        );
        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        let updater = rig.updater.with_limits(limits);
        let UpdateStatus::Available(update) = updater.check().await else {
            panic!("expected an update");
        };
        updater.install(&update).await?;
        Ok(rig.installer.installed())
    }

    #[tokio::test]
    async fn gzip_bombs_are_capped() {
        let bomb = gzip(&vec![0u8; 100_000]);
        assert!(bomb.len() < 1_000);
        let limits = Limits {
            executable_bytes: 50_000,
            ..Limits::default()
        };
        assert_eq!(
            install_signed(bomb, limits).await,
            Err(InstallError::ExecutableTooLarge)
        );
    }

    #[tokio::test]
    async fn signed_but_broken_payloads_are_refused() {
        let defaults = Limits::default();
        assert_eq!(
            install_signed(b"not gzip at all".to_vec(), defaults).await,
            Err(InstallError::Decompress)
        );
        assert_eq!(
            install_signed(gzip(b"#!/bin/sh\necho hi"), defaults).await,
            Err(InstallError::NotAnExecutable(TARGET))
        );
        assert_eq!(
            install_signed(gzip(b""), defaults).await,
            Err(InstallError::NotAnExecutable(TARGET))
        );
    }

    #[test]
    fn executable_magic_table() {
        let cases: [(&'static str, &[u8], bool); 9] = [
            ("windows-x86_64", b"MZ\x90\x00", true),
            ("windows-x86_64", b"\x7fELF", false),
            ("linux-x86_64", b"\x7fELF\x02", true),
            ("linux-x86_64", b"MZ", false),
            ("macos-aarch64", &[0xcf, 0xfa, 0xed, 0xfe, 0x0c], true),
            ("macos-x86_64", &[0xca, 0xfe, 0xba, 0xbe], true),
            ("macos-x86_64", &[0xce, 0xfa, 0xed, 0xfe], true),
            ("macos-x86_64", &[0xfe, 0xed, 0xfa, 0xcf], true),
            ("macos-aarch64", b"\x7fELF", false),
        ];
        for (target, bytes, ok) in cases {
            assert_eq!(
                check_executable(target, bytes).is_ok(),
                ok,
                "{target} {bytes:?}"
            );
        }
    }

    #[tokio::test]
    async fn installer_failure_is_passed_on() {
        let (m, gz) = good_release();
        let feed = Arc::new(feed_for(&m, &gz));
        let installer = Arc::new(RecordingInstaller::failing("disk full"));
        let updater = Updater::new(
            "0.2.0",
            feed,
            installer.clone(),
            Arc::new(RecordingRestarter::new()),
        )
        .unwrap()
        .with_public_key(Some(key_a().public_key()))
        .with_target(Some(TARGET));
        let UpdateStatus::Available(update) = updater.check().await else {
            panic!("expected an update");
        };
        assert_eq!(
            updater.install(&update).await,
            Err(InstallError::Replace("disk full".into()))
        );
        assert_eq!(installer.installed().len(), 1);
    }

    #[tokio::test]
    async fn feed_problems_become_failed_statuses() {
        let (m, gz) = good_release();
        let cases: Vec<(StaticFeed, CheckError)> = vec![
            (
                StaticFeed::failing(FetchError::Status(404)),
                CheckError::Fetch(FetchError::Status(404)),
            ),
            (
                StaticFeed::new(m.to_json().replace("\"schema\": 1", "\"schema\": 2")),
                CheckError::Manifest(ManifestError::UnsupportedSchema(2)),
            ),
            (
                StaticFeed::new(m.to_json().replace("0.3.0\"", "v0.3\"")),
                CheckError::BadVersion,
            ),
        ];
        for (feed, expected) in cases {
            let rig = rig_with("0.2.0", feed, Some(&key_a()));
            assert_eq!(rig.updater.check().await, UpdateStatus::Failed(expected));
        }
        let rig = rig_with(
            "0.2.0",
            StaticFeed::failing(FetchError::Status(404)),
            Some(&key_a()),
        );
        assert_eq!(
            rig.updater.check().await.to_string(),
            "Update check failed: the server answered HTTP 404."
        );
        let schema2 = StaticFeed::new(m.to_json().replace("\"schema\": 1", "\"schema\": 2"));
        let rig = rig_with("0.2.0", schema2, Some(&key_a()));
        assert!(rig
            .updater
            .check()
            .await
            .to_string()
            .contains("needs a newer BuildRail"));
        let _ = gz;
    }

    #[tokio::test]
    async fn a_newer_release_without_this_platform_is_up_to_date_not_a_failure() {
        // Most releases are Windows-only; a Linux copy must stay calm.
        let (m, _gz) = good_release();
        let windows_only = m.to_json().replace(TARGET, "windows-x86_64");
        let rig = rig_with("0.2.0", StaticFeed::new(windows_only.clone()), Some(&key_a()));
        let status = rig.updater.check().await;
        assert_eq!(
            status,
            UpdateStatus::UpToDate {
                current: Version::new(0, 2, 0)
            }
        );
        assert_eq!(status.to_string(), "Up to date.");
        let found = rig.updater.find_update(&key_a().public_key(), TARGET).await;
        assert!(
            matches!(found, Ok(Found::NoBuildForTarget { ref version }) if *version == Version::new(0, 3, 0)),
            "{found:?}"
        );
        // Checked before the key: a release signed with another key that
        // has nothing for this platform is not this copy's concern either.
        let other_key = windows_only.replace(&key_a().public_key().key_id(), "0000000000000000");
        let rig = rig_with("0.2.0", StaticFeed::new(other_key), Some(&key_a()));
        assert!(matches!(rig.updater.check().await, UpdateStatus::UpToDate { .. }));
        // And the schedule treats it like any other quiet answer.
        let mut schedule = crate::client::UpdateSchedule::new();
        schedule.record(std::time::Duration::ZERO, &status);
        assert!(schedule.next_check_at().is_some());
    }

    #[tokio::test]
    async fn notes_links_outside_the_repository_are_replaced() {
        let (mut m, gz) = good_release();
        m.notes_url = "https://evil.example/phish".into();
        let rig = rig_with("0.2.0", feed_for(&m, &gz), Some(&key_a()));
        assert_eq!(available(&rig).await.notes_url, RELEASES_PAGE);
        assert_eq!(
            safe_notes_url("https://github.com/markcastle/buildrail/x y"),
            RELEASES_PAGE
        );
    }

    #[test]
    fn setup_errors() {
        let err = Updater::new(
            "not-a-version",
            Arc::new(StaticFeed::new("{}")),
            Arc::new(RecordingInstaller::new()),
            Arc::new(RecordingRestarter::new()),
        )
        .unwrap_err();
        assert_eq!(err, SetupError::InvalidVersion("not-a-version".into()));
        assert!(err.to_string().contains("not a semantic version"));
        assert!(Updater::for_this_app(reqwest::Client::new(), "x").is_err());
        let updater = Updater::for_this_app(reqwest::Client::new(), "0.2.0").unwrap();
        assert_eq!(updater.limits, Limits::default());
        assert_eq!(
            SetupError::Exe("gone".into()).to_string(),
            "could not locate the running program: gone"
        );
    }

    #[test]
    fn every_install_error_reads_as_one_sentence() {
        let errors = [
            InstallError::NotConfigured,
            InstallError::DifferentKey,
            InstallError::UnsafeUrl(UrlError::Malformed),
            InstallError::TooLarge,
            InstallError::Download(FetchError::Network("could not connect".into())),
            InstallError::SizeMismatch,
            InstallError::HashMismatch,
            InstallError::BadSignature(crate::SignatureError::Mismatch),
            InstallError::Decompress,
            InstallError::ExecutableTooLarge,
            InstallError::NotAnExecutable("macos-aarch64"),
            InstallError::Replace("denied".into()),
        ];
        for error in errors {
            let text = error.to_string();
            assert!(
                text.starts_with("Update not installed: ") && text.ends_with('.'),
                "{text}"
            );
            assert!(!text.contains('\n'));
        }
        let checks = [
            CheckError::ManifestTooLarge,
            CheckError::BadVersion,
            CheckError::DifferentKey,
            CheckError::PayloadTooLarge,
            CheckError::Fetch(FetchError::UnexpectedHost("https://x".into())),
            CheckError::Fetch(FetchError::NotAllowed),
            CheckError::Fetch(FetchError::TooLarge { max: 1 }),
        ];
        for error in checks {
            let line = UpdateStatus::Failed(error).to_string();
            assert!(
                line.starts_with("Update check failed: ") && line.ends_with('.'),
                "{line}"
            );
        }
    }
}
