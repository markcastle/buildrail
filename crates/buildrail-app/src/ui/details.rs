//! The details panel: what [`DetailsModel`] says about one build, drawn in
//! the Task Rail manner on a raised card.
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │ ✕ Keevoro · Failed                        [x] │
//! │ GitHub · acme/keevoro · ci.yml                │
//! │ THIS RUN                                      │
//! │ Branch     main                               │
//! │ Commit     a7c412e · Fix the flaky login test │
//! │ …                                             │
//! │ JOBS                                          │
//! │ ✕ test (ubuntu-latest)               2 min 5 s│
//! │   Failed at “cargo test”.                     │
//! │ RECENT RUNS                                   │
//! │ ✕ 2 min ago  ✓ 1 h ago  ✓ 3 h ago             │
//! │ ───────────────────────────────────────────── │
//! │ [Open run] Refresh now  Copy link  Edit in …  │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! The header and the actions stay put; the sections between them scroll
//! when they are taller than [`BODY_MAX_HEIGHT`]. Every row that opens
//! something is a real egui widget (focusable with Tab, activated with
//! Enter or Space, named for screen readers); the facts are labels with
//! their full sentence as the accessible name. Nothing here asks for
//! frames: the strip decides when to paint.

use eframe::egui::{
    self, pos2, CornerRadius, Frame, Galley, Margin, Rect, Response, Role, Sense, Stroke, Ui, Vec2,
    WidgetInfo,
};

use super::glyphs::{paint_close, paint_icon};
use super::theme::{radius, type_style, Tokens, TypeStyle};
use super::widgets::{self, button, focus_ring, ButtonKind};
use crate::presentation::{DetailsModel, JobRow, PanelAction, PanelButton, RecentRun};

/// The panel card's width in points (inside the window's shadow margin).
pub const PANEL_WIDTH: f32 = 400.0;
/// Padding inside the card.
pub const PANEL_PAD: Vec2 = Vec2::new(16.0, 14.0);
/// The tallest the scrolling middle grows before it scrolls.
pub const BODY_MAX_HEIGHT: f32 = 520.0;
/// The facts' label column.
const LABEL_COLUMN: f32 = 76.0;
/// Status glyph size in rows.
const ROW_ICON: f32 = 13.0;

/// What the panel remembers between frames.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PanelState {
    /// The link was just copied ("Link copied." shows until the panel
    /// closes or shows another build).
    pub copied: bool,
    /// Move keyboard focus to the primary button on the next frame (the
    /// panel was opened from the keyboard).
    pub focus_primary: bool,
    /// The card's height last frame, in points (0 before the first).
    pub height: f32,
}

fn inner_width() -> f32 {
    PANEL_WIDTH - 2.0 * PANEL_PAD.x
}

fn galley(ui: &Ui, style: TypeStyle, text: &str, color: egui::Color32) -> std::sync::Arc<Galley> {
    widgets::text(ui, style, text, color)
}

/// A wrapped label with `name` as its accessible name.
fn words(ui: &mut Ui, style: TypeStyle, text: &str, color: egui::Color32) -> Response {
    let g = widgets::wrapped(ui, style, text, color, ui.available_width());
    let (rect, response) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, color);
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
    response
}

fn section_heading(ui: &mut Ui, tokens: &Tokens, text: &str) {
    ui.add_space(10.0);
    let g = galley(ui, type_style::GROUP_LABEL, text, tokens.text_muted);
    let (rect, response) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, tokens.text_muted);
    response.widget_info(|| WidgetInfo::labeled(Role::Heading, true, text));
    ui.add_space(4.0);
}

