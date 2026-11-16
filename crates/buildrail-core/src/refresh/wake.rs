//! Detecting wake from system sleep.
//!
//! Monotonic timers (what tokio sleeps on) stop while the machine is
//! suspended, but the wall clock keeps going. So if, around one wait, the
//! wall clock advanced much further than the monotonic clock, the machine
//! was asleep and everything on screen is out of date. Comparing the two
//! needs no platform hooks, which keeps core free of OS code; platforms
//! that *do* get a wake notification can additionally call
//! [`RefreshHandle::refresh_now`](super::RefreshHandle::refresh_now).

use std::time::Duration;

use chrono::TimeDelta;

/// Decides whether a pair of (wall, monotonic) elapsed times means the
/// machine was asleep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeDetector {
    threshold: Duration,
}

impl WakeDetector {
    /// A detector that reports a wake when wall time ran ahead of
    /// monotonic time by more than `threshold`.
    pub fn new(threshold: Duration) -> Self {
        Self { threshold }
    }

    /// `true` when `wall_elapsed` exceeds `monotonic_elapsed` by more than
    /// the threshold. A wall clock that went *backwards* (manual clock
    /// change) is not a wake.
    pub fn is_wake(&self, wall_elapsed: TimeDelta, monotonic_elapsed: Duration) -> bool {
        let Ok(wall) = wall_elapsed.to_std() else {
            return false;
        };
        wall.saturating_sub(monotonic_elapsed) > self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_large_forward_jumps() {
        let d = WakeDetector::new(Duration::from_secs(30));
        let mono = Duration::from_secs(5);
        assert!(!d.is_wake(TimeDelta::seconds(5), mono));
        assert!(
            !d.is_wake(TimeDelta::seconds(35), mono),
            "exactly threshold"
        );
        assert!(d.is_wake(TimeDelta::seconds(36), mono));
        assert!(d.is_wake(TimeDelta::hours(8), mono));
        assert!(
            !d.is_wake(TimeDelta::seconds(-3600), mono),
            "clock went back"
        );
        assert!(!d.is_wake(TimeDelta::seconds(1), mono), "wall slower");
    }
}
