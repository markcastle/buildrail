//! One of two to four options, mirroring Task Rail's `Segmented`: a
//! `bg-deep` track (radius 10, 3 px inset) whose chosen option is a raised
//! `bg-raised` chip (radius 8) in `text` at 600; the others are
//! `text-muted` at 500. A `radiogroup` of `radio` buttons to assistive
//! technology.

use eframe::egui::{
    self, CornerRadius, Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo,
};

use super::{focus_ring, text};
use crate::ui::theme::{radius, type_style, Tokens, TypeStyle, Weight};

const INSET: f32 = 3.0;
const PAD: Vec2 = Vec2::new(14.0, 6.0);

/// Draws the options and returns the group's response, marked changed
/// when the person picked another option (and `value` was updated).
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    tokens: &Tokens,
    name: &str,
    value: &mut T,
    options: &[(T, &str)],
) -> Response {
    let on_style = TypeStyle {
        weight: Weight::Semibold,
        ..type_style::BUTTON
    };
    let off_style = type_style::BUTTON;
    // Measure in the heavier weight so the chip does not change width.
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, label)| text(ui, on_style, label, tokens.text).size().x + 2.0 * PAD.x)
        .collect();
    let height = text(ui, on_style, "Ag", tokens.text).size().y + 2.0 * PAD.y;
    let size = Vec2::new(
        widths.iter().sum::<f32>() + 2.0 * INSET,
        height + 2.0 * INSET,
    );
    let (track, mut group) = ui.allocate_exact_size(size, Sense::hover());
    group.widget_info(|| WidgetInfo::labeled(Role::RadioGroup, ui.is_enabled(), name));
    ui.painter()
        .rect_filled(track, CornerRadius::same(radius::R10), tokens.bg_deep);
    ui.painter().rect_stroke(
        track,
        CornerRadius::same(radius::R10),
        Stroke::new(1.0, tokens.line),
        StrokeKind::Inside,
    );

    let mut x = track.min.x + INSET;
    for (index, ((option, label), width)) in options.iter().zip(&widths).enumerate() {
        let rect = egui::Rect::from_min_size(
            egui::pos2(x, track.min.y + INSET),
            Vec2::new(*width, height),
        );
        x += width;
        let id = group.id.with(index);
        let response = ui.interact(rect, id, Sense::click());
        let chosen = *value == *option;
        response.widget_info(|| {
            WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), chosen, *label)
        });
        if response.clicked() && !chosen {
            *value = *option;
            group.mark_changed();
        }
        let painter = ui.painter();
        let r8 = CornerRadius::same(radius::R8);
        if chosen {
            let lift = tokens.card_shadow[0];
            painter.rect_filled(rect.translate(Vec2::new(0.0, 1.0)), r8, lift.color);
            painter.rect_filled(rect, r8, tokens.bg_raised);
        } else if response.hovered() {
            painter.rect_filled(rect, r8, tokens.bg_hover);
        }
        let (style, color) = if chosen {
            (on_style, tokens.text)
        } else {
            (off_style, tokens.text_muted)
        };
        let galley = text(ui, style, label, color);
        let at = rect.center() - galley.size() / 2.0;
        painter.galley(at, galley, color);
        if response.has_focus() {
            focus_ring(painter, rect, radius::R8, tokens);
        }
    }
    group
}
