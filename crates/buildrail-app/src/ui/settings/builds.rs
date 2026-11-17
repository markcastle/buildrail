//! The Builds page: each pinned build as a card in the manner of Task
//! Rail's `ConnectorCard` (state dot, label, status; one line of facts;
//! quiet actions in order of use, Remove red and set apart), with what an
//! action opens directly beneath its card.

use buildrail_core::domain::ConnectionAlias;
use eframe::egui::{Align, Key, Layout, Ui};

use super::forms::{connect_rows, describe, sentence};
use super::{Input, SettingsWindow};
use crate::app::AppController;
use crate::settings::manage::ManageInput;
use crate::ui::theme::Tokens;
use crate::ui::widgets::{
    self, button, connector_card, group, page_header, ButtonKind, CardAction, ConnectorCard,
    TextField,
};

impl SettingsWindow {
    pub(super) fn builds_page(
        &mut self,
        ui: &mut Ui,
        controller: &AppController,
        tokens: &Tokens,
        inputs: &mut Vec<Input>,
    ) {
        let profile = controller.profile();
        ui.horizontal(|ui| {
            ui.set_width(ui.available_width().min(widgets::group::MAX_WIDTH));
            page_header(
                ui,
                tokens,
                "Builds",
                "What the strip shows, in this order. Changes apply as you make them.",
            );
        });
        if profile.is_empty() {
            group(ui, tokens, |g| {
                g.block(|ui| {
                    sentence(ui, tokens, "No builds are pinned yet.");
                    if button(ui, tokens, ButtonKind::Primary, "Add build", true, false).clicked() {
                        self.open_add_build();
                    }
                });
            });
            return;
        }
        if button(ui, tokens, ButtonKind::Primary, "Add build", true, false).clicked() {
            self.open_add_build();
        }
        if let Some(message) = self.manage.message() {
            widgets::toast(ui, tokens, widgets::Tone::Info, message);
        }
        let strip = controller.strip();
        let count = profile.len();
        group(ui, tokens, |g| {
            for (index, watch) in profile.watches().iter().enumerate() {
                let id = watch.id.clone();
                let entry = strip.get(&id);
                let tone = entry.map_or(buildrail_core::domain::Tone::Neutral, |e| e.tone);
                let ink = tokens.status(tone);
                let label = &watch.label;
                let card = ConnectorCard {
                    dot: ink.glyph,
                    name: label,
                    count: entry.map(|e| e.status_text.clone()),
                    count_color: Some(ink.word),
                    facts: vec![
                        describe(&watch.target),
                        format!("connection {}", watch.connection),
                    ],
                    error: None,
                };
                let actions = [
                    CardAction::new("Rename"),
                    CardAction::new("Move left")
                        .enabled(index > 0)
                        .tooltip(format!("Move {label} earlier on the strip")),
                    CardAction::new("Move right")
                        .enabled(index + 1 < count)
                        .tooltip(format!("Move {label} later on the strip")),
                    CardAction::new("Reconnect").tooltip(format!(
                        "Paste a new token for the “{}” connection",
                        watch.connection
                    )),
                    CardAction::danger("Remove"),
                ];
                g.block(|ui| {
                    let clicked = connector_card(ui, tokens, &card, &actions);
                    let input = match clicked {
                        Some(0) => Some(ManageInput::StartRename(id.clone())),
                        Some(1) => Some(ManageInput::MoveUp(id.clone())),
                        Some(2) => Some(ManageInput::MoveDown(id.clone())),
                        Some(3) => Some(ManageInput::StartReconnect(id.clone())),
                        Some(4) => Some(ManageInput::Remove(id.clone())),
                        _ => None,
                    };
                    inputs.extend(input.map(Input::Manage));
                });

                // What an action opens appears directly beneath its card.
                if let Some((renaming, text)) = self.manage.renaming() {
                    if *renaming == id {
                        if self.rename_buffer != text {
                            self.rename_buffer = text.to_owned();
                        }
                        let help = "Shown on the strip. Press Enter when you are done, or Esc to leave it as it was.";
                        g.row("Label", Some(help), |ui| {
                            let edit = TextField::new(&mut self.rename_buffer, "Label")
                                .width(220.0)
                                .show(ui, tokens);
                            if edit.changed() {
                                inputs.push(Input::Manage(ManageInput::SetRename(
                                    self.rename_buffer.clone(),
                                )));
                            }
                            if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                inputs.push(Input::Manage(ManageInput::CommitRename));
                            }
                            if ui.input(|i| i.key_pressed(Key::Escape)) {
                                inputs.push(Input::Manage(ManageInput::CancelRename));
                            }
                        });
                    }
                }
                if self.manage.confirm_remove() == Some(&id) {
                    g.block(|ui| {
                        ui.horizontal(|ui| {
                            sentence(
                                ui,
                                tokens,
                                &format!("Remove {label} from the strip? Its connection stays."),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if button(
                                    ui,
                                    tokens,
                                    ButtonKind::QuietDanger,
                                    "Yes, remove",
                                    true,
                                    false,
                                )
                                .clicked()
                                {
                                    inputs.push(Input::Manage(ManageInput::ConfirmRemove));
                                }
                                if button(ui, tokens, ButtonKind::Plain, "Keep", true, false)
                                    .clicked()
                                {
                                    inputs.push(Input::Manage(ManageInput::CancelRemove));
                                }
                            });
                        });
                    });
                }
                if let Some((reconnecting, flow)) = self.manage.reconnecting() {
                    if *reconnecting == id {
                        let provider = watch.target.provider();
                        let others: Vec<ConnectionAlias> = controller
                            .prefs()
                            .connections_for(provider)
                            .into_iter()
                            .filter(|a| **a != watch.connection)
                            .cloned()
                            .collect();
                        let mut cancel = false;
                        g.block(|ui| {
                            if !others.is_empty() {
                                ui.horizontal_wrapped(|ui| {
                                    sentence(ui, tokens, "Use another connection:");
                                    for alias in others {
                                        if button(
                                            ui,
                                            tokens,
                                            ButtonKind::Plain,
                                            alias.as_str(),
                                            true,
                                            false,
                                        )
                                        .clicked()
                                        {
                                            inputs.push(Input::Manage(ManageInput::UseConnection(
                                                id.clone(),
                                                alias,
                                            )));
                                        }
                                    }
                                });
                            }
                            ui.horizontal(|ui| {
                                sentence(
                                    ui,
                                    tokens,
                                    &format!(
                                        "Or paste a new token for the “{}” connection.",
                                        watch.connection
                                    ),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    cancel = button(
                                        ui,
                                        tokens,
                                        ButtonKind::Quiet,
                                        "Not now",
                                        true,
                                        false,
                                    )
                                    .clicked();
                                });
                            });
                        });
                        if cancel {
                            inputs.push(Input::Manage(ManageInput::CancelReconnect));
                        }
                        if let Some(input) = connect_rows(g, flow, controller, tokens) {
                            inputs.push(Input::Manage(ManageInput::Reconnect(input)));
                        }
                    }
                }
            }
        });
    }
}