/// Draws the panel's card for `model` into `ui` and returns what was asked
/// for. `state.height` is updated with the card's height.
pub fn details_panel(
    ui: &mut Ui,
    model: &DetailsModel,
    tokens: &Tokens,
    state: &mut PanelState,
) -> Option<PanelAction> {
    let mut action = None;
    let shown = Frame::new()
        .fill(tokens.bg_raised)
        .stroke(Stroke::new(1.0, tokens.line))
        .corner_radius(CornerRadius::same(radius::R12))
        .shadow(tokens.card_shadow[1].egui())
        .inner_margin(Margin::symmetric(PANEL_PAD.x as i8, PANEL_PAD.y as i8))
        .show(ui, |ui| {
            ui.set_width(inner_width());
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
            header(ui, model, tokens, &mut action);
            egui::ScrollArea::vertical()
                .max_height(BODY_MAX_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_width(inner_width());
                    body(ui, model, tokens, &mut action);
                });
            actions(ui, model, tokens, state, &mut action);
        });
    state.height = shown.response.rect.height();
    if let Some(PanelAction::CopyLink(url)) = &action {
        ui.ctx().copy_text(url.clone());
        state.copied = true;
    }
    action
}

fn header(ui: &mut Ui, model: &DetailsModel, tokens: &Tokens, action: &mut Option<PanelAction>) {
    let ink = tokens.status(model.tone);
    let width = inner_width();
    let close_size = 24.0;
    let mut job = type_style::CARD_TITLE.job(&model.label, tokens.text);
    job.append(" · ", 0.0, type_style::BODY.format(tokens.text_muted));
    job.append(
        &model.status_text,
        0.0,
        type_style::CARD_TITLE.format(ink.word),
    );
    job.wrap.max_width = width - 22.0 - close_size - 8.0;
    let title = ui.painter().layout_job(job);
    let height = title.size().y.max(close_size);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let heading = model.accessible_heading();
    response.widget_info(|| WidgetInfo::labeled(Role::Heading, true, &heading));
    let painter = ui.painter();
    let first_line = title.rows.first().map_or(16.0, |r| r.rect().height());
    let top = rect.center().y - title.size().y / 2.0;
    paint_icon(
        painter,
        pos2(rect.min.x + 8.0, top + first_line / 2.0),
        16.0,
        model.icon,
        ink.glyph,
        tokens.bg_raised,
        0,
    );
    painter.galley(pos2(rect.min.x + 22.0, top), title, tokens.text);
    let close = Rect::from_min_size(
        pos2(rect.max.x - close_size, rect.center().y - close_size / 2.0),
        Vec2::splat(close_size),
    );
    let response = widgets::icon_button_at(
        ui,
        close,
        tokens,
        "Close details",
        "Close (Esc)",
        &|p, c, s, color| paint_close(p, c, s * 0.8, color),
    );
    if response.clicked() {
        *action = Some(PanelAction::Close);
    }
    words(ui, type_style::META, &model.facts, tokens.text_muted);
    if let Some(notice) = &model.notice {
        ui.add_space(6.0);
        if let Some(heading) = &notice.heading {
            words(ui, type_style::LABEL, heading, tokens.text);
        }
        let color = if notice.problem {
            tokens.error_text()
        } else {
            tokens.text
        };
        words(ui, type_style::HELP, &notice.text, color);
    }
    if let Some(freshness) = &model.freshness {
        ui.add_space(2.0);
        words(ui, type_style::META, freshness, tokens.text_muted);
    }
}

fn body(ui: &mut Ui, model: &DetailsModel, tokens: &Tokens, action: &mut Option<PanelAction>) {
    if !model.run.is_empty() {
        section_heading(ui, tokens, "This run");
        for fact in &model.run {
            fact_row(ui, tokens, fact);
        }
    }
    if let Some(line) = &model.load_line {
        ui.add_space(8.0);
        let color = if line.failed {
            tokens.error_text()
        } else {
            tokens.text_muted
        };
        words(ui, type_style::HELP, &line.text, color);
        if line.failed {
            ui.add_space(2.0);
            if button(ui, tokens, ButtonKind::Plain, "Try again", true, true).clicked() {
                *action = Some(PanelAction::Retry);
            }
        }
    }
    if let Some(jobs) = &model.jobs {
        section_heading(ui, tokens, jobs.heading);
        if let Some(empty) = &jobs.empty {
            words(ui, type_style::HELP, empty, tokens.text_muted);
        }
        for row in &jobs.rows {
            if job_row(ui, tokens, row).clicked() {
                if let Some(url) = &row.url {
                    *action = Some(PanelAction::Open(url.clone()));
                }
            }
        }
        if let Some(more) = &jobs.more {
            if link(ui, tokens, &more.text, &more.accessible_name).clicked() {
                *action = Some(PanelAction::Open(more.url.clone()));
            }
        }
    }
    if let Some(recent) = &model.recent {
        section_heading(ui, tokens, recent.heading);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
            for run in &recent.runs {
                if recent_chip(ui, tokens, run).clicked() {
                    *action = Some(PanelAction::Open(run.url.clone()));
                }
            }
        });
        if let Some(summary) = &recent.summary {
            ui.add_space(2.0);
            words(ui, type_style::META, summary, tokens.text_muted);
        }
    }
    ui.add_space(4.0);
}

