//! Pure view-models: what the strip shows, decided without a window.
//!
//! * [`strip`] – [`StripModel::build`] turns pinned watches + engine
//!   snapshots into ordered [`StripEntry`]s (status word, icon shape, tone,
//!   staleness note, detail lines, accessible name, run URL).
//! * [`overflow`] – which entries fit the strip's width and what the
//!   overflow button says.
//! * [`text`] – relative ages and short commits.
//! * [`repaint`] – when the strip must be painted again
//!   ([`RepaintPlanner`]): only when what it shows changes.
//! * [`update`] – what the strip and About say about updates, and when an
//!   automatic install may start.
//!
//! Nothing here depends on egui; the `ui` module draws these models.

pub mod overflow;
pub mod repaint;
pub mod strip;
pub mod text;
pub mod update;

pub use overflow::{fit, OverflowGroup, OverflowLayout};
pub use repaint::{snapshots_differ_visibly, Motion, RepaintPlanner};
pub use strip::{provider_mark, IconKind, StripEntry, StripModel};
pub use text::{age_long, age_short, short_commit};
pub use update::{
    is_quiet_moment, should_install_automatically, Offer, UpdateAnswer, UpdateChip, UpdateState,
};
