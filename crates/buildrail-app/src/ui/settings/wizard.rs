//! The Add-build wizard, shown on the Builds page: what to watch (choice
//! cards), which account, which target (picker lists), the latest run,
//! and the label. At most one primary button is in view at a time.

use buildrail_core::domain::ProviderKind;
use eframe::egui::Ui;

use super::forms::{connect_form, describe_run, filter_field, footer, loadable, loading, sentence};
use super::{count, Input, SettingsWindow};
use crate::app::AppController;
use crate::settings::wizard::{AddBuildWizard, Step, WizardInput};
use crate::settings::Loadable;
use crate::ui::theme::Tokens;
use crate::ui::widgets::{
    self, back_button, button, choice_cards, error_line, group, list_row, page_header, subhead,
    ButtonKind, Choice, TextField,
};

impl SettingsWindow {
    pub(super) fn wizard_page(
        &mut self,
        ui: &mut Ui,
        controller: &AppController,
        tokens: &Tokens,
        inputs: &mut Vec<Input>,
    ) {
        let push = |inputs: &mut Vec<Input>, input| inputs.push(Input::Wizard(input));
        let step = self.wizard.step();
        page_header(
            ui,
            tokens,
            "Add a build",
            "Pick what to watch, then check its latest run before it goes on the strip.",
        );
        let summary = self.wizard.summary();
        let going_back = !matches!(step, Step::ChooseProvider | Step::Pinned);
        if going_back || !summary.is_empty() {
            ui.horizontal(|ui| {
                if going_back && back_button(ui, tokens, "Back").clicked() {
                    self.filter.clear();
                    push(inputs, WizardInput::Back);
                }
                if !summary.is_empty() {
                    let galley = widgets::text(
                        ui,
                        crate::ui::theme::type_style::META,
                        &summary,
                        tokens.text_muted,
                    );
                    ui.add(eframe::egui::Label::new(eframe::egui::WidgetText::Galley(
                        galley,
                    )));
                }
            });
        }
        let wizard = &self.wizard;
        match step {
            Step::ChooseProvider => {
                subhead(ui, tokens, "What do you want to watch?");
                let chosen = wizard.provider().map(|p| match p {
                    ProviderKind::GitHub => 0,
                    ProviderKind::Cloudflare => 1,
                });
                let choices = [
                    Choice {
                        title: "GitHub Actions workflow",
                        text: "The latest run of a workflow, in any repository your token can see.",
                    },
                    Choice {
                        title: "Cloudflare Workers build",
                        text: "The latest Workers Builds deployment of one of your Workers.",
                    },
                ];
                match choice_cards(ui, tokens, &choices, chosen) {
                    Some(0) => push(inputs, WizardInput::ChooseProvider(ProviderKind::GitHub)),
                    Some(_) => push(
                        inputs,
                        WizardInput::ChooseProvider(ProviderKind::Cloudflare),
                    ),
                    None => {}
                }
            }
            Step::Connect => match wizard.connect_flow() {
                Some(flow) => {
                    let name = wizard.provider().map_or("", |p| p.display_name());
                    subhead(ui, tokens, &format!("Connect your {name} account"));
                    if let Some(input) = connect_form(ui, flow, controller, tokens) {
                        push(inputs, WizardInput::Connect(input));
                    }
                }
                None => {
                    subhead(ui, tokens, "Which account?");
                    let mut aliases = Vec::new();
                    let mut texts = Vec::new();
                    if let Some(provider) = wizard.provider() {
                        for alias in controller.prefs().connections_for(provider) {
                            let who = controller
                                .prefs()
                                .connection(alias)
                                .map(|c| c.display_name.clone())
                                .unwrap_or_default();
                            aliases.push(alias.clone());
                            texts.push(format!("Signed in as {who}, on this computer."));
                        }
                    }
                    let mut choices: Vec<Choice<'_>> = aliases
                        .iter()
                        .zip(&texts)
                        .map(|(alias, text)| Choice {
                            title: alias.as_str(),
                            text,
                        })
                        .collect();
                    choices.push(Choice {
                        title: "Connect another account",
                        text: "Paste a read-only token for a different account.",
                    });
                    if let Some(index) = choice_cards(ui, tokens, &choices, None) {
                        match aliases.get(index) {
                            Some(alias) => push(inputs, WizardInput::UseConnection(alias.clone())),
                            None => push(inputs, WizardInput::NewConnection),
                        }
                    }
                }
            },
            Step::Repository => {
                subhead(ui, tokens, "Which repository?");
                if let Some(repos) =
                    loadable(ui, tokens, wizard.repositories(), "repositories", inputs)
                {
                    if repos.is_empty() {
                        sentence(ui, tokens, "This token cannot see any repositories. Give it access to the repositories you want to watch, then try again.");
                    } else {
                        filter_field(ui, tokens, &mut self.filter, "keevoro");
                        let filter = self.filter.to_lowercase();
                        let shown: Vec<_> = repos
                            .iter()
                            .filter(|r| r.full_name.to_lowercase().contains(&filter))
                            .collect();
                        list_note(
                            ui,
                            tokens,
                            shown.len(),
                            repos.len(),
                            ("repository", "repositories"),
                        );
                        group(ui, tokens, |g| {
                            g.flush(|ui| {
                                for (i, repo) in shown.into_iter().enumerate() {
                                    let meta = repo.archived.then_some("Archived");
                                    if list_row(ui, tokens, &repo.full_name, meta, None, i > 0)
                                        .clicked()
                                    {
                                        push(inputs, WizardInput::SelectRepository(repo.clone()));
                                    }
                                }
                            });
                        });
                    }
                }
            }
            Step::Workflow => {
                subhead(ui, tokens, "Which workflow?");
                if let Some(workflows) =
                    loadable(ui, tokens, wizard.workflows(), "workflows", inputs)
                {
                    if workflows.is_empty() {
                        sentence(ui, tokens, "This repository has no workflows. Add one under .github/workflows, then try again.");
                    } else {
                        group(ui, tokens, |g| {
                            g.flush(|ui| {
                                for (i, workflow) in workflows.iter().enumerate() {
                                    let key = workflow.watch_key();
                                    if list_row(ui, tokens, &workflow.name, Some(&key), None, i > 0)
                                        .clicked()
                                    {
                                        push(inputs, WizardInput::SelectWorkflow(workflow.clone()));
                                    }
                                }
                            });
                        });
                    }
                }
            }
            Step::Branch => self.branch_step(ui, controller, tokens, inputs),
            Step::Account => {
                subhead(ui, tokens, "Which Cloudflare account?");
                if let Some(accounts) = loadable(ui, tokens, wizard.accounts(), "accounts", inputs)
                {
                    group(ui, tokens, |g| {
                        g.flush(|ui| {
                            for (i, account) in accounts.iter().enumerate() {
                                if list_row(ui, tokens, &account.name, None, None, i > 0).clicked()
                                {
                                    push(inputs, WizardInput::SelectAccount(account.clone()));
                                }
                            }
                        });
                    });
                }
            }
            Step::Worker => {
                subhead(ui, tokens, "Which Worker?");
                if let Some(workers) = loadable(ui, tokens, wizard.workers(), "Workers", inputs) {
                    if workers.is_empty() {
                        sentence(ui, tokens, "This account has no Workers yet.");
                    } else {
                        filter_field(ui, tokens, &mut self.filter, "marketing-site");
                        let filter = self.filter.to_lowercase();
                        let shown: Vec<_> = workers
                            .iter()
                            .filter(|w| w.name.contains(&filter))
                            .collect();
                        list_note(
                            ui,
                            tokens,
                            shown.len(),
                            workers.len(),
                            ("Worker", "Workers"),
                        );
                        group(ui, tokens, |g| {
                            g.flush(|ui| {
                                for (i, worker) in shown.into_iter().enumerate() {
                                    if list_row(ui, tokens, &worker.name, None, None, i > 0)
                                        .clicked()
                                    {
                                        push(inputs, WizardInput::SelectWorker(worker.clone()));
                                    }
                                }
                            });
                        });
                    }
                }
            }
            Step::Preview => preview_step(ui, wizard, controller, tokens, inputs),
            Step::Label => {
                subhead(ui, tokens, "What should the strip call it?");
                let mut label_text = wizard.label().to_owned();
                group(ui, tokens, |g| {
                    g.row("Label", Some("Shown on the strip. Keep it short."), |ui| {
                        let edit = TextField::new(&mut label_text, "Label")
                            .placeholder("Keevoro")
                            .width(220.0)
                            .show(ui, tokens);
                        if edit.changed() {
                            push(inputs, WizardInput::SetLabel(label_text.clone()));
                        }
                    });
                });
                if let Some(error) = wizard.label_error() {
                    error_line(ui, tokens, error);
                }
                let busy = wizard.is_pinning();
                let label = if busy { "Pinning…" } else { "Pin build" };
                if footer(ui, tokens, false, Some((label, !busy))).1 {
                    push(inputs, WizardInput::Pin);
                }
            }
            Step::Pinned => {
                widgets::toast(
                    ui,
                    tokens,
                    widgets::Tone::Success,
                    &format!("{} is on the strip now.", wizard.label()),
                );
                let mut again = false;
                let mut done = false;
                ui.horizontal(|ui| {
                    again = button(
                        ui,
                        tokens,
                        ButtonKind::Plain,
                        "Add another build",
                        true,
                        false,
                    )
                    .clicked();
                    done = button(ui, tokens, ButtonKind::Quiet, "Done", true, false).clicked();
                });
                if again {
                    self.wizard = AddBuildWizard::new();
                }
                if done {
                    self.adding = false;
                }
            }
        }
    }

