//! What the strip and Settings › About say about updates, decided without
//! a window.
//!
//! The policy (from the release playbook): updating automatically is off
//! by default; an available update is offered with one quiet element on
//! the strip and never interrupts; the app does not explain the update,
//! it links to the release page; Settings answers "Check for updates" in
//! one line.
//!
//! * [`UpdateState`] – what the app knows: the last check's answer,
//!   whether a check or an install is running, the last install error,
//!   and the version the person said "Not now" to (for this run only).
//! * [`UpdateAnswer`] – the one line About shows.
//! * [`UpdateChip`] – the strip's offer, when there is one.
//! * [`is_quiet_moment`] / [`should_install_automatically`] – when an
//!   automatic install may start.

use buildrail_core::domain::{BuildStatus, WatchSnapshot};
use buildrail_update::{decide, Action, Update, UpdateStatus, Version};

/// Where people download BuildRail by hand.
pub const RELEASES_PAGE: &str = "https://github.com/markcastle/buildrail/releases";

/// About's sentence for a build that cannot update itself.
pub const NOT_CONFIGURED_LINE: &str =
    "This build does not update itself. Download new versions from the releases page.";

/// What the app knows about updates. Shared between the background check
/// loop, install jobs and the UI; every change goes through the
/// controller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateState {
    /// The answer to the last check, scheduled or asked for.
    pub status: Option<UpdateStatus>,
    /// A check asked for in Settings is running.
    pub checking: bool,
    /// The person asked in Settings at least once, so About shows every
    /// answer (not only an available update).
    pub asked: bool,
    /// An install is running (or finished and the app is restarting).
    pub installing: bool,
    /// Why the last install failed, in one line.
    pub install_error: Option<String>,
    /// The version the person said "Not now" to; forgotten at exit.
    pub dismissed: Option<Version>,
    /// An automatic install of this version failed; it is offered instead.
    pub auto_failed: Option<Version>,
    /// The install finished: the app should restart into the new version.
    pub restart_ready: bool,
}

impl UpdateState {
    /// The update the last check found, if any.
    pub fn available(&self) -> Option<&Update> {
        match &self.status {
            Some(UpdateStatus::Available(update)) => Some(update),
            _ => None,
        }
    }

    /// Records a check's answer. A different version than before starts
    /// afresh (no old error, no old "Not now").
    pub fn record(&mut self, status: UpdateStatus) {
        let before = self.available().map(|u| u.version.clone());
        let after = match &status {
            UpdateStatus::Available(update) => Some(update.version.clone()),
            _ => None,
        };
        if after.is_some() && after != before {
            self.install_error = None;
        }
        self.status = Some(status);
        self.checking = false;
    }

    /// The line About shows. Scheduled checks that find nothing new (or
    /// fail) are only logged; About shows them once the person asked.
    pub fn answer(&self) -> UpdateAnswer {
        if self.checking {
            return UpdateAnswer::Checking;
        }
        match &self.status {
            None => UpdateAnswer::Nothing,
            Some(UpdateStatus::NotConfigured) => UpdateAnswer::NotConfigured,
            Some(UpdateStatus::Available(update)) => UpdateAnswer::Available(Offer {
                version: update.version.to_string(),
                notes_url: update.notes_url.clone(),
                installing: self.installing,
                error: self.install_error.clone(),
                dismissed: self.dismissed.as_ref() == Some(&update.version),
            }),
            Some(status @ UpdateStatus::UpToDate { .. }) if self.asked => {
                UpdateAnswer::UpToDate(status.to_string())
            }
            Some(status @ UpdateStatus::Failed(_)) if self.asked => {
                UpdateAnswer::Failed(status.to_string())
            }
            Some(_) => UpdateAnswer::Nothing,
        }
    }

    /// The strip's offer: an available update the person has not said
    /// "Not now" to. With automatic updates on it stays quiet (the update
    /// installs at a quiet moment) unless it is installing or failed.
    pub fn chip(&self, update_automatically: bool) -> Option<UpdateChip> {
        let UpdateAnswer::Available(offer) = self.answer() else {
            return None;
        };
        if offer.dismissed {
            return None;
        }
        if update_automatically && !offer.installing && offer.error.is_none() {
            return None;
        }
        Some(UpdateChip(offer))
    }
}

/// An available update as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// "0.3.0".
    pub version: String,
    /// The release page ("What's new").
    pub notes_url: String,
    /// Installing now; the buttons are disabled.
    pub installing: bool,
    /// The last install failed: why, in one line.
    pub error: Option<String>,
    /// The person said "Not now" to this version.
    pub dismissed: bool,
}