fn fact_row(ui: &mut Ui, tokens: &Tokens, fact: &crate::presentation::Fact) {
    let width = ui.available_width();
    let label = galley(ui, type_style::META, fact.label, tokens.text_muted);
    let value = widgets::wrapped(
        ui,
        type_style::HELP,
        &fact.value,
        tokens.text,
        width - LABEL_COLUMN,
    );
    let height = value.size().y.max(label.size().y) + 2.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let name = fact.accessible_name();
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, &name));
    let painter = ui.painter();
    let first = value
        .rows
        .first()
        .map_or(label.size().y, |r| r.rect().height());
    painter.galley(
        pos2(
            rect.min.x,
            rect.min.y + 1.0 + (first - label.size().y) / 2.0,
        ),
        label,
        tokens.text_muted,
    );
    painter.galley(
        pos2(rect.min.x + LABEL_COLUMN, rect.min.y + 1.0),
        value,
        tokens.text,
    );
    if let Some(more) = &fact.description {
        response.on_hover_text(more);
    }
}

/// Lights a clickable row: the `bg-hover` pill under the pointer, the
/// focus ring with the keyboard.
fn light(ui: &Ui, response: &Response, rect: Rect, tokens: &Tokens) -> bool {
    let lit = response.hovered() || response.has_focus();
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::R8), tokens.bg_hover);
    }
    if response.has_focus() {
        focus_ring(ui.painter(), rect.shrink(2.0), radius::R8 - 2, tokens);
    }
    lit
}

fn job_row(ui: &mut Ui, tokens: &Tokens, row: &JobRow) -> Response {
    const INSET: f32 = 6.0;
    let width = ui.available_width();
    let ink = tokens.status(row.tone);
    let duration = row
        .duration
        .as_ref()
        .map(|d| galley(ui, type_style::META, d, tokens.text_muted));
    let duration_width = duration.as_ref().map_or(0.0, |d| d.size().x + 10.0);
    let text_x = INSET + ROW_ICON + 8.0;
    let name = widgets::wrapped(
        ui,
        type_style::BODY,
        &row.name,
        tokens.text,
        width - text_x - INSET - duration_width,
    );
    let step = row.failing_step.as_ref().map(|s| {
        widgets::wrapped(
            ui,
            type_style::HELP,
            s,
            tokens.error_text(),
            width - text_x - INSET,
        )
    });
    let height = 4.0 + name.size().y + step.as_ref().map_or(0.0, |s| 1.0 + s.size().y) + 4.0;
    let sense = if row.url.is_some() {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), sense);
    let role = if row.url.is_some() {
        Role::Link
    } else {
        Role::Label
    };
    response.widget_info(|| WidgetInfo::labeled(role, true, &row.accessible_name));
    let lit = light(ui, &response, rect, tokens) && response.hovered();
    let ground = if lit {
        tokens.bg_hover
    } else {
        tokens.bg_raised
    };
    let painter = ui.painter();
    let first = name.rows.first().map_or(16.0, |r| r.rect().height());
    let top = rect.min.y + 4.0;
    paint_icon(
        painter,
        pos2(rect.min.x + INSET + ROW_ICON / 2.0, top + first / 2.0),
        ROW_ICON,
        row.icon,
        ink.glyph,
        ground,
        0,
    );
    let name_height = name.size().y;
    painter.galley(pos2(rect.min.x + text_x, top), name, tokens.text);
    if let Some(duration) = duration {
        let at = pos2(
            rect.max.x - INSET - duration.size().x,
            top + (first - duration.size().y) / 2.0,
        );
        painter.galley(at, duration, tokens.text_muted);
    }
    if let Some(step) = step {
        painter.galley(
            pos2(rect.min.x + text_x, top + name_height + 1.0),
            step,
            tokens.error_text(),
        );
    }
    response
}

