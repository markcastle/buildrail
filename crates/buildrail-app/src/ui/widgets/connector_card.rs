//! One connected thing and what can be done to it, mirroring Task Rail's
//! `ConnectorCard`: a state dot, the name (14/600) and a count on the
//! right; one muted line of facts joined with `·` (or the error, in the
//! person's terms, in its place); then quiet buttons in order of use, a
//! spacer, and the one red action set apart.

use eframe::egui::{Align, Color32, Label, Layout, Ui, Vec2, WidgetText};

use super::button::{button, ButtonKind};
use super::{text, wrapped};
use crate::ui::theme::{type_style, Tokens};

/// A quiet action under the card.
#[derive(Debug, Clone)]
pub struct CardAction<'a> {
    /// The visible verb ("Rename").
    pub label: &'a str,
    /// What the tooltip adds ("Move Keevoro earlier on the strip").
    pub tooltip: Option<String>,
    /// Greyed out when it cannot apply (the first build cannot move left).
    pub enabled: bool,
    /// Removes something: red, after the spacer.
    pub danger: bool,
}

impl<'a> CardAction<'a> {
    /// An enabled quiet action.
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            tooltip: None,
            enabled: true,
            danger: false,
        }
    }

    /// The red action, set apart at the end.
    pub fn danger(label: &'a str) -> Self {
        Self {
            danger: true,
            ..Self::new(label)
        }
    }

    /// Adds a tooltip.
    pub fn tooltip(mut self, tooltip: String) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    /// Enables or disables it.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// What a card shows.
#[derive(Debug, Clone)]
pub struct ConnectorCard<'a> {
    /// The state dot's colour (a state token: `done`, `priority`, …).
    pub dot: Color32,
    /// The thing's name.
    pub name: &'a str,
    /// The count or state on the right ("1 build", "Failed").
    pub count: Option<String>,
    /// The colour of the count (`text-muted` unless it reports a state).
    pub count_color: Option<Color32>,
    /// Facts, joined with `·` into one line.
    pub facts: Vec<String>,
    /// An error that replaces the facts.
    pub error: Option<String>,
}

/// Draws a card's lines and actions; returns the index of the action
/// clicked this frame. Call inside a group block (it adds no padding).
pub fn connector_card(
    ui: &mut Ui,
    tokens: &Tokens,
    card: &ConnectorCard<'_>,
    actions: &[CardAction<'_>],
) -> Option<usize> {
    let width = ui.available_width();
    ui.spacing_mut().item_spacing = Vec2::new(10.0, 6.0);
    ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(Vec2::new(9.0, 18.0), eframe::egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 4.5, card.dot);
        let name = text(ui, type_style::LABEL, card.name, tokens.text);
        ui.add(Label::new(WidgetText::Galley(name)).selectable(false));
        if let Some(count) = &card.count {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let color = card.count_color.unwrap_or(tokens.text_muted);
                let count = text(ui, type_style::HELP, count, color);
                ui.add(Label::new(WidgetText::Galley(count)).selectable(false));
            });
        }
    });
    let (line, color) = match &card.error {
        Some(error) => (error.clone(), tokens.error_text()),
        None => (card.facts.join(" · "), tokens.text_muted),
    };
    if !line.is_empty() {
        let galley = wrapped(ui, type_style::HELP, &line, color, width);
        ui.add(Label::new(WidgetText::Galley(galley)).selectable(false));
    }
    let mut clicked = None;
    if !actions.is_empty() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let draw = |ui: &mut Ui, index: usize, action: &CardAction<'_>| {
                let kind = if action.danger {
                    ButtonKind::QuietDanger
                } else {
                    ButtonKind::Quiet
                };
                let mut response = button(ui, tokens, kind, action.label, action.enabled, true);
                if let Some(tooltip) = &action.tooltip {
                    response = response.on_hover_text(tooltip);
                }
                response.clicked().then_some(index)
            };
            for (index, action) in actions.iter().enumerate().filter(|(_, a)| !a.danger) {
                clicked = clicked.or(draw(ui, index, action));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                for (index, action) in actions.iter().enumerate().filter(|(_, a)| a.danger) {
                    clicked = clicked.or(draw(ui, index, action));
                }
            });
        });
    }
    clicked
}
