//! The Settings window: pinned builds, the Add-build wizard, connections,
//! import/export and this device's preferences.
//!
//! This file only *draws* the settings state machines and turns clicks into
//! their inputs; all decisions are in [`crate::settings`]. Each frame:
//!
//! 1. drawing collects inputs (it never mutates a machine mid-draw),
//! 2. inputs are applied to the machines, which return [`Command`]s,
//! 3. commands go to the controller; their results come back through
//!    [`SettingsWindow::handle_events`].

use buildrail_core::domain::{BuildRecord, ConnectionAlias, ProviderKind, WatchId, WatchTarget};
use buildrail_profile::{
    AtomicFile, DevicePrefs, DiffKind, MonitorChoice, Placement, Theme, WatchChange,
};
use eframe::egui::{self, RichText, Ui};

use crate::app::AppController;
use crate::presentation::{age_short, short_commit};
use crate::settings::connect::{guidance, ConnectFlow, ConnectInput};
use crate::settings::import::{choices_for, export_text, ImportFlow, ImportInput, ImportStep};
use crate::settings::manage::{ManageInput, ManageList};
use crate::settings::wizard::{AddBuildWizard, Step, WizardInput};
use crate::settings::{Command, Event, Loadable, SettingsContext};

/// Settings tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Pinned builds: rename, reorder, remove, reconnect.
    Builds,
    /// The Add-build wizard.
    AddBuild,
    /// Connections on this device.
    Connections,
    /// Import and export.
    Share,
    /// Theme, placement, monitor, scale, startup.
    Device,
}

/// Facts about the window the Device tab shows.
#[derive(Debug, Clone, Default)]
pub struct DeviceInfo {
    /// Connected monitors: (id, description).
    pub monitors: Vec<(String, String)>,
    /// How the strip is placed right now, in words.
    pub placement_note: Option<String>,
}

/// What the window asks of its host after a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SettingsOutcome {
    /// Placement, monitor or scale changed: re-place the strip.
    pub placement_changed: bool,
}

/// State of the Settings window. See the module docs.
#[derive(Debug)]
pub struct SettingsWindow {
    /// The selected tab.
    pub tab: Tab,
    wizard: AddBuildWizard,
    manage: ManageList,
    import: ImportFlow,
    filter: String,
    rename_buffer: String,
    import_path: String,
    export_path: String,
    share_message: Option<String>,
    device_message: Option<String>,
}

impl Default for SettingsWindow {
    fn default() -> Self {
        Self::new()
    }
}

enum Input {
    Wizard(WizardInput),
    Manage(ManageInput),
    Import(ImportInput),
}

impl SettingsWindow {
    /// A window on the Builds tab.
    pub fn new() -> Self {
        Self {
            tab: Tab::Builds,
            wizard: AddBuildWizard::new(),
            manage: ManageList::new(),
            import: ImportFlow::new(),
            filter: String::new(),
            rename_buffer: String::new(),
            import_path: String::new(),
            export_path: String::new(),
            share_message: None,
            device_message: None,
        }
    }

    /// The wizard (for tests and the host).
    pub fn wizard(&self) -> &AddBuildWizard {
        &self.wizard
    }

    /// The import flow (for tests and the host).
    pub fn import(&self) -> &ImportFlow {
        &self.import
    }

    /// Shows the Add-build wizard, starting over if the last one finished.
    pub fn open_add_build(&mut self) {
        if self.wizard.step() == Step::Pinned {
            self.wizard = AddBuildWizard::new();
        }
        self.tab = Tab::AddBuild;
    }

    /// Shows the Builds tab with the reconnect form open for `watch`.
    pub fn open_reconnect(&mut self, watch: &WatchId, controller: &AppController) {
        self.tab = Tab::Builds;
        let ctx = context(controller);
        let _ = self
            .manage
            .input(ManageInput::StartReconnect(watch.clone()), ctx);
    }

    /// Routes controller events to every machine; returns follow-up
    /// commands (e.g. the wizard loading repositories after a connect).
    pub fn handle_events(&mut self, events: &[Event]) -> Vec<Command> {
        let mut commands = Vec::new();
        for event in events {
            commands.extend(self.wizard.on_event(event));
            self.manage.on_event(event);
            self.import.on_event(event);
        }
        commands
    }

