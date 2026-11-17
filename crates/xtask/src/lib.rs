//! BuildRail's release tooling.
//!
//! ```text
//! cargo run -p xtask -- release --dry-run     what the next release would be
//! cargo run -p xtask -- changelog check       is CHANGELOG.md in shape
//! cargo run -p xtask -- --help                everything else
//! ```
//!
//! The Release workflow (`.github/workflows/release.yml`) runs the same
//! commands, so a release cut by the button and one rehearsed in a terminal
//! follow one path. How releasing works as a whole is in
//! `docs/releasing.md`.
//!
//! The crate has no dependencies, so CI builds it in seconds. The logic is
//! pure functions over strings ([`changelog`], [`manifest`], [`version`],
//! [`date`]); the release flow ([`release`]) reaches git, cargo and the disk
//! only through two small traits, which the tests replace with fakes.

pub mod changelog;
pub mod cli;
pub mod date;
pub mod manifest;
pub mod release;
pub mod system;
pub mod version;
