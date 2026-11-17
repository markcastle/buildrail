//! A short set of ways to do something, mirroring Task Rail's
//! `ChoiceCards`: two columns of `bg-raised` cards (radius 12, 14 px
//! padding, 10 px apart, `line` border) with a 14/600 title and a 12.5
//! muted sentence. The chosen card takes an `accent` border and a faint
//! ring; choosing one opens its form below while the cards stay.

use eframe::egui::{
    pos2, CornerRadius, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo,
};

use super::{focus_ring, wrapped};
use crate::ui::theme::{radius, type_style, Tokens};

/// One card.
#[derive(Debug, Clone, Copy)]
pub struct Choice<'a> {
    /// Two or three words; the card's accessible name.
    pub title: &'a str,
    /// One sentence that helps someone decide.
    pub text: &'a str,
}

const GAP: f32 = 10.0;
const PAD: f32 = 14.0;

/// Draws the cards; returns the index of the one clicked this frame.
pub fn choice_cards(
    ui: &mut Ui,
    tokens: &Tokens,
    choices: &[Choice<'_>],
    chosen: Option<usize>,
) -> Option<usize> {
    let total = ui.available_width().min(super::group::MAX_WIDTH);
    let columns = if total < 420.0 { 1 } else { 2 };
    let card_width = (total - GAP * (columns as f32 - 1.0)) / columns as f32;
    let inner = card_width - 2.0 * PAD;
    let mut clicked = None;
    for (row, pair) in choices.chunks(columns).enumerate() {
        let laid: Vec<_> = pair
            .iter()
            .map(|c| {
                let title = wrapped(ui, type_style::LABEL, c.title, tokens.text, inner);
                let text = wrapped(ui, type_style::HELP, c.text, tokens.text_muted, inner);
                (title, text)
            })
            .collect();
        let height = laid
            .iter()
            .map(|(t, x)| t.size().y + 6.0 + x.size().y)
            .fold(0.0, f32::max)
            + 2.0 * PAD;
        let (band, _) = ui.allocate_exact_size(Vec2::new(total, height), Sense::hover());
        for (col, (choice, (title, text))) in pair.iter().zip(laid).enumerate() {
            let index = row * columns + col;
            let min = pos2(band.min.x + col as f32 * (card_width + GAP), band.min.y);
            let rect = Rect::from_min_size(min, Vec2::new(card_width, height));
            let response: Response =
                ui.interact(rect, ui.scope_id().with(("choice", index)), Sense::click());
            let on = chosen == Some(index);
            response
                .widget_info(|| WidgetInfo::selected(Role::RadioButton, true, on, choice.title));
            let r12 = CornerRadius::same(radius::R12);
            let painter = ui.painter();
            let fill = if response.hovered() {
                tokens.bg_hover
            } else {
                tokens.bg_raised
            };
            if on {
                painter.rect_stroke(
                    rect,
                    r12,
                    Stroke::new(2.0, tokens.chosen_ring()),
                    StrokeKind::Outside,
                );
            }
            painter.rect_filled(rect, r12, fill);
            let border = if on { tokens.accent } else { tokens.line };
            painter.rect_stroke(rect, r12, Stroke::new(1.0, border), StrokeKind::Inside);
            let title_height = title.size().y;
            painter.galley(rect.min + Vec2::splat(PAD), title, tokens.text);
            painter.galley(
                rect.min + Vec2::new(PAD, PAD + title_height + 6.0),
                text,
                tokens.text_muted,
            );
            if response.has_focus() {
                focus_ring(painter, rect, radius::R12, tokens);
            }
            if response.clicked() {
                clicked = Some(index);
            }
        }
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
    clicked
}
