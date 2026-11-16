//! The tunable numbers behind refresh scheduling.

use std::time::Duration;

/// How often and how cautiously to poll providers.
///
/// Kept as plain data so tests (and, later, settings) can tighten or relax
/// the cadence without touching scheduling logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshPolicy {
    /// Poll interval while the selected run is finished (or there is none).
    /// Finished runs rarely change, so this is slow to save rate limit.
    pub idle: Duration,
    /// Poll interval while the selected run is queued or running, so the
    /// strip turns green/red soon after a build finishes.
    pub active: Duration,
    /// Delay after the first consecutive failure. Each further failure
    /// doubles it, up to [`backoff_max`](Self::backoff_max). A short first
    /// delay recovers quickly from a blip; doubling avoids hammering a
    /// provider that is down.
    pub backoff_base: Duration,
    /// Upper bound for the failure backoff. Reconnect / wake refreshes
    /// immediately anyway, so this only bounds staleness while nothing
    /// external tells us the network is back.
    pub backoff_max: Duration,
    /// How long to pause a provider + connection after a rate limit when
    /// the provider did not say (`retry_after: None`).
    pub rate_limit_default: Duration,
    /// If wall-clock time advanced by more than this *beyond* the
    /// monotonic time we actually waited, the machine was asleep (or the
    /// clock jumped) and everything is refreshed at once.
    pub wake_threshold: Duration,
    /// The engine never sleeps longer than this in one go, so a wake from
    /// system sleep is noticed promptly even though monotonic timers do
    /// not count the time spent suspended.
    pub wake_check_interval: Duration,
    /// A single provider call that takes longer than this is abandoned and
    /// treated as offline, so a hung connection cannot freeze a watch.
    pub request_timeout: Duration,
}

impl RefreshPolicy {
    /// The regular poll interval after a successful refresh.
    pub fn interval(&self, active: bool) -> Duration {
        if active {
            self.active
        } else {
            self.idle
        }
    }

    /// Delay after `failures` consecutive failures (`failures >= 1`):
    /// `backoff_base * 2^(failures - 1)`, capped at `backoff_max`.
    /// `0` is treated like `1`.
    pub fn backoff(&self, failures: u32) -> Duration {
        let exponent = failures.saturating_sub(1).min(31);
        self.backoff_base
            .checked_mul(1u32 << exponent)
            .map_or(self.backoff_max, |delay| delay.min(self.backoff_max))
    }
}

impl Default for RefreshPolicy {
    /// The product defaults: 60 s idle, 15 s active, failure backoff from
    /// 30 s doubling up to 10 min.
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(60),
            active: Duration::from_secs(15),
            backoff_base: Duration::from_secs(30),
            backoff_max: Duration::from_secs(10 * 60),
            rate_limit_default: Duration::from_secs(60),
            wake_threshold: Duration::from_secs(30),
            wake_check_interval: Duration::from_secs(5),
            request_timeout: Duration::from_secs(30),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let p = RefreshPolicy::default();
        assert_eq!(p.interval(false), Duration::from_secs(60));
        assert_eq!(p.interval(true), Duration::from_secs(15));
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let p = RefreshPolicy::default();
        assert_eq!(p.backoff(0), Duration::from_secs(30));
        assert_eq!(p.backoff(1), Duration::from_secs(30));
        assert_eq!(p.backoff(2), Duration::from_secs(60));
        assert_eq!(p.backoff(3), Duration::from_secs(120));
        assert_eq!(p.backoff(5), Duration::from_secs(480));
        assert_eq!(p.backoff(6), Duration::from_secs(600));
        assert_eq!(p.backoff(40), Duration::from_secs(600));
        assert_eq!(p.backoff(u32::MAX), Duration::from_secs(600));
    }

    #[test]
    fn backoff_overflow_falls_back_to_max() {
        let p = RefreshPolicy {
            backoff_base: Duration::MAX,
            ..RefreshPolicy::default()
        };
        assert_eq!(p.backoff(3), p.backoff_max);
    }
}
