//! BuildRail core: the parts of the app that know nothing about windows,
//! HTTP or the filesystem.
//!
//! * [`domain`] – the vocabulary: watches, targets, build records, statuses.
//! * [`details`] – what one run's details panel shows: jobs, trigger,
//!   timings ([`details::RunDetails`]), fetched on demand.
//! * [`ports`] – the traits the outside world implements (providers,
//!   credential store, clock, snapshot cache).
//! * [`selection`] – which run "wins" for a watch, and when a new observation
//!   may replace the one on screen.
//! * [`refresh`] – when to poll, how to back off, how to share requests.
//!
//! Dependency rule: this crate depends on nothing else in the workspace, and
//! every other crate depends on it.

pub mod details;
pub mod domain;
pub mod ports;
pub mod refresh;
pub mod selection;
