//! The egui layer: draws the view-models and settings machines, nothing
//! more.
//!
//! * [`strip`] – the 24 px strip widget (entries, overflow, detail popup).
//! * [`glyphs`] – vector status icons and provider marks.
//! * [`theme`] – light/dark palettes with tested WCAG-AA contrast.
//! * [`settings`] – the Settings window (tabs, wizard, import, device).
//! * [`app`] – [`BuildRailUi`], one frame of the whole UI; used by the real
//!   window and by `egui_kittest` tests alike.
//! * [`window`] – the eframe glue (window options, dock wiring). The only
//!   part that needs a real window.

pub mod app;
pub mod glyphs;
pub mod settings;
pub mod strip;
pub mod theme;
pub mod window;

pub use app::{BuildRailUi, FrameOutput};
pub use strip::{StripAction, StripView, STRIP_HEIGHT};
pub use theme::{Palette, DARK, LIGHT};
