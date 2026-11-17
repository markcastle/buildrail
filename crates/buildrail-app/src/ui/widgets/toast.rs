//! Confirmations, mirroring Task Rail's `Toast` and the group's `saved`
//! mark.
//!
//! [`toast`] is the inline toast (`toast--inline`): one sentence on an
//! inverted ground (`text` behind, `bg` ink, radius 10, 10 × 14 padding).
//! Task Rail fills success and error toasts with `done` and `priority`
//! behind white words, which is under 4.5 : 1 in places; BuildRail keeps
//! the inverted ground for every tone and lets a coloured glyph carry the
//! tone instead, so the words always pass.
//!
//! [`saved_mark`] is the quiet "Saved" that confirms a setting landed.

use eframe::egui::{CornerRadius, Frame, Label, Margin, Ui, Vec2, WidgetText};

use super::wrapped;
use crate::presentation::IconKind;
use crate::ui::glyphs::{paint_icon, paint_tick};
use crate::ui::theme::{radius, type_style, Tokens};

/// What a toast reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Something happened.
    Info,
    /// Something succeeded.
    Success,
    /// Something went wrong.
    Error,
}

/// Draws an inline toast with one sentence.
pub fn toast(ui: &mut Ui, tokens: &Tokens, tone: Tone, sentence: &str) {
    let width = ui.available_width().min(super::group::MAX_WIDTH);
    Frame::new()
        .fill(tokens.text)
        .corner_radius(CornerRadius::same(radius::R10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_max_width(width - 28.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 0.0);
                let (glyph, _) =
                    ui.allocate_exact_size(Vec2::splat(16.0), eframe::egui::Sense::hover());
                let painter = ui.painter();
                // The ground is inverted, so the tone's colour comes from
                // the other theme (light ground in dark, dark in light).
                let ink = if tokens.dark {
                    crate::ui::theme::LIGHT
                } else {
                    crate::ui::theme::DARK
                };
                match tone {
                    Tone::Info => {
                        painter.circle_filled(glyph.center(), 3.5, tokens.bg);
                    }
                    Tone::Success => paint_tick(painter, glyph.center(), 15.0, ink.done),
                    Tone::Error => paint_icon(
                        painter,
                        glyph.center(),
                        15.0,
                        IconKind::Cross,
                        ink.priority,
                        tokens.text,
                        0.0,
                    ),
                }
                let room = ui.available_width();
                let galley = wrapped(ui, type_style::BODY, sentence, tokens.bg, room);
                ui.add(Label::new(WidgetText::Galley(galley)));
            });
        });
}

/// The "Saved" mark: a `done` tick and the word in `text-muted`, shown
/// while `visible`.
pub fn saved_mark(ui: &mut Ui, tokens: &Tokens, visible: bool) {
    if !visible {
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
        let (glyph, _) = ui.allocate_exact_size(Vec2::splat(13.0), eframe::egui::Sense::hover());
        paint_tick(ui.painter(), glyph.center(), 13.0, tokens.done);
        let galley = wrapped(ui, type_style::HELP, "Saved", tokens.text_muted, 100.0);
        ui.add(Label::new(WidgetText::Galley(galley)).selectable(false));
    });
}

/// An error said in place, under what it is about: a red cross glyph and
/// the sentence in the error text colour (`priority`, or `chip-urgent-fg`
/// where `priority` is under 4.5 : 1).
pub fn error_line(ui: &mut Ui, tokens: &Tokens, sentence: &str) {
    let width = ui.available_width().min(super::group::MAX_WIDTH);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
        let (glyph, _) =
            ui.allocate_exact_size(Vec2::new(15.0, 19.0), eframe::egui::Sense::hover());
        paint_icon(
            ui.painter(),
            glyph.center(),
            15.0,
            IconKind::Cross,
            tokens.priority,
            tokens.bg,
            0.0,
        );
        let galley = wrapped(
            ui,
            type_style::BODY,
            sentence,
            tokens.error_text(),
            width - 23.0,
        );
        ui.add(Label::new(WidgetText::Galley(galley)));
    });
}
