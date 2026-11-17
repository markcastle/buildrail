//! The About page: the version, the licence, where BuildRail keeps what
//! it knows, and the log folder.

use eframe::egui::Ui;

use super::SettingsWindow;
use crate::app::AppController;
use crate::ui::theme::{type_style, Tokens};
use crate::ui::widgets::{self, button, error_line, group, page_header, ButtonKind};

/// The version shown on the page.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

impl SettingsWindow {
    /// Draws the page.
    pub(super) fn about_page(&mut self, ui: &mut Ui, controller: &AppController, tokens: &Tokens) {
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
        let mut open_logs = false;
        group(ui, tokens, |g| {
            g.row("Version", None, |ui| value(ui, VERSION));
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
            if controller.logs_dir().is_some() {
                g.row(
                    "Log",
                    Some("What BuildRail did recently, kept for a few days. Useful when something goes wrong; it never holds a token."),
                    |ui| {
                        open_logs = button(ui, tokens, ButtonKind::Quiet, "Open log folder", true, false)
                            .clicked();
                    },
                );
            }
        });
        if open_logs {
            self.about_message = controller
                .open_log_folder()
                .err()
                .map(|error| format!("The log folder could not be opened: {error}."));
        }
        if let Some(message) = &self.about_message {
            error_line(ui, tokens, message);
        }
    }
}
