//! Refresh scheduling: when to poll, how to back off, how to share requests.
//!
//! The module is split so that every decision can be tested without a
//! runtime, and only the thin [`engine`] touches tokio:
//!
//! * [`policy`] – the tunable numbers (intervals, backoff, wake threshold).
//! * [`schedule`] – a pure, per-request state machine: given outcomes and
//!   the current instant, which requests are due and when to wake next.
//!   Rate-limit gates per provider + connection live here.
//! * [`board`] – a pure model of what the strip shows: watches in pinned
//!   order, the record selected for each shared request, cached seeds.
//! * [`fetch`] – one provider request and the mapping of its failures.
//! * [`wake`] – detecting system sleep from a wall-clock jump.
//! * [`save`] – when the snapshot cache is worth writing (material changes
//!   at once, timestamp drift every few minutes, the rest on shutdown).
//! * [`engine`] – the async driver that ties the above to the ports, plus
//!   the cheap [`RefreshHandle`] the (synchronous) UI uses.
//!
//! Watches with an equal [`RequestKey`](crate::domain::RequestKey) share one
//! provider call per refresh: the schedule and the board are keyed by the
//! request, not by the watch.

pub mod board;
pub mod engine;
pub mod fetch;
pub mod policy;
pub mod save;
pub mod schedule;
pub mod wake;

pub use board::Board;
pub use engine::{EngineStopped, RefreshEngine, RefreshHandle, Snapshots};
pub use fetch::{FetchError, SourceRegistry};
pub use policy::RefreshPolicy;
pub use save::{SavePolicy, SaveTracker};
pub use schedule::{Outcome, Schedule};
pub use wake::WakeDetector;
