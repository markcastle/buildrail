//! What the Settings pages share: the token form, list loading with its
//! error, and the words that describe a build, a run and an import.

use buildrail_core::domain::{BuildRecord, WatchTarget};
use buildrail_profile::{DiffKind, ProfileDiff, WatchChange};
use eframe::egui::{Align, Layout, Ui};

use super::Input;
use crate::app::AppController;
use crate::presentation::{age_short, short_commit};
use crate::settings::connect::{guidance, ConnectFlow, ConnectInput};
use crate::settings::wizard::WizardInput;
use crate::settings::Loadable;
use crate::ui::theme::Tokens;
use crate::ui::widgets::{self, button, error_line, group, ButtonKind, Group, TextField};

/// Draws the paste-a-token form: how to get a token, then the connection
/// name and the token (a password field whose help says where it is kept
/// and which read-only permissions it needs), then Connect. Returns the
/// person's input, if any.
pub(super) fn connect_form(
    ui: &mut Ui,
    flow: &ConnectFlow,
    controller: &AppController,
    tokens: &Tokens,
) -> Option<ConnectInput> {
    group(ui, tokens, |g| connect_rows(g, flow, controller, tokens))
}

/// The rows of [`connect_form`], added to a group that is already open
/// (beneath a build's card when it reconnects).
pub(super) fn connect_rows(
    g: &mut Group<'_>,
    flow: &ConnectFlow,
    controller: &AppController,
    tokens: &Tokens,
) -> Option<ConnectInput> {
    let ConnectFlow::Token(form) = flow;
    let help = guidance(form.provider);
    let mut input = None;
    {
        g.label("Get a token");
        g.block(|ui| {
            sentence(
                ui,
                tokens,
                &format!("BuildRail reads your builds with a {}.", help.title),
            );
            for (n, step) in help.steps.iter().enumerate() {
                let line = format!("{}. {step}", n + 1);
                let galley = widgets::wrapped(
                    ui,
                    crate::ui::theme::type_style::BODY,
                    &line,
                    tokens.text,
                    ui.available_width(),
                );
                ui.add(eframe::egui::Label::new(eframe::egui::WidgetText::Galley(
                    galley,
                )));
            }
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if button(
                    ui,
                    tokens,
                    ButtonKind::Plain,
                    "Open the token page",
                    true,
                    false,
                )
                .on_hover_text(help.url)
                .clicked()
                {
                    let _ = controller.open_url(help.url);
                }
            });
        });
        g.label("Connect");
        g.row(
            "Connection name",
            Some("What BuildRail calls this account. Shared profiles refer to it by this name."),
            |ui| {
                let mut alias = form.alias.clone();
                let edit = TextField::new(&mut alias, "Connection name")
                    .placeholder("work")
                    .enabled(!form.alias_locked)
                    .width(200.0)
                    .show(ui, tokens);
                if edit.changed() {
                    input = Some(ConnectInput::SetAlias(alias));
                }
            },
        );
        let token_help = format!(
            "Kept in your operating system's credential store, never in a file. {}",
            help.permissions
        );
        g.stack("Token", Some(&token_help), |ui| {
            let mut token = form.token.clone();
            let edit = TextField::new(&mut token, "Token")
                .placeholder(help.placeholder)
                .password(true)
                .width(ui.available_width())
                .show(ui, tokens);
            if edit.changed() {
                input = Some(ConnectInput::SetToken(token));
            }
            if let Some(error) = &form.error {
                error_line(ui, tokens, error);
            }
            let busy = flow.is_busy();
            let label = if busy { "Checking…" } else { "Connect" };
            ui.add_space(2.0);
            if button(ui, tokens, ButtonKind::Primary, label, !busy, false).clicked() {
                input = Some(ConnectInput::Submit);
            }
        });
    }
    input
}

/// Shows a list's loading state or error (with Reconnect and Try again);
/// returns the items once they are in.
pub(super) fn loadable<'a, T>(
    ui: &mut Ui,
    tokens: &Tokens,
    state: &'a Loadable<Vec<T>>,
    what: &str,
    inputs: &mut Vec<Input>,
) -> Option<&'a Vec<T>> {
    match state {
        Loadable::Idle | Loadable::Loading(_) => {
            loading(ui, tokens, &format!("Loading {what}…"));
            None
        }
        Loadable::Ready(items) => Some(items),
        Loadable::Failed(error) => {
            let what = if error.is_access() {
                format!("BuildRail could not load your {what}: access was refused.")
            } else {
                format!("BuildRail could not load your {what}.")
            };
            error_line(ui, tokens, &format!("{what} {}", error.message()));
            ui.horizontal(|ui| {
                if error.is_access()
                    && button(ui, tokens, ButtonKind::Plain, "Reconnect", true, false).clicked()
                {
                    inputs.push(Input::Wizard(WizardInput::Reconnect));
                }
                if button(ui, tokens, ButtonKind::Primary, "Try again", true, false).clicked() {
                    inputs.push(Input::Wizard(WizardInput::Retry));
                }
            });
            None
        }
    }
}

