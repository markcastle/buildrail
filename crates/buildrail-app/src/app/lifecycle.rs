//! What the running app holds, and letting go of it.
//!
//! BuildRail holds two things another process may be waiting for: the
//! reserved screen edge (a Windows AppBar, an X11 strut) and the
//! single-instance lock. Both must be given back on every way out (Quit,
//! closing the window, Ctrl+C or SIGTERM, a panic unwinding) and, above
//! all, *before* a self-update starts the new version: otherwise the new
//! process finds the lock taken and quits, or docks next to a ghost
//! reservation.
//!
//! Each such thing is a [`Held`] registered with the [`Lifecycle`].
//! [`Lifecycle::release_all`] lets go of everything (idempotent, safe to
//! call from several exit paths); [`ReleaseOnDrop`] calls it when it goes
//! out of scope, including during unwinding.

use std::sync::{Arc, Mutex, PoisonError};

use buildrail_platform::single_instance::InstanceGuard;

/// Something held for the life of the process. `release` must be
/// idempotent and must not block for long.
pub trait Held: Send + Sync {
    /// What it is, for logs ("screen edge", "single-instance lock").
    fn name(&self) -> &'static str;
    /// Lets go of it.
    fn release(&self) -> Result<(), String>;
}

/// The things held, in the order they were taken. Clones share the list.
#[derive(Clone, Default)]
pub struct Lifecycle {
    held: Arc<Mutex<Vec<Arc<dyn Held>>>>,
}

impl std::fmt::Debug for Lifecycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.list().iter().map(|h| h.name()).collect();
        f.debug_struct("Lifecycle").field("held", &names).finish()
    }
}

impl Lifecycle {
    /// Nothing held.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers something to release later.
    pub fn hold(&self, held: Arc<dyn Held>) {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(held);
    }

    fn list(&self) -> Vec<Arc<dyn Held>> {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Releases everything, last taken first, and returns what failed
    /// (`"name: why"`). Everything stays registered, so a later call
    /// releases again (each `Held` makes that a no-op).
    pub fn release_all(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for held in self.list().iter().rev() {
            match held.release() {
                Ok(()) => tracing::debug!(what = held.name(), "released"),
                Err(error) => {
                    tracing::warn!(what = held.name(), %error, "could not release");
                    problems.push(format!("{}: {error}", held.name()));
                }
            }
        }
        problems
    }
}

/// Releases a [`Lifecycle`] when dropped: keep one alive for as long as
/// the app runs, so leaving that scope in any way (return, error, panic
/// unwinding) gives everything back.
#[derive(Debug)]
pub struct ReleaseOnDrop(pub Lifecycle);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release_all();
    }
}

/// The single-instance lock as a [`Held`]: releasing drops the guard.
pub struct InstanceLock(Mutex<Option<InstanceGuard>>);

impl InstanceLock {
    /// Holds `guard` until released.
    pub fn new(guard: InstanceGuard) -> Self {
        Self(Mutex::new(Some(guard)))
    }

    /// `true` until released.
    pub fn is_held(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }
}

impl Held for InstanceLock {
    fn name(&self) -> &'static str {
        "single-instance lock"
    }

    fn release(&self) -> Result<(), String> {
        drop(self.0.lock().unwrap_or_else(PoisonError::into_inner).take());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::{AtomicUsize, Ordering};

    use buildrail_platform::single_instance::{InMemoryInstances, SingleInstance};

    use super::*;

    struct Counting {
        name: &'static str,
        releases: AtomicUsize,
        order: Arc<Mutex<Vec<&'static str>>>,
        fail: bool,
    }

    impl Held for Counting {
        fn name(&self) -> &'static str {
            self.name
        }
        fn release(&self) -> Result<(), String> {
            self.releases.fetch_add(1, Ordering::SeqCst);
            self.order.lock().unwrap().push(self.name);
            if self.fail {
                Err("busy".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn releases_last_taken_first_and_reports_failures() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let make = |name, fail| {
            Arc::new(Counting {
                name,
                releases: AtomicUsize::new(0),
                order: order.clone(),
                fail,
            })
        };
        let (a, b) = (make("a", false), make("b", true));
        let lifecycle = Lifecycle::new();
        lifecycle.hold(a.clone());
        lifecycle.hold(b.clone());
        assert!(format!("{lifecycle:?}").contains("\"a\""));
        assert_eq!(lifecycle.release_all(), vec!["b: busy".to_owned()]);
        assert_eq!(*order.lock().unwrap(), vec!["b", "a"]);
        {
            let _guard = ReleaseOnDrop(lifecycle.clone());
        }
        assert_eq!(a.releases.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn instance_lock_lets_the_next_instance_start() {
        let instances = InMemoryInstances::new();
        let lock = Arc::new(InstanceLock::new(instances.acquire("BuildRail").unwrap()));
        assert!(lock.is_held());
        let lifecycle = Lifecycle::new();
        lifecycle.hold(lock.clone());
        assert!(lifecycle.release_all().is_empty());
        assert!(!lock.is_held());
        assert!(!instances.is_held("BuildRail"));
        assert_eq!(lock.name(), "single-instance lock");
        // Idempotent.
        assert!(lock.release().is_ok());
    }
}