impl Offer {
    /// "BuildRail 0.3.0 is available."
    pub fn headline(&self) -> String {
        format!("BuildRail {} is available.", self.version)
    }
}

/// The one line About shows under "Updates".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateAnswer {
    /// Nothing to say yet.
    Nothing,
    /// "Checking…"
    Checking,
    /// "Up to date."
    UpToDate(String),
    /// "Update check failed: …."
    Failed(String),
    /// This build cannot update itself.
    NotConfigured,
    /// A newer release can be installed.
    Available(Offer),
}

impl UpdateAnswer {
    /// The sentence, if there is one.
    pub fn line(&self) -> Option<String> {
        match self {
            Self::Nothing => None,
            Self::Checking => Some("Checking…".to_owned()),
            Self::UpToDate(line) | Self::Failed(line) => Some(line.clone()),
            Self::NotConfigured => Some(NOT_CONFIGURED_LINE.to_owned()),
            // While installing, the button says "Installing…".
            Self::Available(offer) => Some(offer.headline()),
        }
    }
}

/// The strip's update offer: a small accent chip before the + and
/// settings wells, with a card on hover or focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateChip(pub Offer);

impl UpdateChip {
    /// The word on the chip.
    pub fn label(&self) -> &'static str {
        if self.0.installing {
            "Installing…"
        } else {
            "Update"
        }
    }

    /// What a screen reader says; activating the chip opens Settings on
    /// About, where the update is installed.
    pub fn accessible_name(&self) -> String {
        if self.0.installing {
            format!(
                "Installing BuildRail {}. Open update options",
                self.0.version
            )
        } else {
            format!("{} Open update options", self.0.headline())
        }
    }

    /// The card's first line.
    pub fn headline(&self) -> String {
        self.0.headline()
    }

    /// The release page.
    pub fn notes_url(&self) -> &str {
        &self.0.notes_url
    }

    /// Installing now.
    pub fn installing(&self) -> bool {
        self.0.installing
    }

    /// Why the last install failed.
    pub fn error(&self) -> Option<&str> {
        self.0.error.as_deref()
    }
}

/// A quiet moment to install: no watched build is queued or running (the
/// person is probably waiting on it), and Settings is closed (they are in
/// the middle of something).
pub fn is_quiet_moment(snapshots: &[WatchSnapshot], settings_open: bool) -> bool {
    !settings_open
        && !snapshots.iter().any(|snapshot| {
            snapshot
                .record
                .as_ref()
                .is_some_and(|r| matches!(r.status, BuildStatus::Queued | BuildStatus::Running))
        })
}

