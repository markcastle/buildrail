//! Buttons, mirroring Task Rail's `Button`: 13 px medium labels, 8 × 14
//! padding, radius 9, in four weights.
//!
//! * [`ButtonKind::Primary`] – the one thing a panel is for: `accent` fill,
//!   `accent-text` label. At most one in view.
//! * [`ButtonKind::Plain`] – an action beside a primary one: `bg` with a
//!   `line` border.
//! * [`ButtonKind::Quiet`] – actions on something already there: no
//!   border, no fill until hovered, `text-muted`.
//! * [`ButtonKind::QuietDanger`] – removes something: quiet, in `priority`.
//!
//! Labels are verbs, verb first, in sentence case.

use eframe::egui::{CornerRadius, Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};

use super::{focus_ring, text};
use crate::ui::theme::{radius, type_style, Tokens};

/// A button's weight. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// The one main action.
    Primary,
    /// A plain action.
    Plain,
    /// An action on an existing thing.
    Quiet,
    /// Removes something.
    QuietDanger,
}

/// Draws a button. `enabled = false` dims it to 55 % and ignores clicks.
/// `compact` uses the card-action padding (5 × 8) of `ConnectorCard`.
pub fn button(
    ui: &mut Ui,
    tokens: &Tokens,
    kind: ButtonKind,
    label: &str,
    enabled: bool,
    compact: bool,
) -> Response {
    let pad = if compact {
        Vec2::new(8.0, 5.0)
    } else {
        Vec2::new(14.0, 8.0)
    };
    let (fill, ink, border) = match kind {
        ButtonKind::Primary => (Some(tokens.accent), tokens.accent_text, None),
        ButtonKind::Plain => (Some(tokens.bg), tokens.text, Some(tokens.line)),
        ButtonKind::Quiet => (None, tokens.text_muted, None),
        ButtonKind::QuietDanger => (None, tokens.priority, None),
    };
    let galley = text(ui, type_style::BUTTON, label, ink);
    let size = galley.size() + 2.0 * pad;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hovered = enabled && response.hovered();
    let fill = match (kind, hovered) {
        (ButtonKind::Primary, true) => Some(tokens.accent_hover()),
        (ButtonKind::QuietDanger, true) => Some(tokens.danger_hover()),
        (_, true) => Some(tokens.bg_hover),
        (_, false) => fill,
    };
    let ink = match (kind, hovered) {
        (ButtonKind::Quiet, true) => tokens.text,
        _ => ink,
    };
    let fade = |c: eframe::egui::Color32| if enabled { c } else { c.gamma_multiply(0.55) };
    let r9 = CornerRadius::same(radius::R9);
    let painter = ui.painter();
    if let Some(fill) = fill {
        painter.rect_filled(rect, r9, fade(fill));
    }
    if let Some(border) = border {
        painter.rect_stroke(rect, r9, Stroke::new(1.0, border), StrokeKind::Inside);
    }
    let galley = text(ui, type_style::BUTTON, label, fade(ink));
    painter.galley(rect.min + pad, galley, fade(ink));
    if response.has_focus() {
        focus_ring(painter, rect, radius::R9, tokens);
    }
    response
}

/// A quiet button led by `←`, for the way back ("Back", "Builds").
pub fn back_button(ui: &mut Ui, tokens: &Tokens, label: &str) -> Response {
    const GLYPH: f32 = 14.0;
    let pad = Vec2::new(10.0, 6.0);
    let galley = text(ui, type_style::BUTTON, label, tokens.text_muted);
    let size = Vec2::new(
        pad.x * 2.0 + GLYPH + 6.0 + galley.size().x,
        pad.y * 2.0 + galley.size().y,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    let ink = if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::R9), tokens.bg_hover);
        tokens.text
    } else {
        tokens.text_muted
    };
    let painter = ui.painter();
    let glyph_center = rect.left_center() + Vec2::new(pad.x + GLYPH / 2.0, 0.0);
    crate::ui::glyphs::paint_arrow_left(painter, glyph_center, GLYPH, ink);
    let galley = text(ui, type_style::BUTTON, label, ink);
    let at = rect.min + Vec2::new(pad.x + GLYPH + 6.0, pad.y);
    painter.galley(at, galley, ink);
    if response.has_focus() {
        focus_ring(painter, rect, radius::R9, tokens);
    }
    response
}
