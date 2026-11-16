//! # BuildRail profile and device storage
//!
//! This crate owns everything BuildRail keeps on disk:
//!
//! * the **profile** – the ordered list of pinned watches. It is portable:
//!   you can export it and a teammate can import it on any OS;
//! * the **device preferences** – monitor, placement, scale, startup,
//!   theme and the registry of *local* connections. They stay on this
//!   machine and are never exported;
//! * the **snapshot cache** – last known build states, for instant startup
//!   and offline display.
//!
//! Credentials are in none of these. They live in the OS credential store,
//! keyed by connection alias (see
//! [`buildrail_core::ports::CredentialReader`]). No type in this crate has a
//! field that could hold a secret, so an export *cannot* contain one.
//!
//! ## The profile file
//!
//! A profile is plain TOML that is pleasant to read and edit by hand:
//!
//! ```toml
//! # BuildRail profile. Watches appear on the strip in this order.
//! version = 1
//!
//! [[watch]]
//! id = "4f6c1a52-93d3-4bd5-8f0e-6f7c1b2a9e10"
//! label = "API CI"
//! provider = "github"
//! connection = "work-github"
//! owner = "acme"
//! repo = "api"
//! workflow = "ci.yml"        # file name, or a numeric workflow ID
//! branch = "main"            # optional: leave out to watch every branch
//!
//! [[watch]]
//! id = "marketing-site"      # any short ID works for hand-written files
//! label = "Marketing site"
//! provider = "cloudflare"
//! connection = "acme-cf"
//! account_id = "0123456789abcdef0123456789abcdef"
//! worker = "marketing-site"
//! ```
//!
//! Rules (all enforced by one validator, shared with the picker – see
//! [`validate`]):
//!
//! | key | rule |
//! |---|---|
//! | `version` | required; `1`. Files from a newer BuildRail are refused with a "please update" error. |
//! | `id` | letters, digits, `-`, `_`; at most 64; unique in the profile. **Optional**: a watch without one gets a new UUID (so it will look "added" on every import of that file). |
//! | `label` | required; trimmed; 1–64 characters; no control characters. Need not be unique. |
//! | `provider` | `"github"` or `"cloudflare"`. |
//! | `connection` | the *local* connection alias: starts with a letter/digit, then letters, digits, `.`, `-`, `_`; at most 64. |
//! | `owner` | GitHub only: letters, digits, `-` (not first/last); at most 39. |
//! | `repo` | GitHub only: letters, digits, `.`, `-`, `_`; at most 100; not `owner/repo`. |
//! | `workflow` | GitHub only: `name.yml` / `name.yaml` (no path) or a numeric ID (may be unquoted). |
//! | `account_id` | Cloudflare only: exactly 32 hex characters (stored lower-case). |
//! | `worker` | Cloudflare only: lower-case letters, digits, `-`, `_`; at most 63. |
//! | `branch` | optional; a valid git branch name; must not be empty when present. |
//!
//! **Unknown keys are rejected**, at the top level and inside `[[watch]]`,
//! so a typo like `lable = "API"` is reported instead of silently losing
//! data. So are keys that belong to the other provider. Every problem in a
//! file is reported in one go, each with a machine-usable
//! [`IssueLocation`] and [`Field`] and a message such as
//! `watch 3 ("Keevoro"): repo must not be empty`.
//!
//! Format changes bump `version`; [`parse_profile`] has a `match version`
//! seam where older versions will be migrated.
//!
//! ## Editing
//!
//! [`Profile`] is immutable and always valid. Settings edits are pure
//! methods that return a new profile or an [`EditError`]:
//! [`Profile::with_added`], [`Profile::with_label`] (rename),
//! [`Profile::with_moved`] (reorder by index), [`Profile::without`]
//! (remove), [`Profile::with_connection`] (reconnect) and
//! [`Profile::with_replaced`] (edit a watch in the picker). Persist with
//! [`ProfileStore::save`].
//!
//! ## Export and import
//!
//! Export is [`profile_to_toml`]. Import is a three-step pipeline in which
//! nothing touches the disk until the last step:
//!
//! 1. [`ImportPreview::from_toml`] parses and validates the file (any
//!    problem is an error, listing them all) and compares it with the
//!    current profile: [`ImportPreview::diff`] lists added, removed, changed
//!    (and what changed) and unchanged watches and whether the order
//!    changed; [`ImportPreview::required_connections`] lists the distinct
//!    `(alias, provider)` pairs the file uses.
//! 2. The recipient maps each pair to one of *their* connections
//!    ([`DevicePrefs::connections`]). [`ImportPreview::suggested_mapping`]
//!    pre-fills obvious choices (same alias, or the only connection for
//!    that provider). A pair with no suitable connection is where the UI
//!    prompts for new local credentials.
//! 3. [`ImportPreview::apply`] checks the mapping – every missing,
//!    unknown or wrong-provider choice is reported – and returns the new
//!    profile, which replaces the old one through the atomic
//!    [`ProfileStore::save`].
//!
//! Import *replaces* the profile; it does not merge. A malformed file or a
//! bad mapping stops at step 1 or 3 and the working configuration stays
//! exactly as it was.
//!
//! ## Device preferences
//!
//! ```toml
//! version = 1
//! launch_at_startup = true
//! theme = "system"            # "system" | "light" | "dark"
//! scale = 1.25                # optional override, 0.5–4.0
//!
//! [monitor]                   # optional; absent means the primary monitor
//! id = '\\.\DISPLAY2'
//! name = "DELL U2720Q"        # fallback match when the id changed
//!
//! [placement]
//! mode = "floating"           # or "docked-bottom"
//! position = { x = 120, y = 40 }
//!
//! [connections.work-github]
//! provider = "github"
//! display_name = "octocat"
//! ```
//!
//! Unknown keys are ignored here (the file is machine-written, and an
//! older build should still start after a newer one added a setting).
//!
//! ## Storage
//!
//! [`AppPaths`] finds the per-OS directories. [`AtomicFile`] replaces a
//! file via temp file + fsync + rename, so a crash or a failed write never
//! leaves a half-written or damaged file; [`DocumentStore`] builds the
//! profile and device-preference stores on top of it, and
//! [`FileSnapshotCache`] the cache. A missing profile or preferences file
//! loads as the empty default (first launch).

