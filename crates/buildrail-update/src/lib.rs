//! BuildRail's self-update.
//!
//! BuildRail ships as one self-contained executable. To update, the app
//! downloads a newer copy of that executable, checks it really came from
//! the BuildRail release process, and swaps it in place of its own file.
//! There is no installer and no update server: the "server" is a small
//! signed file, `latest.json`, attached to the latest GitHub release.
//!
//! The crate has two layers:
//!
//! * **The core** (always compiled, no I/O): the [`Manifest`] format,
//!   Ed25519 [`sign`]ing and [`verify`]ing, key parsing and
//!   [`PublicKey::key_id`], the version policy ([`is_newer`]),
//!   [`current_target`] and [`FEED_URL`]. The release-time tool
//!   `buildrail-sign` uses only this.
//! * **The client** (feature `client`): fetching the feed over HTTPS,
//!   verifying and installing a payload, restarting, and the check
//!   schedule. The app enables it. Everything that touches the network,
//!   the file system or processes sits behind a small trait (`Feed`,
//!   `Installer`, `Restarter`) so the whole flow is tested with fakes.
//!
//! # The trust model, in plain words
//!
//! A release is signed with one Ed25519 key pair. The private half lives
//! only in the repository's secrets (`BUILDRAIL_UPDATE_SIGNING_KEY`); the
//! public half is baked into the app when it is compiled
//! (`BUILDRAIL_UPDATE_PUBLIC_KEY`, see [`embedded_public_key`]).
//!
//! **What the signature proves.** For each platform the release signs one
//! short message: the format tag, the version, the platform name and the
//! SHA-256 of the compressed executable ([`signed_message`]). If the
//! signature checks out against the embedded key, the downloaded bytes are
//! exactly what the release process produced *for that version and that
//! platform*. A tampered download, a file swapped on the release, an old
//! payload relabelled as a new version, or the Windows build offered to a
//! Mac all fail, and nothing is installed. The manifest itself is not
//! signed as a whole; everything in it that matters for safety is either
//! covered by the signature (version, platform, hash) or checked on its own
//! (download addresses must be BuildRail's GitHub release downloads, sizes
//! are capped).
//!
//! **What it does not prove.** It is not operating-system code signing.
//! A person who downloads BuildRail by hand still meets SmartScreen or
//! Gatekeeper on first run; that needs a Windows certificate and an Apple
//! Developer ID, which are separate. It also does not stop someone who
//! holds the private key, so the key never leaves the release secrets.
//! And it does not make an update good: a rollback is a new, higher
//! release of the old code, because the app only ever moves forward.
//!
//! **If the key is lost or rotated.** Generate a new pair
//! (`buildrail-sign generate`), store the new seed as the secret and the
//! new public key for builds, and release. Copies already installed carry
//! the old public key, so they refuse the new release and say it was
//! "signed with a different key" (the manifest's `key_id` names the key).
//! They fail safe: nothing is installed, and people install that one
//! release by hand. Back the seed up somewhere that is not the repository.
//!
//! **Builds without a key.** A developer build compiled without
//! `BUILDRAIL_UPDATE_PUBLIC_KEY` cannot verify anything, so it never
//! installs anything: the client reports `UpdateStatus::NotConfigured`
//! ("This build does not update itself."). So does a build for a platform
//! that has no release asset ([`current_target`] is `None`).
//!
//! # File formats
//!
//! **Keys.** The private key is the 32-byte Ed25519 seed, standard base64
//! on one line. The public key is the 32-byte Ed25519 public key, standard
//! base64. The key id is the first 16 lowercase hex characters of the
//! SHA-256 of the public key bytes.
//!
//! **Payload.** The bare executable, gzip-compressed, attached to the
//! release as `buildrail-v{version}-{target}.gz`, where `target` is one of
//! [`TARGETS`].
//!
//! **Feed.** `latest.json` at [`FEED_URL`]; GitHub redirects that address
//! to the file on whichever release is currently "latest":
//!
//! ```json
//! {
//!   "schema": 1,
//!   "version": "0.3.0",
//!   "notes_url": "https://github.com/markcastle/buildrail/releases/tag/v0.3.0",
//!   "key_id": "0123456789abcdef",
//!   "targets": {
//!     "windows-x86_64": {
//!       "url": "https://github.com/markcastle/buildrail/releases/download/v0.3.0/buildrail-v0.3.0-windows-x86_64.gz",
//!       "sha256": "<hex of the .gz>",
//!       "size": 1234567,
//!       "signature": "<base64 Ed25519 signature of signed_message(...)>"
//!     }
//!   }
//! }
//! ```
//!
//! Unknown fields are ignored. A `schema` above [`SCHEMA_VERSION`] means
//! the feed needs a newer updater; the check reports that, it does not
//! crash.
//!
//! # Wiring it into the app
//!
//! ```ignore
//! use buildrail_update::{decide, cleanup_after_update, Action, UpdatePrefs, UpdateSchedule, Updater};
//!
//! // At startup, before anything else: remove the copy a Windows update
//! // left beside the executable.
//! let _ = cleanup_after_update();
//!
//! let updater = Updater::for_this_app(http_client, env!("CARGO_PKG_VERSION"))?;
//! let mut schedule = UpdateSchedule::new();
//! // On each tick of the app's background loop (elapsed = time since start):
//! if schedule.is_due(elapsed) {
//!     let status = updater.check().await;
//!     tracing::info!("{status}");               // one line per check
//!     schedule.record(elapsed, &status);
//!     match decide(&status, prefs.auto_install) {
//!         Action::Nothing => {}
//!         Action::Offer(update) => { /* "BuildRail 0.3.0 is available." + Install now */ }
//!         Action::InstallNow(update) => {
//!             // release files/locks the app holds, then:
//!             updater.install(&update).await?;
//!             updater.restart()?;                // spawns the new copy
//!             std::process::exit(0);
//!         }
//!     }
//! }
//! ```
//!
//! `Updater::install` is `async` but decompresses and writes the file on
//! the calling thread; run it where a second or so of blocking is fine.
//! Settings' "Check for updates" calls `Updater::check` directly and
//! shows the status's one-line `Display`.

// Tests may unwrap; library code may not (checked when clippy builds the lib target).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod keys;
mod manifest;
mod policy;
mod signing;

pub use keys::{
    embedded_public_key, encode_seed, parse_embedded_public_key, KeyError, PublicKey, SigningKey,
    PUBLIC_KEY_ENV, SIGNING_KEY_ENV,
};
pub use manifest::{Manifest, ManifestError, TargetEntry, SCHEMA_VERSION};
pub use policy::{
    current_target, is_allowed_download_host, is_known_target, is_newer, payload_file_name,
    validate_asset_url, UrlError, ALLOWED_DOWNLOAD_HOSTS, FEED_URL, MAX_EXECUTABLE_BYTES,
    MAX_MANIFEST_BYTES, MAX_PAYLOAD_BYTES, RELEASE_DOWNLOAD_PREFIX, TARGETS,
};
pub use semver::Version;
pub use signing::{sha256_hex, sign, signed_message, verify, SignatureError, MESSAGE_TAG};

#[cfg(feature = "client")]
mod client;

#[cfg(feature = "client")]
pub use client::*;
