//! The pure refresh state machine.
//!
//! [`Schedule`] tracks, for every distinct [`RequestKey`], when it is next
//! due, how many times in a row it failed and whether a request is in
//! flight. It never reads a clock: every method takes `now`, so tests can
//! walk through hours of polling in microseconds.
//!
//! Rate limits are tracked per *gate* (provider + connection), not per key:
//! a provider throttles a credential, not one URL, so when one request is
//! told to slow down every request using the same credential pauses.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use super::policy::RefreshPolicy;
use crate::domain::{ConnectionAlias, ProviderKind, RequestKey};

/// What happened to one request, as far as scheduling cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The provider answered. `active` is whether the selected run is still
    /// queued or running, which picks the fast or slow cadence.
    Success {
        /// The selected run can still change state.
        active: bool,
    },
    /// The request failed for any reason other than a rate limit
    /// (including "no credential" and "no provider registered"). Counts
    /// towards exponential backoff.
    Failure,
    /// The provider throttled us. Pauses the whole provider + connection
    /// gate; does not count as a failure because nothing is broken.
    RateLimited {
        /// How long the provider asked us to wait, if it said.
        retry_after: Option<Duration>,
    },
}

/// Requests sharing a credential on one provider share a rate limit.
type Gate = (ProviderKind, ConnectionAlias);

fn gate_of(key: &RequestKey) -> Gate {
    (key.target.provider(), key.connection.clone())
}

/// Scheduling state of one request key.
#[derive(Debug, Clone)]
struct Entry {
    key: RequestKey,
    next_due: Instant,
    failures: u32,
    in_flight: bool,
    /// A forced refresh arrived while a request was in flight; that request
    /// may have started before the event (e.g. before the network came
    /// back), so fetch again as soon as it completes.
    refresh_after_flight: bool,
}

/// Per-request refresh state machine. See the module docs.
///
/// Entries are kept in a `Vec` in pinned order: a strip holds tens of
/// watches at most, so linear lookups are cheaper than hashing and the
/// order of [`take_due`](Self::take_due) is deterministic.
#[derive(Debug, Clone)]
pub struct Schedule {
    policy: RefreshPolicy,
    entries: Vec<Entry>,
    gates: HashMap<Gate, Instant>,
}

impl Schedule {
    /// An empty schedule.
    pub fn new(policy: RefreshPolicy) -> Self {
        Self {
            policy,
            entries: Vec::new(),
            gates: HashMap::new(),
        }
    }

    /// The policy in force.
    pub fn policy(&self) -> &RefreshPolicy {
        &self.policy
    }