pub mod cache;
pub mod device;
pub mod format;
pub mod import;
pub mod paths;
pub mod profile;
pub mod store;
pub mod validate;

pub use cache::FileSnapshotCache;
pub use device::{
    DevicePrefs, DevicePrefsStore, LocalConnection, MonitorChoice, Placement, Position, Theme,
};
pub use format::{parse_profile, profile_to_toml, DocumentError, PROFILE_VERSION};
pub use import::{
    AliasMapping, DiffCounts, DiffEntry, DiffKind, ImportError, ImportPreview, LocalConnections,
    MappingProblem, ProfileDiff, RequiredConnection, WatchChange,
};
pub use paths::{AppPaths, NoHomeDir};
pub use profile::{EditError, Profile};
pub use store::{AtomicFile, DocumentStore, ProfileStore, StoreError, StoredDocument};
pub use validate::{
    new_watch_id, validate_alias, validate_draft, validate_drafts, Field, IssueLocation,
    ValidationErrors, ValidationIssue, WatchDraft,
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod doc_examples {
    //! The TOML examples in the crate docs must stay valid.

    use super::*;

    fn toml_blocks() -> Vec<String> {
        let docs = include_str!("lib.rs");
        let mut blocks = Vec::new();
        let mut current: Option<String> = None;
        for line in docs.lines().take_while(|l| l.starts_with("//!")) {
            let body = line
                .trim_start_matches("//!")
                .strip_prefix(' ')
                .unwrap_or("");
            match (&mut current, body) {
                (None, "```toml") => current = Some(String::new()),
                (Some(_), "```") => blocks.extend(current.take()),
                (Some(block), text) => {
                    block.push_str(text);
                    block.push('\n');
                }
                (None, _) => {}
            }
        }
        blocks
    }

    #[test]
    fn crate_doc_examples_parse() {
        let blocks = toml_blocks();
        assert_eq!(blocks.len(), 2);
        let profile = parse_profile(&blocks[0]).unwrap();
        assert_eq!(profile.len(), 2);
        let prefs = <DevicePrefs as StoredDocument>::decode(&blocks[1]).unwrap();
        assert_eq!(prefs.monitor.unwrap().id, r"\\.\DISPLAY2");
    }
}
