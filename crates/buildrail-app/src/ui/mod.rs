//! The egui layer: draws the view-models and settings machines, nothing
//! more, in the Task Rail design system.
//!
//! * [`theme`] – the design tokens: colour (light and dark, contrast
//!   tested), type, space, radius and the card shadow.
//! * [`fonts`] – the operating system's own interface font, found on disk
//!   behind the [`fonts::FontSource`] seam.
//! * [`style`] – the tokens applied to egui's own style.
//! * [`glyphs`] – vector glyphs in the lucide manner and provider marks.
//! * [`icon`] – the BuildRail mark (window icon; source of the `.ico`).
//! * [`widgets`] – small widgets, one per Task Rail component.
//! * [`strip`] – the 24 px strip widget (entries, overflow, detail popup).
//! * [`settings`] – the Settings window (navigation, pages, wizard, import).
//! * [`app`] – [`BuildRailUi`], one frame of the whole UI; used by the real
//!   window and by `egui_kittest` tests alike.
//! * [`window`] – the eframe glue (window options, dock wiring). The only
//!   part that needs a real window.

pub mod app;
pub mod fonts;
pub mod glyphs;
pub mod icon;
pub mod settings;
pub mod strip;
pub mod style;
pub mod theme;
pub mod widgets;
pub mod window;

pub use app::{BuildRailUi, FrameCounter, FrameOutput};
pub use strip::{StripAction, StripView, STRIP_HEIGHT};
pub use theme::{Tokens, DARK, LIGHT};
