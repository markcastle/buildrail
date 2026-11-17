//! Small egui widgets, each a rendition of one Task Rail component with
//! its metrics from the system's stylesheet (`components/bundle.css`).
//!
//! Every widget takes the [`Tokens`] it paints with and knows nothing about
//! BuildRail: no controller, no settings machine, no strings of its own.
//! Each reports an AccessKit role and name, is reachable with Tab, and
//! shows the system's focus ring (a solid 2 px `accent` outline, 2 px clear
//! of the control) while it has keyboard focus.
//!
//! | Widget | Task Rail component |
//! | --- | --- |
//! | [`toggle`] | `Toggle` |
//! | [`segmented`] | `Segmented` |
//! | [`button`] | `Button` |
//! | [`icon_button`] | `IconButton` |
//! | [`text_field`] | `TextField` |
//! | [`group`] | `SettingsGroup` (group, row, group label) |
//! | [`page_header`] | `PageHeader` |
//! | [`nav`] | `SettingsNav` |
//! | [`choice_cards`] | `ChoiceCards` |
//! | [`connector_card`] | `ConnectorCard` |
//! | [`display_picker`] | `DisplayPicker` |
//! | [`list_row`] | a picker row, built from `SettingsGroup`'s row |
//! | [`toast`] | `Toast` (inline) and the group's `saved` mark |

pub mod button;
pub mod choice_cards;
pub mod connector_card;
pub mod display_picker;
pub mod group;
pub mod icon_button;
pub mod list_row;
pub mod nav;
pub mod page_header;
pub mod segmented;
pub mod text_field;
pub mod toast;
pub mod toggle;

use std::sync::Arc;

use eframe::egui::{CornerRadius, Galley, Painter, Rect, Stroke, StrokeKind, Ui};

use super::theme::{Tokens, TypeStyle};

pub use button::{back_button, button, ButtonKind};
pub use choice_cards::{choice_cards, Choice};
pub use connector_card::{connector_card, CardAction, ConnectorCard};
pub use display_picker::{display_picker, DisplayTile};
pub use group::{group, Group};
pub use icon_button::{icon_button, icon_button_at};
pub use list_row::list_row;
pub use nav::{nav_item, NavItem};
pub use page_header::{page_header, subhead};
pub use segmented::segmented;
pub use text_field::TextField;
pub use toast::{error_line, saved_mark, toast, Tone};
pub use toggle::toggle;

/// The system's focus ring: a solid 2 px `accent` outline, 2 px clear of
/// `rect`, following its corners.
pub fn focus_ring(painter: &Painter, rect: Rect, radius: u8, tokens: &Tokens) {
    painter.rect_stroke(
        rect.expand(2.0),
        CornerRadius::same(radius.saturating_add(2)),
        Stroke::new(2.0, tokens.accent),
        StrokeKind::Outside,
    );
}

/// Lays out `text` in `style` and `color`, on one line.
pub fn text(ui: &Ui, style: TypeStyle, text: &str, color: eframe::egui::Color32) -> Arc<Galley> {
    ui.painter().layout_job(style.job(text, color))
}

/// Lays out `text` in `style` and `color`, wrapped at `width`.
pub fn wrapped(
    ui: &Ui,
    style: TypeStyle,
    text: &str,
    color: eframe::egui::Color32,
    width: f32,
) -> Arc<Galley> {
    ui.painter().layout_job(style.wrapped(text, color, width))
}