    /// Number of distinct request keys tracked.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when no request keys are tracked.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn entry(&self, key: &RequestKey) -> Option<&Entry> {
        self.entries.iter().find(|e| &e.key == key)
    }

    fn entry_mut(&mut self, key: &RequestKey) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| &e.key == key)
    }

    /// Replaces the set of keys (when the profile changes). Keys already
    /// known keep their timing, backoff and in-flight state, so editing one
    /// watch does not re-poll all the others; new keys are due at `now`;
    /// keys no longer listed are dropped. Duplicates are ignored. Rate-limit
    /// gates are kept: the provider still remembers us.
    pub fn sync_keys(&mut self, keys: impl IntoIterator<Item = RequestKey>, now: Instant) {
        let mut old = std::mem::take(&mut self.entries);
        for key in keys {
            if self.entries.iter().any(|e| e.key == key) {
                continue;
            }
            let entry = match old.iter().position(|e| e.key == key) {
                Some(index) => old.swap_remove(index),
                None => Entry {
                    key,
                    next_due: now,
                    failures: 0,
                    in_flight: false,
                    refresh_after_flight: false,
                },
            };
            self.entries.push(entry);
        }
    }

    /// The deadline of the rate-limit gate covering `key`, if one is set.
    /// Expired gates may still be reported until the next
    /// [`take_due`](Self::take_due); compare with `now`.
    pub fn gate_until(&self, key: &RequestKey) -> Option<Instant> {
        self.gates.get(&gate_of(key)).copied()
    }

    /// `true` while a request for `key` has been handed out and its outcome
    /// not yet recorded.
    pub fn is_in_flight(&self, key: &RequestKey) -> bool {
        self.entry(key).is_some_and(|e| e.in_flight)
    }

    /// Consecutive failures recorded for `key` (0 for unknown keys).
    pub fn failures(&self, key: &RequestKey) -> u32 {
        self.entry(key).map_or(0, |e| e.failures)
    }

    /// The earliest instant `key` may be requested: its own due time, or
    /// later if its gate is closed. `None` for unknown or in-flight keys.
    pub fn due_at(&self, key: &RequestKey) -> Option<Instant> {
        self.entry(key)
            .filter(|e| !e.in_flight)
            .map(|e| self.effective_due(e))
    }

    fn effective_due(&self, entry: &Entry) -> Instant {
        match self.gates.get(&gate_of(&entry.key)) {
            Some(&gate) if gate > entry.next_due => gate,
            _ => entry.next_due,
        }
    }

    /// Keys that should be requested now, in pinned order. Each returned key
    /// is marked in flight and will not be returned again until its outcome
    /// is [`record`](Self::record)ed, so a slow request is never duplicated.
    pub fn take_due(&mut self, now: Instant) -> Vec<RequestKey> {
        self.gates.retain(|_, until| *until > now);
        let gates = &self.gates;
        self.entries
            .iter_mut()
            .filter(|e| {
                !e.in_flight
                    && e.next_due <= now
                    && gates.get(&gate_of(&e.key)).is_none_or(|g| *g <= now)
            })
            .map(|e| {
                e.in_flight = true;
                e.key.clone()
            })
            .collect()
    }

    /// Records the outcome of a request handed out by
    /// [`take_due`](Self::take_due) and schedules the next one.
    ///
    /// A rate limit closes the gate even if `key` has since been removed,
    /// because the other requests on that credential are throttled too.
    pub fn record(&mut self, key: &RequestKey, outcome: Outcome, now: Instant) {
        if let Outcome::RateLimited { retry_after } = outcome {
            let until = now + retry_after.unwrap_or(self.policy.rate_limit_default);
            let gate = self.gates.entry(gate_of(key)).or_insert(until);
            if *gate < until {
                *gate = until;
            }
        }
        let policy = self.policy;
        let Some(entry) = self.entry_mut(key) else {
            return;
        };
        entry.in_flight = false;
        entry.next_due = match outcome {
            Outcome::Success { active } => {
                entry.failures = 0;
                now + policy.interval(active)
            }
            Outcome::Failure => {
                entry.failures = entry.failures.saturating_add(1);
                now + policy.backoff(entry.failures)
            }
            // The gate carries the deadline; `effective_due` applies it.
            Outcome::RateLimited { .. } => now,
        };
        if std::mem::take(&mut entry.refresh_after_flight) {
            entry.next_due = now;
        }
    }

    /// Makes every key due immediately and forgets accumulated backoff.
    /// Used for "refresh now", reconnect and wake from sleep. Rate-limit
    /// gates are *not* lifted: the provider would only refuse again, and
    /// ignoring `retry-after` risks a longer ban.
    pub fn refresh_all(&mut self, now: Instant) {
        for entry in &mut self.entries {
            entry.failures = 0;
            if entry.in_flight {
                entry.refresh_after_flight = true;
            } else {
                entry.next_due = now;
            }
        }
    }

    /// When the caller should next call [`take_due`](Self::take_due):
    /// the earliest effective due time among keys not in flight. `None`
    /// when nothing is waiting (no keys, or all in flight).
    pub fn next_wake(&self) -> Option<Instant> {
        self.entries
            .iter()
            .filter(|e| !e.in_flight)
            .map(|e| self.effective_due(e))
            .min()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::domain::WatchTarget;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn gh(conn: &str, repo: &str) -> RequestKey {
        RequestKey {
            connection: ConnectionAlias::new(conn),
            target: WatchTarget::GitHub {
                owner: "o".into(),
                repo: repo.into(),
                workflow: "ci.yml".into(),
                branch: None,
            },
        }
    }

    fn cf(conn: &str, worker: &str) -> RequestKey {
        RequestKey {
            connection: ConnectionAlias::new(conn),
            target: WatchTarget::Cloudflare {
                account_id: "acc".into(),
                worker: worker.into(),
                branch: None,
            },
        }
    }

    fn schedule_with(keys: &[RequestKey], now: Instant) -> Schedule {
        let mut s = Schedule::new(RefreshPolicy::default());
        s.sync_keys(keys.iter().cloned(), now);
        s
    }

    #[test]
    fn new_schedule_is_empty() {
        let s = Schedule::new(RefreshPolicy::default());
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert_eq!(s.next_wake(), None);
        assert_eq!(s.policy(), &RefreshPolicy::default());
    }

    #[test]
    fn new_keys_are_due_immediately_in_order_and_deduplicated() {
        let t0 = Instant::now();
        let a = gh("w", "a");
        let b = gh("w", "b");
        let mut s = schedule_with(&[a.clone(), b.clone(), a.clone()], t0);
        assert_eq!(s.len(), 2);
        assert!(!s.is_empty());
        assert_eq!(s.next_wake(), Some(t0));
        assert_eq!(s.take_due(t0), vec![a.clone(), b.clone()]);
        assert!(s.is_in_flight(&a));
        // In-flight keys are not handed out twice and do not set a wake.
        assert!(s.take_due(t0 + secs(1000)).is_empty());
        assert_eq!(s.next_wake(), None);
        assert_eq!(s.due_at(&a), None);
    }

    #[test]
    fn success_schedules_idle_or_active_interval() {
        let t0 = Instant::now();
        let a = gh("w", "a");
        let b = gh("w", "b");
        let mut s = schedule_with(&[a.clone(), b.clone()], t0);
        s.take_due(t0);
        s.record(&a, Outcome::Success { active: false }, t0);
        s.record(&b, Outcome::Success { active: true }, t0);
        assert_eq!(s.due_at(&a), Some(t0 + secs(60)));
        assert_eq!(s.due_at(&b), Some(t0 + secs(15)));
        assert_eq!(s.next_wake(), Some(t0 + secs(15)));
        assert!(s.take_due(t0 + secs(14)).is_empty());
        assert_eq!(s.take_due(t0 + secs(15)), vec![b.clone()]);
        assert_eq!(s.take_due(t0 + secs(60)), vec![a]);
    }

    #[test]
    fn failures_back_off_exponentially_and_reset_on_success() {
        let t0 = Instant::now();
        let a = gh("w", "a");
        let mut s = schedule_with(std::slice::from_ref(&a), t0);
        let mut now = t0;
        let expected = [30, 60, 120, 240, 480, 600, 600];
        for (i, wait) in expected.iter().enumerate() {
            assert_eq!(s.take_due(now), vec![a.clone()], "attempt {i}");
            s.record(&a, Outcome::Failure, now);
            assert_eq!(s.failures(&a), u32::try_from(i + 1).unwrap());
            assert_eq!(s.due_at(&a), Some(now + secs(*wait)));
            now += secs(*wait);
        }
        s.take_due(now);
        s.record(&a, Outcome::Success { active: false }, now);
        assert_eq!(s.failures(&a), 0);
        assert_eq!(s.due_at(&a), Some(now + secs(60)));
        // Next failure starts from the base again.
        now += secs(60);
        s.take_due(now);
        s.record(&a, Outcome::Failure, now);
        assert_eq!(s.due_at(&a), Some(now + secs(30)));
    }

    #[test]
    fn rate_limit_gates_every_key_on_same_provider_and_connection() {
        let t0 = Instant::now();
        let a = gh("work", "a");
        let b = gh("work", "b");
        let other_conn = gh("home", "c");
        let other_provider = cf("work", "w");
        let keys = [
            a.clone(),
            b.clone(),
            other_conn.clone(),
            other_provider.clone(),
        ];
        let mut s = schedule_with(&keys, t0);
        assert_eq!(s.take_due(t0).len(), 4);
        s.record(
            &a,
            Outcome::RateLimited {
                retry_after: Some(secs(300)),
            },
            t0,
        );
        for k in [&b, &other_conn, &other_provider] {
            s.record(k, Outcome::Success { active: true }, t0);
        }
        assert_eq!(s.failures(&a), 0, "rate limit is not a failure");
        assert_eq!(s.gate_until(&a), Some(t0 + secs(300)));
        assert_eq!(s.gate_until(&b), Some(t0 + secs(300)));
        assert_eq!(s.gate_until(&other_conn), None);
        assert_eq!(s.gate_until(&other_provider), None);
        assert_eq!(s.due_at(&a), Some(t0 + secs(300)));
        assert_eq!(s.due_at(&b), Some(t0 + secs(300)));

        let at15 = s.take_due(t0 + secs(15));
        assert_eq!(at15, vec![other_conn.clone(), other_provider.clone()]);
        s.record(
            &other_conn,
            Outcome::Success { active: false },
            t0 + secs(15),
        );
        s.record(
            &other_provider,
            Outcome::Success { active: false },
            t0 + secs(15),
        );
        assert!(s
            .take_due(t0 + secs(299))
            .iter()
            .all(|k| k != &a && k != &b));
        assert_eq!(s.take_due(t0 + secs(300)), vec![a.clone(), b]);
        assert_eq!(s.gate_until(&a), None, "expired gate cleared");
    }

    #[test]
    fn rate_limit_without_retry_after_uses_default_and_never_shortens() {
        let t0 = Instant::now();
        let a = gh("w", "a");
        let b = gh("w", "b");
        let mut s = schedule_with(&[a.clone(), b.clone()], t0);
        s.take_due(t0);
        s.record(
            &a,
            Outcome::RateLimited {
                retry_after: Some(secs(500)),
            },
            t0,
        );
        s.record(&b, Outcome::RateLimited { retry_after: None }, t0);
        assert_eq!(s.gate_until(&a), Some(t0 + secs(500)));

        let mut fresh = schedule_with(std::slice::from_ref(&a), t0);
        fresh.take_due(t0);
        fresh.record(&a, Outcome::RateLimited { retry_after: None }, t0);
        assert_eq!(fresh.gate_until(&a), Some(t0 + secs(60)));
        fresh.take_due(t0 + secs(60));
        fresh.record(
            &a,
            Outcome::RateLimited {
                retry_after: Some(secs(10)),
            },
            t0 + secs(60),
        );
        assert_eq!(fresh.gate_until(&a), Some(t0 + secs(70)));
    }

    #[test]
    fn rate_limit_for_removed_key_still_closes_gate() {
        let t0 = Instant::now();
        let a = gh("w", "a");
        let b = gh("w", "b");
        let mut s = schedule_with(&[a.clone(), b.clone()], t0);
        s.take_due(t0);
        s.record(&b, Outcome::Success { active: false }, t0);
        s.sync_keys([b.clone()], t0);
        s.record(
            &a,
            Outcome::RateLimited {
                retry_after: Some(secs(120)),
            },
            t0,
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s.due_at(&b), Some(t0 + secs(120)));
        // Outcomes for unknown keys are otherwise ignored.
        s.record(&a, Outcome::Failure, t0);
        assert_eq!(s.failures(&a), 0);
    }

    #[test]
    fn refresh_all_clears_backoff_but_respects_gates() {
        let t0 = Instant::now();
        let a = gh("w", "a");
        let limited = cf("w", "x");
        let mut s = schedule_with(&[a.clone(), limited.clone()], t0);
        s.take_due(t0);
        s.record(&a, Outcome::Failure, t0);
        s.record(&a, Outcome::Failure, t0);
        s.record(
            &limited,
            Outcome::RateLimited {
                retry_after: Some(secs(90)),
            },
            t0,
        );
        let t1 = t0 + secs(5);
        s.refresh_all(t1);
        assert_eq!(s.failures(&a), 0);
        assert_eq!(s.due_at(&a), Some(t1));
        assert_eq!(s.due_at(&limited), Some(t0 + secs(90)));
        assert_eq!(s.take_due(t1), vec![a.clone()]);
        s.record(&a, Outcome::Failure, t1);
        assert_eq!(s.due_at(&a), Some(t1 + secs(30)), "backoff restarted");
    }

    #[test]
    fn refresh_all_during_flight_refetches_after_completion() {
        let t0 = Instant::now();
        let a = gh("w", "a");
        let mut s = schedule_with(std::slice::from_ref(&a), t0);
        s.take_due(t0);
        s.refresh_all(t0 + secs(1));
        assert!(s.is_in_flight(&a));
        s.record(&a, Outcome::Success { active: false }, t0 + secs(2));
        assert_eq!(s.due_at(&a), Some(t0 + secs(2)));
        s.take_due(t0 + secs(2));
        s.record(&a, Outcome::Success { active: false }, t0 + secs(3));
        assert_eq!(s.due_at(&a), Some(t0 + secs(63)), "flag is one-shot");
    }

    #[test]
    fn sync_keys_keeps_state_for_unchanged_keys_and_drops_removed() {
        let t0 = Instant::now();
        let a = gh("w", "a");
        let b = gh("w", "b");
        let c = gh("w", "c");
        let mut s = schedule_with(&[a.clone(), b.clone()], t0);
        s.take_due(t0);
        s.record(&a, Outcome::Failure, t0);
        let t1 = t0 + secs(10);
        s.sync_keys([c.clone(), a.clone()], t1);
        assert_eq!(s.len(), 2);
        assert_eq!(s.failures(&a), 1);
        assert_eq!(s.due_at(&a), Some(t0 + secs(30)));
        assert_eq!(s.due_at(&c), Some(t1));
        assert_eq!(s.due_at(&b), None);
        assert!(!s.is_in_flight(&b));
        assert_eq!(s.next_wake(), Some(t1));
    }

    #[test]
    fn next_wake_reports_gate_when_later_than_due() {
        let t0 = Instant::now();
        let a = gh("w", "a");
        let mut s = schedule_with(std::slice::from_ref(&a), t0);
        s.take_due(t0);
        s.record(
            &a,
            Outcome::RateLimited {
                retry_after: Some(secs(42)),
            },
            t0,
        );
        assert_eq!(s.next_wake(), Some(t0 + secs(42)));
    }

    #[test]
    fn later_rate_limit_extends_the_gate() {
        let t0 = Instant::now();
        let a = gh("w", "a");
        let b = gh("w", "b");
        let mut s = schedule_with(&[a.clone(), b.clone()], t0);
        s.take_due(t0);
        s.record(
            &a,
            Outcome::RateLimited {
                retry_after: Some(secs(10)),
            },
            t0,
        );
        s.record(
            &b,
            Outcome::RateLimited {
                retry_after: Some(secs(100)),
            },
            t0,
        );
        assert_eq!(s.gate_until(&a), Some(t0 + secs(100)));
    }
}