fn link(ui: &mut Ui, tokens: &Tokens, text: &str, name: &str) -> Response {
    let ink = tokens.accent_ink();
    let g = galley(ui, type_style::HELP, text, ink);
    let size = g.size() + Vec2::new(12.0, 8.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Link, true, name));
    light(ui, &response, rect, tokens);
    ui.painter().galley(rect.min + Vec2::new(6.0, 4.0), g, ink);
    response
}

fn recent_chip(ui: &mut Ui, tokens: &Tokens, run: &RecentRun) -> Response {
    let ink = tokens.status(run.tone);
    let text = match &run.branch {
        Some(branch) => format!("{} · {branch}", run.when),
        None => run.when.clone(),
    };
    let g = galley(ui, type_style::META, &text, tokens.text);
    let size = Vec2::new(8.0 + ROW_ICON + 5.0 + g.size().x + 9.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Link, true, &run.accessible_name));
    let painter = ui.painter();
    let fill = if response.hovered() {
        tokens.bg_hover
    } else {
        tokens.bg
    };
    painter.rect_filled(rect, CornerRadius::same(radius::R8), fill);
    if run.current {
        painter.rect_stroke(
            rect,
            CornerRadius::same(radius::R8),
            Stroke::new(1.0, tokens.chosen_ring()),
            egui::StrokeKind::Inside,
        );
    }
    paint_icon(
        painter,
        pos2(rect.min.x + 8.0 + ROW_ICON / 2.0, rect.center().y),
        ROW_ICON,
        run.icon,
        ink.glyph,
        fill,
        0,
    );
    painter.galley(
        pos2(
            rect.min.x + 8.0 + ROW_ICON + 5.0,
            rect.center().y - g.size().y / 2.0,
        ),
        g,
        tokens.text,
    );
    if response.has_focus() {
        focus_ring(painter, rect, radius::R8, tokens);
    }
    response
}

fn panel_button(
    ui: &mut Ui,
    tokens: &Tokens,
    kind: ButtonKind,
    spec: &PanelButton,
    focus: &mut bool,
) -> Option<PanelAction> {
    let response = button(ui, tokens, kind, &spec.label, spec.enabled, true);
    if std::mem::take(focus) {
        response.request_focus();
    }
    response.clicked().then(|| spec.action.clone())
}

fn actions(
    ui: &mut Ui,
    model: &DetailsModel,
    tokens: &Tokens,
    state: &mut PanelState,
    action: &mut Option<PanelAction>,
) {
    ui.add_space(6.0);
    let rule = ui.available_rect_before_wrap();
    ui.painter().hline(
        rule.min.x..=rule.max.x,
        rule.min.y,
        Stroke::new(1.0, tokens.line),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
        if let Some(chosen) = panel_button(
            ui,
            tokens,
            ButtonKind::Primary,
            &model.primary,
            &mut state.focus_primary,
        ) {
            *action = Some(chosen);
        }
        ui.add_space(2.0);
        for spec in &model.secondary {
            let mut no_focus = false;
            let label = if state.copied && matches!(spec.action, PanelAction::CopyLink(_)) {
                PanelButton {
                    label: "Link copied".to_owned(),
                    ..spec.clone()
                }
            } else {
                spec.clone()
            };
            if let Some(chosen) = panel_button(ui, tokens, ButtonKind::Quiet, &label, &mut no_focus)
            {
                *action = Some(chosen);
            }
        }
    });
}