    /// Draws the window and executes resulting commands.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        controller: &mut AppController,
        device: &DeviceInfo,
    ) -> SettingsOutcome {
        let mut inputs = Vec::new();
        let mut outcome = SettingsOutcome::default();
        if let Some(error) = controller.profile_error() {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Your profile file could not be read and was left untouched: {error}"),
            );
        }
        ui.horizontal(|ui| {
            for (tab, name) in [
                (Tab::Builds, "Builds"),
                (Tab::AddBuild, "Add build"),
                (Tab::Connections, "Connections"),
                (Tab::Share, "Import / export"),
                (Tab::Device, "This device"),
            ] {
                if ui.selectable_label(self.tab == tab, name).clicked() {
                    if tab == Tab::AddBuild {
                        self.open_add_build();
                    } else {
                        self.tab = tab;
                    }
                }
            }
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.tab {
                Tab::Builds => self.builds_tab(ui, controller, &mut inputs),
                Tab::AddBuild => self.wizard_tab(ui, controller, &mut inputs),
                Tab::Connections => self.connections_tab(ui, controller, &mut inputs),
                Tab::Share => self.share_tab(ui, controller, &mut inputs),
                Tab::Device => outcome = self.device_tab(ui, controller, device),
            });

        let mut commands = Vec::new();
        {
            let ctx = context(controller);
            for input in inputs {
                commands.extend(match input {
                    Input::Wizard(input) => self.wizard.input(input, ctx),
                    Input::Manage(input) => self.manage.input(input, ctx),
                    Input::Import(input) => self.import.input(input, ctx),
                });
            }
        }
        for command in commands {
            controller.execute(command);
        }
        outcome
    }

    // --------------------------------------------------------------- builds

    fn builds_tab(&mut self, ui: &mut Ui, controller: &AppController, inputs: &mut Vec<Input>) {
        let profile = controller.profile();
        if profile.is_empty() {
            ui.label("No builds are pinned yet.");
            if ui.button("Add build").clicked() {
                self.open_add_build();
            }
            return;
        }
        if let Some(message) = self.manage.message() {
            ui.label(message);
        }
        let strip = controller.strip();
        let count = profile.len();
        for (index, watch) in profile.watches().iter().enumerate() {
            let id = watch.id.clone();
            ui.group(|ui| {
                let status = strip
                    .get(&id)
                    .map(|e| e.status_text.clone())
                    .unwrap_or_default();
                match self.manage.renaming() {
                    Some((renaming, text)) if *renaming == id => {
                        if self.rename_buffer != text {
                            self.rename_buffer = text.to_owned();
                        }
                        ui.horizontal(|ui| {
                            let label = ui.label("New label");
                            let edit = ui
                                .text_edit_singleline(&mut self.rename_buffer)
                                .labelled_by(label.id);
                            if edit.changed() {
                                inputs.push(Input::Manage(ManageInput::SetRename(
                                    self.rename_buffer.clone(),
                                )));
                            }
                            if ui.button("Save").clicked() {
                                inputs.push(Input::Manage(ManageInput::CommitRename));
                            }
                            if ui.button("Cancel").clicked() {
                                inputs.push(Input::Manage(ManageInput::CancelRename));
                            }
                        });
                    }
                    _ => {
                        ui.horizontal(|ui| {
                            ui.strong(&watch.label);
                            ui.weak(format!("{status} · {}", describe(&watch.target)));
                        });
                    }
                }
                ui.horizontal(|ui| {
                    ui.weak(format!("Connection: {}", watch.connection));
                    let label = &watch.label;
                    let up = ui
                        .add_enabled(index > 0, egui::Button::new("Move left"))
                        .on_hover_text(format!("Move {label} earlier on the strip"));
                    if up.clicked() {
                        inputs.push(Input::Manage(ManageInput::MoveUp(id.clone())));
                    }
                    let down = ui.add_enabled(index + 1 < count, egui::Button::new("Move right"));
                    if down.clicked() {
                        inputs.push(Input::Manage(ManageInput::MoveDown(id.clone())));
                    }
                    if ui.button("Rename").clicked() {
                        inputs.push(Input::Manage(ManageInput::StartRename(id.clone())));
                    }
                    if ui.button("Reconnect").clicked() {
                        inputs.push(Input::Manage(ManageInput::StartReconnect(id.clone())));
                    }
                    if ui.button("Remove").clicked() {
                        inputs.push(Input::Manage(ManageInput::Remove(id.clone())));
                    }
                });
                if self.manage.confirm_remove() == Some(&id) {
                    ui.horizontal(|ui| {
                        ui.label(format!("Remove {} from the strip?", watch.label));
                        if ui.button("Yes, remove").clicked() {
                            inputs.push(Input::Manage(ManageInput::ConfirmRemove));
                        }
                        if ui.button("Keep").clicked() {
                            inputs.push(Input::Manage(ManageInput::CancelRemove));
                        }
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
                        if !others.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                ui.label("Use another connection:");
                                for alias in others {
                                    if ui.button(alias.as_str()).clicked() {
                                        inputs.push(Input::Manage(ManageInput::UseConnection(
                                            id.clone(),
                                            alias,
                                        )));
                                    }
                                }
                            });
                        }
                        ui.label(format!(
                            "Or paste a new token for \"{}\":",
                            watch.connection
                        ));
                        if let Some(input) = connect_form(ui, flow, controller) {
                            inputs.push(Input::Manage(ManageInput::Reconnect(input)));
                        }
                        if ui.button("Close").clicked() {
                            inputs.push(Input::Manage(ManageInput::CancelReconnect));
                        }
                    }
                }
            });
        }
    }

    // --------------------------------------------------------------- wizard

    fn wizard_tab(&mut self, ui: &mut Ui, controller: &AppController, inputs: &mut Vec<Input>) {
        let wizard = &self.wizard;
        let push = |inputs: &mut Vec<Input>, input| inputs.push(Input::Wizard(input));
        ui.heading("Add build");
        let summary = wizard.summary();
        if !summary.is_empty() {
            ui.weak(summary);
        }
        match wizard.step() {
            Step::ChooseProvider => {
                ui.label("What do you want to watch?");
                if ui.button("GitHub Actions workflow").clicked() {
                    push(inputs, WizardInput::ChooseProvider(ProviderKind::GitHub));
                }
                if ui.button("Cloudflare Workers build").clicked() {
                    push(
                        inputs,
                        WizardInput::ChooseProvider(ProviderKind::Cloudflare),
                    );
                }
            }
            Step::Connect => match wizard.connect_flow() {
                Some(flow) => {
                    if let Some(input) = connect_form(ui, flow, controller) {
                        push(inputs, WizardInput::Connect(input));
                    }
                }
                None => {
                    ui.label("Use a connection on this device:");
                    if let Some(provider) = wizard.provider() {
                        for alias in controller.prefs().connections_for(provider) {
                            let who = controller
                                .prefs()
                                .connection(alias)
                                .map(|c| c.display_name.clone())
                                .unwrap_or_default();
                            if ui.button(format!("{alias} ({who})")).clicked() {
                                push(inputs, WizardInput::UseConnection(alias.clone()));
                            }
                        }
                    }
                    if ui.button("Connect another account").clicked() {
                        push(inputs, WizardInput::NewConnection);
                    }
                }
            },
            Step::Repository => {
                let filter = self.filter.to_lowercase();
                if let Some(repos) = loadable(ui, wizard.repositories(), "repositories", inputs) {
                    filter_field(ui, &mut self.filter);
                    for repo in repos
                        .iter()
                        .filter(|r| r.full_name.to_lowercase().contains(&filter))
                    {
                        let mut text = repo.full_name.clone();
                        if repo.archived {
                            text.push_str(" (archived)");
                        }
                        if ui.button(text).clicked() {
                            push(inputs, WizardInput::SelectRepository(repo.clone()));
                        }
                    }
                    if repos.is_empty() {
                        ui.label("This token cannot see any repositories. Give it access to the repositories you want to watch.");
                    }
                }
            }
            Step::Workflow => {
                if let Some(workflows) = loadable(ui, wizard.workflows(), "workflows", inputs) {
                    for workflow in workflows {
                        let text = format!("{} ({})", workflow.name, workflow.watch_key());
                        if ui.button(text).clicked() {
                            push(inputs, WizardInput::SelectWorkflow(workflow.clone()));
                        }
                    }
                    if workflows.is_empty() {
                        ui.label("This repository has no workflows.");
                    }
                }
            }
            Step::Branch => {
                let current = wizard.branch().map(str::to_owned);
                match wizard.provider() {
                    Some(ProviderKind::GitHub) => {
                        if ui.radio(current.is_none(), "Any branch").clicked() {
                            push(inputs, WizardInput::SetBranch(None));
                        }
                        if let Some(branches) = loadable(ui, wizard.branches(), "branches", inputs)
                        {
                            for branch in branches {
                                let selected = current.as_deref() == Some(branch.name.as_str());
                                if ui.radio(selected, &branch.name).clicked() {
                                    push(inputs, WizardInput::SetBranch(Some(branch.name.clone())));
                                }
                            }
                        }
                    }
                    _ => {
                        let mut text = current.unwrap_or_default();
                        let label = ui.label("Branch (leave empty for every branch)");
                        if ui
                            .text_edit_singleline(&mut text)
                            .labelled_by(label.id)
                            .changed()
                        {
                            push(inputs, WizardInput::SetBranch(Some(text)));
                        }
                    }
                }
                if ui.button("Preview").clicked() {
                    push(inputs, WizardInput::ConfirmBranch);
                }
            }
            Step::Account => {
                if let Some(accounts) = loadable(ui, wizard.accounts(), "accounts", inputs) {
                    for account in accounts {
                        if ui.button(&account.name).clicked() {
                            push(inputs, WizardInput::SelectAccount(account.clone()));
                        }
                    }
                }
            }
            Step::Worker => {
                let filter = self.filter.to_lowercase();
                if let Some(workers) = loadable(ui, wizard.workers(), "Workers", inputs) {
                    filter_field(ui, &mut self.filter);
                    for worker in workers.iter().filter(|w| w.name.contains(&filter)) {
                        if ui.button(&worker.name).clicked() {
                            push(inputs, WizardInput::SelectWorker(worker.clone()));
                        }
                    }
                    if workers.is_empty() {
                        ui.label("This account has no Workers.");
                    }
                }
            }
            Step::Preview => match wizard.preview() {
                Loadable::Idle | Loadable::Loading(_) => {
                    ui.label("Looking for the latest matching run…");
                }
                Loadable::Ready(Some(record)) => {
                    ui.label("Latest matching run:");
                    ui.strong(describe_run(record, controller));
                    if ui.button("This is the right build").clicked() {
                        push(inputs, WizardInput::ConfirmPreview);
                    }
                }
                Loadable::Ready(None) => {
                    ui.label("No runs yet. The target exists, but nothing has run for this selection. You can still pin it; it will show \"No runs yet\" until the first run.");
                    if ui.button("Pin it anyway").clicked() {
                        push(inputs, WizardInput::ConfirmPreview);
                    }
                }
                Loadable::Failed(error) => {
                    let kind = if error.is_access() {
                        "Access error"
                    } else {
                        "Could not check"
                    };
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("{kind}: {}", error.message()),
                    );
                    ui.horizontal(|ui| {
                        if error.is_access() && ui.button("Reconnect").clicked() {
                            push(inputs, WizardInput::Reconnect);
                        }
                        if ui.button("Try again").clicked() {
                            push(inputs, WizardInput::Retry);
                        }
                    });
                }
            },
            Step::Label => {
                let mut label_text = wizard.label().to_owned();
                let label = ui.label("Short label shown on the strip");
                if ui
                    .text_edit_singleline(&mut label_text)
                    .labelled_by(label.id)
                    .changed()
                {
                    push(inputs, WizardInput::SetLabel(label_text));
                }
                if let Some(error) = wizard.label_error() {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if ui
                    .add_enabled(!wizard.is_pinning(), egui::Button::new("Pin"))
                    .clicked()
                {
                    push(inputs, WizardInput::Pin);
                }
            }
            Step::Pinned => {
                ui.label(format!(
                    "Pinned \"{}\". It is on the strip now.",
                    wizard.label()
                ));
                if ui.button("Add another build").clicked() {
                    self.wizard = AddBuildWizard::new();
                }
            }
        }
        if !matches!(self.wizard.step(), Step::ChooseProvider | Step::Pinned)
            && ui.button("Back").clicked()
        {
            self.filter.clear();
            inputs.push(Input::Wizard(WizardInput::Back));
        }
    }

    // ---------------------------------------------------------- connections

    fn connections_tab(
        &mut self,
        ui: &mut Ui,
        controller: &AppController,
        inputs: &mut Vec<Input>,
    ) {
        ui.label("Connections hold a read-only token in your operating system's credential store. Profiles refer to them by name only.");
        if let Some(message) = self.manage.message() {
            ui.label(message);
        }
        let prefs = controller.prefs();
        if prefs.connections.is_empty() {
            ui.label("No connections yet. Add a build to create one.");
        }
        for (alias, connection) in &prefs.connections {
            ui.horizontal(|ui| {
                let used = crate::settings::watches_using(controller.profile(), alias).len();
                ui.strong(alias.as_str());
                ui.label(format!(
                    "{} · {} · used by {used} build{}",
                    connection.provider,
                    connection.display_name,
                    if used == 1 { "" } else { "s" }
                ));
                if ui.button("Remove").clicked() {
                    inputs.push(Input::Manage(ManageInput::RemoveConnection(alias.clone())));
                }
            });
        }
    }

    // ---------------------------------------------------------------- share

    fn share_tab(&mut self, ui: &mut Ui, controller: &AppController, inputs: &mut Vec<Input>) {
        ui.heading("Export");
        ui.label("Share your pinned builds with a teammate. The file names connections but never contains tokens.");
        let text = export_text(controller.profile());
        ui.horizontal(|ui| {
            if ui.button("Copy profile").clicked() {
                ui.ctx().copy_text(text.clone());
                self.share_message = Some("Profile copied to the clipboard.".into());
            }
            let label = ui.label("Save to");
            ui.text_edit_singleline(&mut self.export_path)
                .labelled_by(label.id);
            if ui.button("Save file").clicked() {
                self.share_message = Some(
                    match AtomicFile::new(self.export_path.trim()).write(text.as_bytes()) {
                        Ok(()) => format!("Saved {}.", self.export_path.trim()),
                        Err(error) => format!("Could not save: {error}"),
                    },
                );
            }
        });
        if let Some(message) = &self.share_message {
            ui.label(message);
        }
        ui.separator();
        ui.heading("Import");
        if let Some(error) = self.import.error() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let push = |inputs: &mut Vec<Input>, input| inputs.push(Input::Import(input));
        match self.import.step() {
            ImportStep::Input => {
                ui.horizontal(|ui| {
                    let label = ui.label("Load file");
                    ui.text_edit_singleline(&mut self.import_path)
                        .labelled_by(label.id);
                    if ui.button("Load").clicked() {
                        match std::fs::read_to_string(self.import_path.trim()) {
                            Ok(text) => push(inputs, ImportInput::SetText(text)),
                            Err(error) => {
                                self.share_message =
                                    Some(format!("Could not read the file: {error}"))
                            }
                        }
                    }
                });
                let mut text = self.import.text().to_owned();
                let label = ui.label("Profile to import");
                let edit = ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(6)
                        .desired_width(f32::INFINITY),
                );
                let edit = edit.labelled_by(label.id);
                if edit.changed() {
                    push(inputs, ImportInput::SetText(text));
                }
                if ui.button("Preview import").clicked() {
                    push(inputs, ImportInput::Load);
                }
            }
            ImportStep::Mapping => {
                if let Some(preview) = self.import.preview() {
                    diff_summary(ui, preview.diff());
                }
                ui.label("Choose which of your connections each imported connection should use:");
                let ctx = context(controller);
                let required = self.import.required().to_vec();
                for (index, required) in required.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "\"{}\" ({}, {} build{})",
                            required.alias,
                            required.provider,
                            required.watch_count,
                            if required.watch_count == 1 { "" } else { "s" }
                        ));
                        let current = self.import.mapped(required).cloned();
                        let shown = current
                            .as_ref()
                            .map_or("Choose…".to_owned(), |a| a.to_string());
                        egui::ComboBox::from_id_salt(("map", index))
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                for (alias, who) in choices_for(required.provider, ctx) {
                                    let selected = current.as_ref() == Some(&alias);
                                    if ui
                                        .selectable_label(selected, format!("{alias} ({who})"))
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
                        if ui.button("New connection").clicked() {
                            push(inputs, ImportInput::CreateConnection(index));
                        }
                    });
                }
                if let Some((_, flow)) = self.import.creating() {
                    if let Some(input) = connect_form(ui, flow, controller) {
                        push(inputs, ImportInput::Connect(input));
                    }
                    if ui.button("Cancel new connection").clicked() {
                        push(inputs, ImportInput::CancelCreate);
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("Back").clicked() {
                        push(inputs, ImportInput::Back);
                    }
                    if ui.button("Review changes").clicked() {
                        push(inputs, ImportInput::Continue);
                    }
                });
            }
            ImportStep::Review => {
                if let Some((_, diff)) = self.import.applied() {
                    diff_summary(ui, diff);
                }
                ui.label("Your pinned builds will be replaced by these. Connections and tokens on this device are not changed.");
                ui.horizontal(|ui| {
                    if ui.button("Back").clicked() {
                        push(inputs, ImportInput::Back);
                    }
                    if ui
                        .add_enabled(
                            !self.import.is_saving(),
                            egui::Button::new("Replace my builds"),
                        )
                        .clicked()
                    {
                        push(inputs, ImportInput::Confirm);
                    }
                });
            }
            ImportStep::Done => {
                ui.label(format!(
                    "Imported. {} builds are pinned.",
                    controller.profile().len()
                ));
                if ui.button("Import another").clicked() {
                    self.import = ImportFlow::new();
                }
            }
        }
    }

    // --------------------------------------------------------------- device

    fn device_tab(
        &mut self,
        ui: &mut Ui,
        controller: &mut AppController,
        device: &DeviceInfo,
    ) -> SettingsOutcome {
        let mut prefs: DevicePrefs = controller.prefs().clone();
        let before = prefs.clone();
        ui.label("These settings stay on this computer and are never exported.");
        ui.horizontal(|ui| {
            ui.label("Theme");
            ui.radio_value(&mut prefs.theme, Theme::System, "System");
            ui.radio_value(&mut prefs.theme, Theme::Light, "Light");
            ui.radio_value(&mut prefs.theme, Theme::Dark, "Dark");
        });
        ui.horizontal(|ui| {
            ui.label("Placement");
            let docked = matches!(prefs.placement, Placement::DockedBottom);
            if ui.radio(docked, "Docked at the bottom edge").clicked() && !docked {
                prefs.placement = Placement::DockedBottom;
            }
            if ui.radio(!docked, "Floating window").clicked() && docked {
                prefs.placement = Placement::Floating { position: None };
            }
        });
        if let Some(note) = &device.placement_note {
            ui.weak(note);
        }
        let current = prefs.monitor.as_ref().map(|m| m.id.clone());
        egui::ComboBox::from_label("Display")
            .selected_text(current.clone().unwrap_or_else(|| "Primary display".into()))
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(current.is_none(), "Primary display")
                    .clicked()
                {
                    prefs.monitor = None;
                }
                for (id, description) in &device.monitors {
                    let selected = current.as_deref() == Some(id.as_str());
                    if ui.selectable_label(selected, description).clicked() {
                        prefs.monitor = Some(MonitorChoice {
                            id: id.clone(),
                            name: Some(description.clone()),
                        });
                    }
                }
            });
        let mut override_scale = prefs.scale.is_some();
        ui.horizontal(|ui| {
            ui.checkbox(&mut override_scale, "Override display scaling");
            let mut scale = prefs.scale.unwrap_or(1.0);
            if override_scale {
                ui.add(egui::Slider::new(&mut scale, 0.5..=4.0).step_by(0.25));
                prefs.scale = Some(scale);
            } else {
                prefs.scale = None;
            }
        });
        ui.checkbox(
            &mut prefs.launch_at_startup,
            "Start BuildRail when I log in",
        );
        if prefs != before {
            let placement_changed = prefs.placement != before.placement
                || prefs.monitor != before.monitor
                || prefs.scale != before.scale;
            match controller.update_prefs(|p| *p = prefs) {
                Ok(()) => {
                    self.device_message = None;
                    return SettingsOutcome { placement_changed };
                }
                Err(error) => self.device_message = Some(error),
            }
        }
        if let Some(message) = &self.device_message {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
        SettingsOutcome::default()
    }
}

