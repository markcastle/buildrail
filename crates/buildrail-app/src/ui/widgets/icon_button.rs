//! A square button holding one glyph, mirroring Task Rail's `IconButton`:
//! a `bg-deep` well (radius 9 at 32 px) that turns `bg-hover` under the
//! pointer. It has no visible words, so it always carries an accessible
//! name and a tooltip that adds what the name cannot.

use eframe::egui::{
    CornerRadius, Painter, Pos2, Rect, Response, Role, Sense, Ui, Vec2, WidgetInfo,
};

use super::focus_ring;
use crate::ui::theme::Tokens;

/// Paints a glyph centred on a point, `size` across, in a colour.
pub type Glyph<'a> = &'a dyn Fn(&Painter, Pos2, f32, eframe::egui::Color32);

/// The well's corner radius for its size: 9 at the system's 32 px, scaled
/// down for the strip's smaller wells.
fn corner(side: f32) -> u8 {
    (side * 9.0 / 32.0).round() as u8
}

/// Draws an icon button in `rect` (placed by the caller, as on the strip).
pub fn icon_button_at(
    ui: &mut Ui,
    rect: Rect,
    tokens: &Tokens,
    name: &str,
    tooltip: &str,
    glyph: Glyph<'_>,
) -> Response {
    let response = ui.interact(
        rect,
        ui.scope_id().with(("icon-button", name)),
        Sense::click(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), name));
    let radius = corner(rect.height());
    let painter = ui.painter_at(rect.expand(4.0));
    let fill = if response.hovered() || response.is_pointer_button_down_on() {
        tokens.bg_hover
    } else {
        tokens.bg_deep
    };
    painter.rect_filled(rect, CornerRadius::same(radius), fill);
    // 18 px glyph in a 32 px well. A strip-sized well keeps the glyph
    // nearer the 12 – 14 px inline size so its strokes stay open.
    let size = if rect.height() < 28.0 {
        rect.height() * 0.7
    } else {
        rect.height() * 18.0 / 32.0
    };
    glyph(&painter, rect.center(), size, tokens.text);
    if response.has_focus() {
        // A well too small to leave 2 px clear (the strip's) keeps its ring
        // hugging the well, so the 24 px strip does not clip it.
        if rect.height() < 28.0 {
            focus_ring(&painter, rect.shrink(2.0), radius.saturating_sub(2), tokens);
        } else {
            focus_ring(&painter, rect, radius, tokens);
        }
    }
    response.on_hover_text(tooltip)
}

/// Draws a 32 px icon button in the layout.
pub fn icon_button(
    ui: &mut Ui,
    tokens: &Tokens,
    name: &str,
    tooltip: &str,
    glyph: Glyph<'_>,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
    icon_button_at(ui, rect, tokens, name, tooltip, glyph)
}
