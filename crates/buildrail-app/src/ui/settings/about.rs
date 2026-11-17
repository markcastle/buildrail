//! The About page: the version and updates, the licence, where BuildRail
//! keeps what it knows, and the log folder.
//!
//! Updates follow the release playbook: "Check for updates" answers in
//! one line under it; an available update gets **Install now** and a
//! **What's new** link to its release page (the app does not explain the
//! release itself); a build that cannot update itself says so and links
//! to the releases page; and "Update automatically" is off unless the
//! person turns it on.

use eframe::egui::Ui;

use super::SettingsWindow;
use crate::app::AppController;
use crate::presentation::update::RELEASES_PAGE;
use crate::presentation::UpdateAnswer;
use crate::ui::theme::{type_style, Tokens};
use crate::ui::widgets::{self, button, error_line, group, page_header, toggle, ButtonKind};

/// The version shown on the page.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the person asked for on the page this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AboutAction {
    OpenLogs,
    Check,
    Install,
    Open(String),
    SetAutomatic(bool),
}

impl SettingsWindow {
    /// Draws the page.
    pub(super) fn about_page(
        &mut self,
        ui: &mut Ui,
        controller: &mut AppController,
        tokens: &Tokens,
    ) {
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
        let answer = controller.update_answer();
        let mut automatic = controller.prefs().update_automatically;
        let mut action = None;
        let version = format!("Version {VERSION}");
        group(ui, tokens, |g| {
            match &answer {
                UpdateAnswer::NotConfigured => {
                    g.row(&version, answer.line().as_deref(), |ui| {
                        if button(
                            ui,
                            tokens,
                            ButtonKind::Quiet,
                            "Open releases page",
                            true,
                            false,
                        )
                        .clicked()
                        {
                            action = Some(AboutAction::Open(RELEASES_PAGE.to_owned()));
                        }
                    });
                }
                UpdateAnswer::Available(offer) => {
                    g.row(&version, answer.line().as_deref(), |ui| {
                        // Right to left: the primary action sits at the end.
                        let label = if offer.installing {
                            "Installing…"
                        } else {
                            "Install now"
                        };
                        if button(
                            ui,
                            tokens,
                            ButtonKind::Primary,
                            label,
                            !offer.installing,
                            false,
                        )
                        .clicked()
                        {
                            action = Some(AboutAction::Install);
                        }
                        if button(ui, tokens, ButtonKind::Quiet, "What's new", true, false)
                            .clicked()
                        {
                            action = Some(AboutAction::Open(offer.notes_url.clone()));
                        }
                    });
                }
                _ => {
                    let help = answer.line().unwrap_or_else(|| {
                        "BuildRail looks for a new release once an hour.".to_owned()
                    });
                    let checking = answer == UpdateAnswer::Checking;
                    g.row(&version, Some(&help), |ui| {
                        if button(
                            ui,
                            tokens,
                            ButtonKind::Quiet,
                            "Check for updates",
                            !checking,
                            false,
                        )
                        .clicked()
                        {
                            action = Some(AboutAction::Check);
                        }
                    });
                }
            }
            g.row(
                "Update automatically",
                Some("New releases install at a quiet moment, when no build is running. Off, and the strip tells you one is ready instead."),
                |ui| {
                    if toggle(ui, tokens, &mut automatic, "Update automatically").changed() {
                        action = Some(AboutAction::SetAutomatic(automatic));
                    }
                },
            );
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
                        if button(ui, tokens, ButtonKind::Quiet, "Open log folder", true, false)
                            .clicked()
                        {
                            action = Some(AboutAction::OpenLogs);
                        }
                    },
                );
            }
        });
        match action {
            None => {}
            Some(AboutAction::OpenLogs) => {
                self.about_message = controller
                    .open_log_folder()
                    .err()
                    .map(|error| format!("The log folder could not be opened: {error}."));
            }
            Some(AboutAction::Check) => {
                self.about_message = None;
                controller.check_for_updates();
            }
            Some(AboutAction::Install) => {
                self.about_message = None;
                controller.install_update();
            }
            Some(AboutAction::Open(url)) => {
                self.about_message = controller
                    .open_url(&url)
                    .err()
                    .map(|error| format!("The page could not be opened: {error}."));
            }
            Some(AboutAction::SetAutomatic(on)) => {
                self.about_message = controller
                    .update_prefs(|prefs| prefs.update_automatically = on)
                    .err()
                    .map(|error| {
                        format!("BuildRail could not save this change: {error}. Try again.")
                    });
            }
        }
        if let UpdateAnswer::Available(offer) = &answer {
            if let Some(error) = &offer.error {
                error_line(ui, tokens, error);
            }
        }
        if let Some(message) = &self.about_message {
            error_line(ui, tokens, message);
        }
    }
}