fn context(controller: &AppController) -> SettingsContext<'_> {
    SettingsContext {
        profile: controller.profile(),
        connections: &controller.prefs().connections,
    }
}

/// Draws the paste-a-token form; returns the user's input, if any.
fn connect_form(
    ui: &mut Ui,
    flow: &ConnectFlow,
    controller: &AppController,
) -> Option<ConnectInput> {
    let ConnectFlow::Token(form) = flow;
    let help = guidance(form.provider);
    let mut input = None;
    ui.group(|ui| {
        ui.strong(format!("Connect with a {}", help.title));
        for (n, step) in help.steps.iter().enumerate() {
            ui.label(format!("{}. {step}", n + 1));
        }
        if ui.button("Open the token page").clicked() {
            let _ = controller.open_url(help.url);
        }
        ui.horizontal(|ui| {
            let label = ui.label("Connection name");
            let mut alias = form.alias.clone();
            let edit = ui.add_enabled(
                !form.alias_locked,
                egui::TextEdit::singleline(&mut alias).desired_width(160.0),
            );
            if edit.labelled_by(label.id).changed() {
                input = Some(ConnectInput::SetAlias(alias));
            }
        });
        ui.horizontal(|ui| {
            let label = ui.label("Token");
            let mut token = form.token.clone();
            let edit = ui.add(
                egui::TextEdit::singleline(&mut token)
                    .password(true)
                    .desired_width(260.0),
            );
            if edit.labelled_by(label.id).changed() {
                input = Some(ConnectInput::SetToken(token));
            }
        });
        if let Some(error) = &form.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let busy = flow.is_busy();
        let text = if busy { "Verifying…" } else { "Connect" };
        if ui.add_enabled(!busy, egui::Button::new(text)).clicked() {
            input = Some(ConnectInput::Submit);
        }
    });
    input
}

