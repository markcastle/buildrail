//! The Connections page: one card per account BuildRail reads from, in
//! the manner of Task Rail's `ConnectorCard`.

use eframe::egui::Ui;

use super::forms::sentence;
use super::{count, Input, SettingsWindow};
use crate::app::AppController;
use crate::settings::manage::ManageInput;
use crate::ui::theme::Tokens;
use crate::ui::widgets::{self, connector_card, group, page_header, CardAction, ConnectorCard};

impl SettingsWindow {
    pub(super) fn connections_page(
        &mut self,
        ui: &mut Ui,
        controller: &AppController,
        tokens: &Tokens,
        inputs: &mut Vec<Input>,
    ) {
        page_header(
            ui,
            tokens,
            "Connections",
            "The accounts BuildRail reads your builds from. Their tokens stay in your operating system's credential store.",
        );
        if let Some(message) = self.manage.message() {
            widgets::toast(ui, tokens, widgets::Tone::Info, message);
        }
        let prefs = controller.prefs();
        group(ui, tokens, |g| {
            if prefs.connections.is_empty() {
                g.block(|ui| {
                    sentence(
                        ui,
                        tokens,
                        "No connections yet. Add a build and you connect an account on the way.",
                    );
                });
            }
            for (alias, connection) in &prefs.connections {
                let used = crate::settings::watches_using(controller.profile(), alias).len();
                let card = ConnectorCard {
                    dot: if used > 0 {
                        tokens.done
                    } else {
                        tokens.accent_slate
                    },
                    name: alias.as_str(),
                    count: Some(if used > 0 {
                        format!("Used by {}", count(used, "build", "builds"))
                    } else {
                        "Not used".to_owned()
                    }),
                    count_color: None,
                    facts: vec![
                        connection.provider.to_string(),
                        format!("signed in as {}", connection.display_name),
                        "token in the credential store".to_owned(),
                    ],
                    error: None,
                };
                let actions = [CardAction::danger("Remove")
                    .tooltip(format!("Remove the “{alias}” connection and its token"))];
                g.block(|ui| {
                    if connector_card(ui, tokens, &card, &actions).is_some() {
                        inputs.push(Input::Manage(ManageInput::RemoveConnection(alias.clone())));
                    }
                });
            }
        });
    }
}
