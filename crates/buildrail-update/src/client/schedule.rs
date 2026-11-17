//! When to check, and what to do with the answer. Pure: time is passed in.

use std::time::Duration;

use crate::client::updater::{Update, UpdateStatus};

/// First check, this long after start, so it stays out of startup's way.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);

/// Between checks while things work: a release reaches people the same day.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// After the first failure in a row, try again this soon (a blip); after
/// that, fall back to [`CHECK_INTERVAL`].
pub const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(5 * 60);

/// The check timetable, measured as time since the app started.
///
/// * first check at 20 s;
/// * then hourly;
/// * after a failed check, retry in 5 minutes, then hourly while it keeps
///   failing;
/// * after [`UpdateStatus::NotConfigured`], never again (nothing changes
///   while the app runs).
///
/// "Check for updates" in Settings is not scheduled; it calls
/// [`crate::Updater::check`] and may [`record`](Self::record) the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSchedule {
    next: Option<Duration>,
    failures_in_a_row: u32,
}

impl Default for UpdateSchedule {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateSchedule {
    /// A schedule whose first check is due at [`FIRST_CHECK_DELAY`].
    pub fn new() -> Self {
        Self {
            next: Some(FIRST_CHECK_DELAY),
            failures_in_a_row: 0,
        }
    }

    /// Whether a check is due at `elapsed` since start.
    pub fn is_due(&self, elapsed: Duration) -> bool {
        self.next.is_some_and(|next| elapsed >= next)
    }

    /// When the next check is due (since start); `None` means never.
    pub fn next_check_at(&self) -> Option<Duration> {
        self.next
    }

    /// How long from `elapsed` until the next check (zero if overdue).
    pub fn wait_from(&self, elapsed: Duration) -> Option<Duration> {
        self.next.map(|next| next.saturating_sub(elapsed))
    }

    /// Records the outcome of a check made at `elapsed` and moves the timetable on.
    pub fn record(&mut self, elapsed: Duration, status: &UpdateStatus) {
        let delay = match status {
            UpdateStatus::NotConfigured => {
                self.next = None;
                return;
            }
            UpdateStatus::Failed(_) => {
                self.failures_in_a_row = self.failures_in_a_row.saturating_add(1);
                if self.failures_in_a_row == 1 {
                    RETRY_AFTER_FAILURE
                } else {
                    CHECK_INTERVAL
                }
            }
            UpdateStatus::UpToDate { .. } | UpdateStatus::Available(_) => {
                self.failures_in_a_row = 0;
                CHECK_INTERVAL
            }
        };
        self.next = Some(elapsed.saturating_add(delay));
    }
}

/// The user's update preferences.
///
/// Updating automatically is off by default: an update that changes things
/// under someone is a surprise, so the app offers it and waits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpdatePrefs {
    /// Install without asking when an update is found.
    pub auto_install: bool,
}

/// What the app should do after a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing to do (up to date, failed, or not configured; log the status).
    Nothing,
    /// Show "BuildRail x.y.z is available." with **Install now**; never interrupt.
    Offer(Update),
    /// Install straight away, then restart.
    InstallNow(Update),
}

/// Turns a check result and the auto-install preference into an [`Action`].
pub fn decide(status: &UpdateStatus, auto_install: bool) -> Action {
    match status {
        UpdateStatus::Available(update) if auto_install => Action::InstallNow(update.clone()),
        UpdateStatus::Available(update) => Action::Offer(update.clone()),
        _ => Action::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::feed::FetchError;
    use crate::client::updater::CheckError;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn failed() -> UpdateStatus {
        UpdateStatus::Failed(CheckError::Fetch(FetchError::Status(500)))
    }

    fn up_to_date() -> UpdateStatus {
        UpdateStatus::UpToDate {
            current: semver::Version::new(0, 2, 0),
        }
    }

    #[test]
    fn first_check_after_twenty_seconds_then_hourly() {
        let mut s = UpdateSchedule::default();
        assert!(!s.is_due(secs(19)));
        assert!(s.is_due(secs(20)));
        assert_eq!(s.wait_from(secs(5)), Some(secs(15)));
        s.record(secs(21), &up_to_date());
        assert_eq!(s.next_check_at(), Some(secs(21 + 3600)));
        assert!(!s.is_due(secs(3620)));
        assert!(s.is_due(secs(3621)));
        assert_eq!(s.wait_from(secs(9999)), Some(Duration::ZERO));
    }

    #[test]
    fn failures_back_off_then_recover() {
        let mut s = UpdateSchedule::new();
        // (elapsed, status, expected next)
        let table = [
            (20, failed(), 20 + 300),
            (320, failed(), 320 + 3600),
            (3920, failed(), 3920 + 3600),
            (7520, up_to_date(), 7520 + 3600),
            (11120, failed(), 11120 + 300),
        ];
        for (at, status, next) in table {
            s.record(secs(at), &status);
            assert_eq!(
                s.next_check_at(),
                Some(secs(next)),
                "after {status} at {at}"
            );
        }
    }

    #[test]
    fn not_configured_stops_checking() {
        let mut s = UpdateSchedule::new();
        s.record(secs(20), &UpdateStatus::NotConfigured);
        assert_eq!(s.next_check_at(), None);
        assert!(!s.is_due(secs(1_000_000)));
        assert_eq!(s.wait_from(secs(0)), None);
    }

    #[test]
    fn decide_table() {
        let update = crate::client::updater::tests::sample_update();
        let available = UpdateStatus::Available(update.clone());
        assert!(!UpdatePrefs::default().auto_install);
        let table = [
            (available.clone(), false, Action::Offer(update.clone())),
            (available.clone(), true, Action::InstallNow(update.clone())),
            (up_to_date(), true, Action::Nothing),
            (failed(), true, Action::Nothing),
            (UpdateStatus::NotConfigured, true, Action::Nothing),
        ];
        for (status, auto, expected) in table {
            assert_eq!(decide(&status, auto), expected, "{status} auto={auto}");
        }
        // Available keeps the hourly rhythm too.
        let mut s = UpdateSchedule::new();
        s.record(secs(20), &available);
        assert_eq!(s.next_check_at(), Some(secs(3620)));
    }
}