/// Shows a list's loading / error state; returns the items once loaded.
fn loadable<'a, T>(
    ui: &mut Ui,
    state: &'a Loadable<Vec<T>>,
    what: &str,
    inputs: &mut Vec<Input>,
) -> Option<&'a Vec<T>> {
    match state {
        Loadable::Idle | Loadable::Loading(_) => {
            ui.label(format!("Loading {what}…"));
            None
        }
        Loadable::Ready(items) => Some(items),
        Loadable::Failed(error) => {
            let kind = if error.is_access() {
                "Access error"
            } else {
                "Could not load"
            };
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("{kind}: {}", error.message()),
            );
            ui.horizontal(|ui| {
                if error.is_access() && ui.button("Reconnect").clicked() {
                    inputs.push(Input::Wizard(WizardInput::Reconnect));
                }
                if ui.button("Try again").clicked() {
                    inputs.push(Input::Wizard(WizardInput::Retry));
                }
            });
            None
        }
    }
}

fn filter_field(ui: &mut Ui, filter: &mut String) {
    ui.horizontal(|ui| {
        let label = ui.label("Filter");
        ui.text_edit_singleline(filter).labelled_by(label.id);
    });
}

fn describe(target: &WatchTarget) -> String {
    match target {
        WatchTarget::GitHub {
            owner,
            repo,
            workflow,
            branch,
        } => format!(
            "GitHub {owner}/{repo} · {workflow}{}",
            branch
                .as_ref()
                .map(|b| format!(" · {b}"))
                .unwrap_or_default()
        ),
        WatchTarget::Cloudflare { worker, branch, .. } => format!(
            "Cloudflare Worker {worker}{}",
            branch
                .as_ref()
                .map(|b| format!(" · {b}"))
                .unwrap_or_default()
        ),
    }
}

