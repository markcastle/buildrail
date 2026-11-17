//! A switch, mirroring Task Rail's `Toggle`: a 44 × 26 pill whose 20 px
//! knob slides over in 160 ms, `line` when off and `done` green when on.
//! It is a `switch` to assistive technology, named after its setting, and
//! Space or Enter flips it.

use eframe::egui::{Response, Role, Sense, Ui, Vec2, WidgetInfo};

use super::focus_ring;
use crate::ui::theme::Tokens;

const SIZE: Vec2 = Vec2::new(44.0, 26.0);
const KNOB: f32 = 20.0;
const INSET: f32 = 3.0;

/// Draws a switch for `on`, named `name`. Flips `on` when clicked or
/// activated from the keyboard; the response is marked changed then.
pub fn toggle(ui: &mut Ui, tokens: &Tokens, on: &mut bool, name: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(SIZE, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let state = *on;
    response.widget_info(|| WidgetInfo::selected(Role::Switch, ui.is_enabled(), state, name));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_with_time(response.id, state, 0.16);
        let painter = ui.painter();
        let track = if state { tokens.done } else { tokens.line };
        painter.rect_filled(rect, rect.height() / 2.0, track);
        let x = rect.min.x + INSET + KNOB / 2.0 + t * (SIZE.x - KNOB - 2.0 * INSET);
        let center = eframe::egui::pos2(x, rect.center().y);
        let shadow = tokens.card_shadow[0];
        painter.circle_filled(center + Vec2::new(0.0, 1.0), KNOB / 2.0 + 0.5, shadow.color);
        // The knob is white in both themes, as the system draws it.
        painter.circle_filled(center, KNOB / 2.0, tokens.accent_text);
        if response.has_focus() {
            focus_ring(painter, rect, (SIZE.y / 2.0) as u8, tokens);
        }
    }
    response
}