    fn branch_step(
        &mut self,
        ui: &mut Ui,
        _controller: &AppController,
        tokens: &Tokens,
        inputs: &mut Vec<Input>,
    ) {
        let push = |inputs: &mut Vec<Input>, input| inputs.push(Input::Wizard(input));
        let wizard = &self.wizard;
        let current = wizard.branch().map(str::to_owned);
        subhead(ui, tokens, "Which branch?");
        match wizard.provider() {
            Some(ProviderKind::GitHub) => {
                let branches = match wizard.branches() {
                    Loadable::Ready(branches) => Some(branches),
                    _ => None,
                };
                group(ui, tokens, |g| {
                    g.flush(|ui| {
                        if list_row(
                            ui,
                            tokens,
                            "Any branch",
                            None,
                            Some(current.is_none()),
                            false,
                        )
                        .clicked()
                        {
                            push(inputs, WizardInput::SetBranch(None));
                        }
                        for branch in branches.into_iter().flatten() {
                            let chosen = current.as_deref() == Some(branch.name.as_str());
                            if list_row(ui, tokens, &branch.name, None, Some(chosen), true)
                                .clicked()
                            {
                                push(inputs, WizardInput::SetBranch(Some(branch.name.clone())));
                            }
                        }
                    });
                });
                if branches.is_none() {
                    let _ = loadable(ui, tokens, wizard.branches(), "branches", inputs);
                }
            }
            _ => {
                let mut text = current.unwrap_or_default();
                group(ui, tokens, |g| {
                    g.row(
                        "Branch",
                        Some("Optional. Left blank, a build on any branch counts."),
                        |ui| {
                            let edit = TextField::new(&mut text, "Branch")
                                .placeholder("main")
                                .width(220.0)
                                .show(ui, tokens);
                            if edit.changed() {
                                push(inputs, WizardInput::SetBranch(Some(text.clone())));
                            }
                        },
                    );
                });
            }
        }
        if footer(ui, tokens, false, Some(("Check latest run", true))).1 {
            push(inputs, WizardInput::ConfirmBranch);
        }
    }
}