/// A muted "Loading…" line with the pending glyph.
pub(super) fn loading(ui: &mut Ui, tokens: &Tokens, sentence: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(
            eframe::egui::Vec2::new(16.0, 18.0),
            eframe::egui::Sense::hover(),
        );
        crate::ui::glyphs::paint_icon(
            ui.painter(),
            rect.center(),
            15.0,
            crate::presentation::IconKind::Spinner,
            tokens.text_muted,
            tokens.bg,
            ui.input(|i| i.time),
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(60));
        let galley = widgets::text(
            ui,
            crate::ui::theme::type_style::BODY,
            sentence,
            tokens.text_muted,
        );
        ui.add(eframe::egui::Label::new(eframe::egui::WidgetText::Galley(
            galley,
        )));
    });
}

/// A filter field above a long list.
pub(super) fn filter_field(ui: &mut Ui, tokens: &Tokens, filter: &mut String, example: &str) {
    TextField::new(filter, "Filter")
        .placeholder(example)
        .width(320.0)
        .show(ui, tokens);
}

/// A sentence in the body style, muted.
pub(super) fn sentence(ui: &mut Ui, tokens: &Tokens, text: &str) {
    let width = ui.available_width().min(560.0);
    let galley = widgets::wrapped(
        ui,
        crate::ui::theme::type_style::BODY,
        text,
        tokens.text_muted,
        width,
    );
    ui.add(eframe::egui::Label::new(eframe::egui::WidgetText::Galley(
        galley,
    )));
}

/// A row of buttons: the way back on the left, the primary on the right.
pub(super) fn footer(
    ui: &mut Ui,
    tokens: &Tokens,
    back: bool,
    primary: Option<(&str, bool)>,
) -> (bool, bool) {
    let mut pressed = (false, false);
    ui.horizontal(|ui| {
        ui.set_width(ui.available_width().min(widgets::group::MAX_WIDTH));
        if back {
            pressed.0 = widgets::back_button(ui, tokens, "Back").clicked();
        }
        if let Some((label, enabled)) = primary {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                pressed.1 =
                    button(ui, tokens, ButtonKind::Primary, label, enabled, false).clicked();
            });
        }
    });
    pressed
}

/// "GitHub · acme/keevoro · ci.yml · main".
pub(super) fn describe(target: &WatchTarget) -> String {
    let mut parts = match target {
        WatchTarget::GitHub {
            owner,
            repo,
            workflow,
            ..
        } => vec![
            "GitHub".to_owned(),
            format!("{owner}/{repo}"),
            workflow.clone(),
        ],
        WatchTarget::Cloudflare { worker, .. } => {
            vec!["Cloudflare".to_owned(), format!("Worker {worker}")]
        }
    };
    parts.push(match target.branch() {
        Some(branch) => format!("branch {branch}"),
        None => "any branch".to_owned(),
    });
    parts.join(" · ")
}

/// "Passed · branch main · commit a7c412e · 3 min ago".
pub(super) fn describe_run(record: &BuildRecord, controller: &AppController) -> String {
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

/// What an import changes: one sentence of counts, then a group with a row
/// per build saying what happens to it.
pub(super) fn diff_summary(ui: &mut Ui, tokens: &Tokens, diff: &ProfileDiff) {
    let counts = diff.counts();
    let mut parts = Vec::new();
    for (n, word) in [
        (counts.added, "added"),
        (counts.removed, "removed"),
        (counts.changed, "changed"),
        (counts.unchanged, "unchanged"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {word}"));
        }
    }
    if diff.reordered {
        parts.push("the order changes".to_owned());
    }
    let summary = if parts.is_empty() {
        "Nothing changes.".to_owned()
    } else {
        format!("{}.", parts.join(", "))
    };
    sentence(ui, tokens, &summary);
    group(ui, tokens, |g| {
        for entry in &diff.entries {
            let (label, what, target) = match &entry.kind {
                DiffKind::Added(w) => (&w.label, "Added".to_owned(), describe(&w.target)),
                DiffKind::Removed(w) => (&w.label, "Removed".to_owned(), describe(&w.target)),
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
                    (
                        &after.label,
                        format!("Changed {}", what.join(", ")),
                        describe(&after.target),
                    )
                }
                DiffKind::Unchanged(w) => (&w.label, "Unchanged".to_owned(), describe(&w.target)),
            };
            let help = format!("{what} · {target}");
            g.row(label, Some(&help), |_| {});
        }
    });
}
