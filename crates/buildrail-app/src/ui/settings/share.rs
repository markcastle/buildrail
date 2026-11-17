//! The Share page: export your pinned builds for a teammate, or import
//! theirs (read, map their connections to yours, review, replace).

use std::path::Path;

use buildrail_profile::AtomicFile;
use eframe::egui::{self, Ui};

use super::forms::{connect_rows, diff_summary, footer, sentence};
use super::{count, Input, SettingsWindow};
use crate::app::AppController;
use crate::settings::import::{choices_for, export_text, ImportFlow, ImportInput, ImportStep};
use crate::ui::theme::Tokens;
use crate::ui::widgets::{
    self, button, error_line, group, page_header, subhead, ButtonKind, TextField, Tone,
};

impl SettingsWindow {
    pub(super) fn share_page(
        &mut self,
        ui: &mut Ui,
        controller: &AppController,
        tokens: &Tokens,
        inputs: &mut Vec<Input>,
    ) {
        page_header(
            ui,
            tokens,
            "Share",
            "Swap pinned builds with a teammate. A profile names connections but never holds a token.",
        );

        subhead(ui, tokens, "Export");
        let text = export_text(controller.profile());
        group(ui, tokens, |g| {
            g.row(
                "Copy to the clipboard",
                Some("Paste it into a message to a teammate."),
                |ui| {
                    if button(ui, tokens, ButtonKind::Plain, "Copy profile", true, false).clicked()
                    {
                        ui.ctx().copy_text(text.clone());
                        self.share_message =
                            Some((Tone::Success, "Profile copied to the clipboard.".into()));
                    }
                },
            );
            g.row(
                "Save to a file",
                Some("Choose where to save it, for example builds.toml in your documents folder."),
                |ui| {
                    if button(ui, tokens, ButtonKind::Plain, "Save as…", true, false).clicked() {
                        if let Some(path) = controller.choose_export_file() {
                            self.export_path = path.display().to_string();
                            self.share_message = Some(save_profile(&path, &text));
                        }
                    }
                },
            );
            g.stack(
                "Or type where to save it",
                None,
                |ui| {
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 100.0).max(160.0);
                        TextField::new(&mut self.export_path, "Save to")
                            .placeholder("/home/you/Documents/builds.toml")
                            .width(width)
                            .show(ui, tokens);
                        let ready = !self.export_path.trim().is_empty();
                        if button(ui, tokens, ButtonKind::Plain, "Export", ready, false).clicked() {
                            let path = self.export_path.trim().to_owned();
                            self.share_message = Some(save_profile(Path::new(&path), &text));
                        }
                    });
                },
            );
        });
        if let Some((tone, message)) = &self.share_message {
            widgets::toast(ui, tokens, *tone, message);
        }

        subhead(ui, tokens, "Import");
        if let Some(error) = self.import.error() {
            error_line(ui, tokens, error);
        }
        let push = |inputs: &mut Vec<Input>, input| inputs.push(Input::Import(input));
        match self.import.step() {
            ImportStep::Input => {
                let mut text = self.import.text().to_owned();
                let mut load_error = None;
                group(ui, tokens, |g| {
                    g.row(
                        "From a file",
                        Some("A profile a teammate exported, such as builds.toml."),
                        |ui| {
                            if button(ui, tokens, ButtonKind::Plain, "Choose file…", true, false)
                                .clicked()
                            {
                                if let Some(path) = controller.choose_import_file() {
                                    self.import_path = path.display().to_string();
                                    match read_profile(&path) {
                                        Ok(text) => push(inputs, ImportInput::SetText(text)),
                                        Err(error) => load_error = Some(error),
                                    }
                                }
                            }
                        },
                    );
                    g.stack(
                        "Or type its path",
                        None,
                        |ui| {
                            ui.horizontal(|ui| {
                                let width = (ui.available_width() - 90.0).max(160.0);
                                TextField::new(&mut self.import_path, "Load file")
                                    .placeholder("/home/you/Downloads/builds.toml")
                                    .width(width)
                                    .show(ui, tokens);
                                let ready = !self.import_path.trim().is_empty();
                                if button(ui, tokens, ButtonKind::Plain, "Load", ready, false)
                                    .clicked()
                                {
                                    let path = self.import_path.trim().to_owned();
                                    match read_profile(Path::new(&path)) {
                                        Ok(text) => push(inputs, ImportInput::SetText(text)),
                                        Err(error) => load_error = Some(error),
                                    }
                                }
                            });
                        },
                    );
                    g.stack(
                        "Or paste it",
                        Some("A profile is a short text file that starts with “version = 1”."),
                        |ui| {
                            let edit = TextField::new(&mut text, "Profile to import")
                                .placeholder("version = 1\n\n[[watch]]\nlabel = \"Keevoro\"\n…")
                                .multiline(6)
                                .width(ui.available_width())
                                .show(ui, tokens);
                            if edit.changed() {
                                push(inputs, ImportInput::SetText(text.clone()));
                            }
                        },
                    );
                });
                if let Some(error) = load_error {
                    self.share_message = Some((Tone::Error, error));
                }
                let ready = !self.import.text().trim().is_empty();
                if footer(ui, tokens, false, Some(("Preview import", ready))).1 {
                    push(inputs, ImportInput::Load);
                }
            }
            ImportStep::Mapping => self.mapping_step(ui, controller, tokens, inputs),
            ImportStep::Review => {
                if let Some((_, diff)) = self.import.applied() {
                    diff_summary(ui, tokens, diff);
                }
                sentence(ui, tokens, "Your pinned builds will be replaced by these. Connections and tokens on this computer stay as they are.");
                let saving = self.import.is_saving();
                let (back, replace) =
                    footer(ui, tokens, true, Some(("Replace my builds", !saving)));
                if back {
                    push(inputs, ImportInput::Back);
                }
                if replace {
                    push(inputs, ImportInput::Confirm);
                }
            }
            ImportStep::Done => {
                let n = controller.profile().len();
                widgets::toast(
                    ui,
                    tokens,
                    Tone::Success,
                    &format!(
                        "Imported {}. They are on the strip now.",
                        count(n, "build", "builds")
                    ),
                );
                if button(ui, tokens, ButtonKind::Plain, "Import another", true, false).clicked() {
                    self.import = ImportFlow::new();
                }
            }
        }
    }

    fn mapping_step(
        &mut self,
        ui: &mut Ui,
        controller: &AppController,
        tokens: &Tokens,
        inputs: &mut Vec<Input>,
    ) {
        let push = |inputs: &mut Vec<Input>, input| inputs.push(Input::Import(input));
        if let Some(preview) = self.import.preview() {
            diff_summary(ui, tokens, preview.diff());
        }
        sentence(
            ui,
            tokens,
            "Choose which of your connections each imported connection uses.",
        );
        let ctx = super::context(controller);
        let required = self.import.required().to_vec();
        group(ui, tokens, |g| {
            for (index, required) in required.iter().enumerate() {
                let help = format!(
                    "{} · {} in the profile",
                    required.provider,
                    count(required.watch_count, "build", "builds")
                );
                let label = format!("“{}”", required.alias);
                g.row(&label, Some(&help), |ui| {
                    if button(ui, tokens, ButtonKind::Quiet, "New connection", true, false)
                        .clicked()
                    {
                        push(inputs, ImportInput::CreateConnection(index));
                    }
                    let current = self.import.mapped(required).cloned();
                    let shown = current
                        .as_ref()
                        .map_or("Choose…".to_owned(), |a| a.to_string());
                    egui::ComboBox::from_id_salt(("map", index))
                        .selected_text(shown)
                        .width(180.0)
                        .show_ui(ui, |ui| {
                            for (alias, who) in choices_for(required.provider, ctx) {
                                let selected = current.as_ref() == Some(&alias);
                                if ui
                                    .selectable_label(selected, format!("{alias} · {who}"))
                                    .clicked()
                                {
                                    push(
                                        inputs,
                                        ImportInput::Map {
                                            index,
                                            local: Some(alias),
                                        },
                                    );
                                }
                            }
                        });
                });
            }
            if let Some((_, flow)) = self.import.creating() {
                g.block(|ui| {
                    if button(ui, tokens, ButtonKind::Quiet, "Not now", true, false).clicked() {
                        push(inputs, ImportInput::CancelCreate);
                    }
                });
                if let Some(input) = connect_rows(g, flow, controller, tokens) {
                    push(inputs, ImportInput::Connect(input));
                }
            }
        });
        // While a connection is being made, its Connect is the one primary.
        let primary = self
            .import
            .creating()
            .is_none()
            .then_some(("Review changes", true));
        let (back, next) = footer(ui, tokens, true, primary);
        if back {
            push(inputs, ImportInput::Back);
        }
        if next {
            push(inputs, ImportInput::Continue);
        }
    }
}

/// Writes the exported profile to `path`; the message to show.
fn save_profile(path: &Path, text: &str) -> (Tone, String) {
    let shown = path.display();
    match AtomicFile::new(path).write(text.as_bytes()) {
        Ok(()) => (Tone::Success, format!("Saved the profile to {shown}.")),
        Err(error) => (
            Tone::Error,
            format!("The profile could not be saved to {shown}: {error}. Check the folder exists and you can write to it."),
        ),
    }
}

/// Reads a profile to import from `path`.
fn read_profile(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| {
        format!(
            "{} could not be read: {error}. Check the path and try again.",
            path.display()
        )
    })
}
