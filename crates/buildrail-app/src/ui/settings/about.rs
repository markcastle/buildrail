//! The About page: the version, the licence, and where BuildRail keeps
//! what it knows.

use eframe::egui::Ui;

use crate::ui::theme::{type_style, Tokens};
use crate::ui::widgets::{self, group, page_header};

/// Draws the page.
pub(super) fn page(ui: &mut Ui, tokens: &Tokens) {
    page_header(
        ui,
        tokens,
        "About",
        "BuildRail keeps your builds in view on a thin strip at the edge of the screen.",
    );
    let value = |ui: &mut Ui, text: &str| {
        let galley = widgets::text(ui, type_style::BODY, text, tokens.text_muted);
        ui.add(eframe::egui::Label::new(eframe::egui::WidgetText::Galley(
            galley,
        )));
    };
    group(ui, tokens, |g| {
        g.row("Version", None, |ui| value(ui, env!("CARGO_PKG_VERSION")));
        g.row("Licence", None, |ui| value(ui, "MIT"));
        g.row(
            "Your data",
            Some("Pinned builds and preferences are files on this computer. Tokens are kept in the operating system's credential store, never in a file or an export."),
            |_| {},
        );
        g.row(
            "Keyboard",
            Some("On the strip, Tab moves between builds, and Enter or Space opens the latest run. Hover or focus shows the details."),
            |_| {},
        );
    });
}
