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
//! * [`details`] – [`DetailsModel::build`]: what a build's details panel
//!   says (this run, jobs failed first, recent runs, the actions), its
//!   wording of durations and triggers, and when its words next change.
//! * [`popup`] – where the strip's pop-up cards go, in physical pixels on
//!   the strip's own monitor, and when a pop-up window is in place to be
//!   shown.
//!
//! Nothing here depends on egui; the `ui` module draws these models.

pub mod details;
pub mod overflow;
pub mod popup;
pub mod repaint;
pub mod strip;
pub mod text;
pub mod update;

pub use details::{
    DetailsInput, DetailsModel, Fact, JobRow, JobsSection, LoadLine, MoreLink, Notice, PanelAction,
    PanelButton, RecentRun, RecentSection,
};
pub use overflow::{fit, OverflowGroup, OverflowLayout};
pub use popup::{
    monitor_under, popup_rect, PopupAlign, PopupPlacement, PopupRequest, PopupStep, StripSide,
};
pub use repaint::{snapshots_differ_visibly, Motion, RepaintPlanner};
pub use strip::{provider_mark, IconKind, StripEntry, StripModel};
pub use text::{age_long, age_short, short_commit};
pub use update::{
    is_quiet_moment, should_install_automatically, Offer, UpdateAnswer, UpdateChip, UpdateState,
};