fn describe_run(record: &BuildRecord, controller: &AppController) -> String {
    let now = controller.now();
    let mut parts = vec![record.status.label().to_owned()];
    if let Some(branch) = &record.branch {
        parts.push(format!("branch {branch}"));
    }
    if let Some(commit) = &record.commit {
        parts.push(format!("commit {}", short_commit(commit)));
    }
    parts.push(age_short(now - record.updated_at));
    parts.join(" · ")
}

fn diff_summary(ui: &mut Ui, diff: &buildrail_profile::ProfileDiff) {
    let counts = diff.counts();
    ui.label(format!(
        "{} added, {} removed, {} changed, {} unchanged{}",
        counts.added,
        counts.removed,
        counts.changed,
        counts.unchanged,
        if diff.reordered {
            ", order changed"
        } else {
            ""
        }
    ));
    for entry in &diff.entries {
        let line = match &entry.kind {
            DiffKind::Added(w) => format!("+ {} ({})", w.label, describe(&w.target)),
            DiffKind::Removed(w) => format!("- {} ({})", w.label, describe(&w.target)),
            DiffKind::Changed { after, changes, .. } => {
                let what: Vec<&str> = changes
                    .iter()
                    .map(|c| match c {
                        WatchChange::Label => "label",
                        WatchChange::Connection => "connection",
                        WatchChange::Target => "target",
                        WatchChange::Branch => "branch",
                    })
                    .collect();
                format!("~ {} ({} changed)", after.label, what.join(", "))
            }
            DiffKind::Unchanged(w) => format!("= {}", w.label),
        };
        ui.label(RichText::new(line).monospace());
    }
}
