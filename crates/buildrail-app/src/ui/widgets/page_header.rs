//! The title of a Settings page and one sentence under it, mirroring Task
//! Rail's `PageHeader` (24/700 tightened, then a 14 px `text-muted`
//! sentence 4 px below), and the `subhead` between groups (15/650).

use eframe::egui::{Label, Ui, Vec2, WidgetText};

use super::wrapped;
use crate::ui::theme::{space, type_style, Tokens};

/// Draws a page's title and its one sentence.
pub fn page_header(ui: &mut Ui, tokens: &Tokens, title: &str, sentence: &str) {
    let width = ui.available_width();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(0.0, space::S4);
        let title = wrapped(ui, type_style::PAGE_TITLE, title, tokens.text, width);
        ui.add(Label::new(WidgetText::Galley(title)).selectable(false));
        let sentence = wrapped(
            ui,
            type_style::BODY,
            sentence,
            tokens.text_muted,
            width.min(560.0),
        );
        ui.add(Label::new(WidgetText::Galley(sentence)).selectable(false));
    });
}

/// A heading between groups on a page.
pub fn subhead(ui: &mut Ui, tokens: &Tokens, text: &str) {
    let width = ui.available_width();
    let galley = wrapped(ui, type_style::SUBHEAD, text, tokens.text, width);
    ui.add(Label::new(WidgetText::Galley(galley)).selectable(false));
}