/// Whether to start installing the available update now, without asking.
pub fn should_install_automatically(
    state: &UpdateState,
    update_automatically: bool,
    quiet: bool,
) -> bool {
    let Some(status) = &state.status else {
        return false;
    };
    let Action::InstallNow(update) = decide(status, update_automatically) else {
        return false;
    };
    quiet
        && !state.installing
        && !state.restart_ready
        && state.auto_failed.as_ref() != Some(&update.version)
        && state.dismissed.as_ref() != Some(&update.version)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use buildrail_core::domain::{BuildRecord, Freshness, RunId, WatchId};
    use buildrail_update::{CheckError, FetchError};
    use chrono::{TimeZone, Utc};

    use super::*;

    fn available(version: &str) -> UpdateStatus {
        UpdateStatus::Available(buildrail_update::testing::update(
            Version::parse(version).unwrap(),
            "https://github.com/markcastle/buildrail/releases/tag/v0.3.0",
        ))
    }

    fn up_to_date() -> UpdateStatus {
        UpdateStatus::UpToDate {
            current: Version::new(0, 2, 0),
        }
    }

    fn failed() -> UpdateStatus {
        UpdateStatus::Failed(CheckError::Fetch(FetchError::Status(503)))
    }

    fn snapshot(status: BuildStatus) -> WatchSnapshot {
        let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        WatchSnapshot {
            watch_id: WatchId::new("w"),
            record: Some(BuildRecord {
                run_id: RunId::new("1"),
                attempt: 1,
                status,
                url: String::new(),
                branch: None,
                commit: None,
                failing_job: None,
                created_at: at,
                updated_at: at,
                observed_at: at,
            }),
            freshness: Freshness::Fresh,
            last_success: Some(at),
        }
    }

    #[test]
    fn about_answers_in_one_line() {
        let mut state = UpdateState::default();
        assert_eq!(state.answer(), UpdateAnswer::Nothing);
        assert_eq!(state.answer().line(), None);
        // Scheduled checks that find nothing are only logged...
        state.record(up_to_date());
        assert_eq!(state.answer(), UpdateAnswer::Nothing);
        state.record(failed());
        assert_eq!(state.answer(), UpdateAnswer::Nothing);
        // ...until the person asks.
        state.asked = true;
        state.checking = true;
        assert_eq!(state.answer().line().unwrap(), "Checking…");
        state.record(up_to_date());
        assert!(!state.checking);
        assert_eq!(state.answer().line().unwrap(), "Up to date.");
        state.record(failed());
        assert_eq!(
            state.answer().line().unwrap(),
            "Update check failed: the server answered HTTP 503."
        );
        state.record(UpdateStatus::NotConfigured);
        assert_eq!(state.answer().line().unwrap(), NOT_CONFIGURED_LINE);
        state.record(available("0.3.0"));
        assert_eq!(
            state.answer().line().unwrap(),
            "BuildRail 0.3.0 is available."
        );
        state.installing = true;
        assert_eq!(
            state.answer().line().unwrap(),
            "BuildRail 0.3.0 is available."
        );
    }

    #[test]
    fn the_chip_offers_until_not_now_and_stays_quiet_when_automatic() {
        let mut state = UpdateState::default();
        assert_eq!(state.chip(false), None);
        state.record(available("0.3.0"));
        let chip = state.chip(false).unwrap();
        assert_eq!(chip.label(), "Update");
        assert_eq!(
            chip.accessible_name(),
            "BuildRail 0.3.0 is available. Open update options"
        );
        assert_eq!(chip.headline(), "BuildRail 0.3.0 is available.");
        assert!(chip.notes_url().ends_with("/v0.3.0"));
        assert!(!chip.installing() && chip.error().is_none());
        // Automatic: installs at a quiet moment instead of offering.
        assert_eq!(state.chip(true), None);
        state.installing = true;
        let chip = state.chip(true).unwrap();
        assert_eq!(chip.label(), "Installing…");
        assert_eq!(
            chip.accessible_name(),
            "Installing BuildRail 0.3.0. Open update options"
        );
        state.installing = false;
        state.install_error = Some("Update not installed: no disk.".into());
        assert_eq!(
            state.chip(true).unwrap().error().unwrap(),
            "Update not installed: no disk."
        );
        // Not now hides this version only.
        state.dismissed = Some(Version::new(0, 3, 0));
        assert_eq!(state.chip(false), None);
        assert!(matches!(state.answer(), UpdateAnswer::Available(o) if o.dismissed));
        state.record(available("0.3.1"));
        assert!(state.chip(false).is_some());
        assert_eq!(state.install_error, None, "a new version starts afresh");
    }

    #[test]
    fn quiet_moments() {
        assert!(is_quiet_moment(&[], false));
        assert!(!is_quiet_moment(&[], true), "Settings is open");
        assert!(is_quiet_moment(&[snapshot(BuildStatus::Failed)], false));
        assert!(!is_quiet_moment(&[snapshot(BuildStatus::Running)], false));
        assert!(!is_quiet_moment(&[snapshot(BuildStatus::Queued)], false));
        let mut pending = snapshot(BuildStatus::Passed);
        pending.record = None;
        assert!(is_quiet_moment(&[pending], false));
    }

    #[test]
    fn automatic_install_waits_for_its_moment() {
        let mut state = UpdateState::default();
        assert!(!should_install_automatically(&state, true, true));
        state.record(up_to_date());
        assert!(!should_install_automatically(&state, true, true));
        state.record(available("0.3.0"));
        // (preference, quiet, expected)
        for (auto, quiet, expected) in [
            (true, true, true),
            (false, true, false),
            (true, false, false),
        ] {
            assert_eq!(
                should_install_automatically(&state, auto, quiet),
                expected,
                "auto={auto} quiet={quiet}"
            );
        }
        state.installing = true;
        assert!(!should_install_automatically(&state, true, true));
        state.installing = false;
        state.auto_failed = Some(Version::new(0, 3, 0));
        assert!(
            !should_install_automatically(&state, true, true),
            "offered instead"
        );
        state.auto_failed = None;
        state.dismissed = Some(Version::new(0, 3, 0));
        assert!(!should_install_automatically(&state, true, true));
        state.dismissed = None;
        state.restart_ready = true;
        assert!(!should_install_automatically(&state, true, true));
    }
}