/// "12 repositories", or "3 of 12 repositories" while filtered.
fn list_note(ui: &mut Ui, tokens: &Tokens, shown: usize, total: usize, noun: (&str, &str)) {
    let text = if shown == total {
        format!("{}.", count(total, noun.0, noun.1))
    } else {
        format!("{shown} of {}.", count(total, noun.0, noun.1))
    };
    sentence(ui, tokens, &text);
}

fn preview_step(
    ui: &mut Ui,
    wizard: &AddBuildWizard,
    controller: &AppController,
    tokens: &Tokens,
    inputs: &mut Vec<Input>,
) {
    let push = |inputs: &mut Vec<Input>, input| inputs.push(Input::Wizard(input));
    subhead(ui, tokens, "Is this the right build?");
    match wizard.preview() {
        Loadable::Idle | Loadable::Loading(_) => {
            loading(ui, tokens, "Looking for the latest matching run…");
        }
        Loadable::Ready(Some(record)) => {
            let run = describe_run(record, controller);
            group(ui, tokens, |g| {
                g.row("Latest run", Some(&run), |_| {});
            });
            if footer(ui, tokens, false, Some(("Use this build", true))).1 {
                push(inputs, WizardInput::ConfirmPreview);
            }
        }
        Loadable::Ready(None) => {
            sentence(ui, tokens, "Nothing has run for this yet. You can still pin it: the strip says “No runs yet” until the first run.");
            if footer(ui, tokens, false, Some(("Use it anyway", true))).1 {
                push(inputs, WizardInput::ConfirmPreview);
            }
        }
        Loadable::Failed(error) => {
            let what = if error.is_access() {
                "BuildRail could not check the latest run: access was refused."
            } else {
                "BuildRail could not check the latest run."
            };
            error_line(ui, tokens, &format!("{what} {}", error.message()));
            ui.horizontal(|ui| {
                if error.is_access()
                    && button(ui, tokens, ButtonKind::Plain, "Reconnect", true, false).clicked()
                {
                    push(inputs, WizardInput::Reconnect);
                }
                if button(ui, tokens, ButtonKind::Primary, "Try again", true, false).clicked() {
                    push(inputs, WizardInput::Retry);
                }
            });
        }
    }
}
