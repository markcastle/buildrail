//! The 24 px strip: `Builds │ GH ✕ Keevoro · Failed │ CF ✓ Site · Deployed │ +2 · 1 failed  + ⚙`.
//!
//! The widget is drawn by hand (measured, then painted) because a 24 px
//! row has no room for egui's default widget padding. Every interactive
//! part is still a real egui widget (`ui.interact` + `widget_info`), so it
//! is keyboard focusable (Tab, Shift+Tab, arrow keys), activates on
//! Enter/Space or click, and carries an AccessKit name.
//!
//! Hover or keyboard focus reveals an entry's details. The strip window is
//! only 24 px tall, so details (and the overflow list) open in their own
//! small, non-activating viewport above the strip; in single-window test
//! harnesses egui embeds them as windows instead.

use eframe::egui::{
    self, Align2, FontId, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, ViewportBuilder,
    ViewportId, WidgetInfo,
};

use super::glyphs::{paint_gear, paint_icon, paint_plus, paint_provider_mark};
use super::theme::Palette;
use crate::presentation::{fit, provider_mark, OverflowGroup, StripEntry, StripModel};

/// Height of the strip in logical pixels (points).
pub const STRIP_HEIGHT: f32 = 24.0;

const PAD: f32 = 6.0;
const GAP: f32 = 4.0;
const ICON: f32 = 11.0;
const MARK: Vec2 = Vec2::new(19.0, 13.0);
const BUTTON: f32 = 24.0;
const SPACING: f32 = 1.0;
const TEXT_SIZE: f32 = 13.0;
const NOTE_SIZE: f32 = 11.0;

/// What the user asked for on the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripAction {
    /// Open this run page.
    Open(String),
    /// Open Settings.
    OpenSettings,
    /// Open Settings on the Add-build wizard.
    AddBuild,
    /// Open Settings to reconnect this watch.
    Reconnect(buildrail_core::domain::WatchId),
    /// The user pressed on the caption of the floating window: move it.
    StartDrag,
}

/// Per-strip UI state that survives frames.
#[derive(Debug, Default)]
pub struct StripView {
    overflow_open: bool,
    /// Animate the running spinner (off in tests so frames settle).
    pub animate: bool,
}

struct Measured {
    mark: String,
    text: std::sync::Arc<egui::Galley>,
    note: Option<std::sync::Arc<egui::Galley>>,
    width: f32,
}

impl StripView {
    /// A strip view with animation on.
    pub fn new() -> Self {
        Self {
            overflow_open: false,
            animate: true,
        }
    }

    /// Draws the strip in the top `STRIP_HEIGHT` points of `ui` and returns
    /// what the user asked for. `movable` makes the caption a drag handle.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        model: &StripModel,
        palette: &Palette,
        movable: bool,
    ) -> Vec<StripAction> {
        let mut actions = Vec::new();
        let full = ui.max_rect();
        let strip = Rect::from_min_size(full.min, Vec2::new(full.width(), STRIP_HEIGHT));
        let painter = ui.painter_at(strip);
        painter.rect_filled(strip, 0.0, palette.background);
        let time = ui.input(|i| i.time);
        let y = strip.center().y;

        // Leading caption (drag handle while floating).
        let caption = painter.layout_no_wrap(
            "Builds".to_owned(),
            FontId::proportional(TEXT_SIZE),
            palette.muted,
        );
        let caption_rect = Rect::from_min_size(
            Pos2::new(strip.min.x, strip.min.y),
            Vec2::new(caption.size().x + 2.0 * PAD, STRIP_HEIGHT),
        );
        let sense = if movable {
            Sense::drag()
        } else {
            Sense::hover()
        };
        let caption_response = ui.interact(caption_rect, ui.scope_id().with("caption"), sense);
        caption_response.widget_info(|| WidgetInfo::labeled(Role::Label, true, "Builds"));
        if caption_response.drag_started() {
            actions.push(StripAction::StartDrag);
        }
        painter.galley(
            Pos2::new(caption_rect.min.x + PAD, y - caption.size().y / 2.0),
            caption,
            palette.muted,
        );
        separator(&painter, caption_rect.max.x, y, palette);

        // Trailing buttons.
        let settings_rect = Rect::from_min_size(
            Pos2::new(strip.max.x - BUTTON, strip.min.y),
            Vec2::splat(BUTTON),
        );
        let right_edge = if model.is_empty() {
            settings_rect.min.x
        } else {
            let add_rect = settings_rect.translate(Vec2::new(-BUTTON, 0.0));
            if icon_button(ui, add_rect, "Add build", palette, |p, c| {
                paint_plus(p, c, 10.0, palette.text);
            }) {
                actions.push(StripAction::AddBuild);
            }
            add_rect.min.x
        };
        if icon_button(ui, settings_rect, "Settings", palette, |p, c| {
            paint_gear(p, c, 13.0, palette.text);
        }) {
            actions.push(StripAction::OpenSettings);
        }

        let left = caption_rect.max.x + SPACING;
        if model.is_empty() {
            self.empty_state(ui, left, strip, palette, &mut actions);
            return actions;
        }

        // Measure, fit, draw.
        let measured: Vec<Measured> = model
            .entries
            .iter()
            .map(|entry| measure(&painter, entry, palette))
            .collect();
        let widths: Vec<f32> = measured.iter().map(|m| m.width).collect();
        let overflow_probe = OverflowGroup {
            hidden: model.entries.len(),
            failures: model.failure_count(),
        };
        let overflow_width = painter
            .layout_no_wrap(
                overflow_probe.text(),
                FontId::proportional(TEXT_SIZE),
                palette.text,
            )
            .size()
            .x
            + 2.0 * PAD;
        let available = (right_edge - left - GAP).max(0.0);
        let layout = fit(&model.entries, &widths, SPACING, available, overflow_width);

        let mut x = left;
        let mut animating = false;
        for (index, (entry, m)) in model.entries.iter().zip(&measured).enumerate() {
            if index >= layout.visible {
                break;
            }
            if index > 0 {
                separator(&painter, x, y, palette);
                x += SPACING;
            }
            let rect =
                Rect::from_min_size(Pos2::new(x, strip.min.y), Vec2::new(m.width, STRIP_HEIGHT));
            animating |= entry.icon.is_animated();
            if let Some(action) = self.entry(ui, rect, entry, m, palette, time) {
                actions.push(action);
            }
            x += m.width;
        }

        if let Some(group) = &layout.overflow {
            if layout.visible > 0 {
                separator(&painter, x, y, palette);
                x += SPACING;
            }
            let rect = Rect::from_min_size(
                Pos2::new(x, strip.min.y),
                Vec2::new(overflow_width, STRIP_HEIGHT),
            );
            let response = ui.interact(rect, ui.scope_id().with("overflow"), Sense::click());
            let name = group.accessible_name();
            response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
            highlight(&painter, &response, rect, palette);
            let color = if group.failures > 0 {
                palette.red
            } else {
                palette.text
            };
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                group.text(),
                FontId::proportional(TEXT_SIZE),
                color,
            );
            if response.clicked() {
                self.overflow_open = !self.overflow_open;
            }
            if self.overflow_open {
                let hidden = &model.entries[layout.visible..];
                if let Some(action) =
                    overflow_list(ui.ctx(), hidden, palette, &mut self.overflow_open)
                {
                    actions.push(action);
                }
            }
        } else {
            self.overflow_open = false;
        }

        if animating && self.animate {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(60));
        }
        actions
    }

    fn empty_state(
        &mut self,
        ui: &mut Ui,
        left: f32,
        strip: Rect,
        palette: &Palette,
        actions: &mut Vec<StripAction>,
    ) {
        let painter = ui.painter_at(strip);
        let text = painter.layout_no_wrap(
            "Add build".to_owned(),
            FontId::proportional(TEXT_SIZE),
            palette.text,
        );
        let width = text.size().x + 2.0 * PAD + 14.0;
        let rect = Rect::from_min_size(
            Pos2::new(left + GAP, strip.min.y),
            Vec2::new(width, STRIP_HEIGHT),
        );
        let response = ui.interact(rect, ui.scope_id().with("add-build"), Sense::click());
        response.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Add build"));
        highlight(&painter, &response, rect, palette);
        paint_plus(
            &painter,
            Pos2::new(rect.min.x + PAD + 4.0, rect.center().y),
            9.0,
            palette.text,
        );
        painter.galley(
            Pos2::new(
                rect.min.x + PAD + 14.0,
                rect.center().y - text.size().y / 2.0,
            ),
            text,
            palette.text,
        );
        let hint = painter.text(
            Pos2::new(rect.max.x + GAP * 2.0, rect.center().y),
            Align2::LEFT_CENTER,
            "No builds pinned yet",
            FontId::proportional(NOTE_SIZE),
            palette.muted,
        );
        ui.interact(hint, ui.scope_id().with("empty-hint"), Sense::hover())
            .widget_info(|| WidgetInfo::labeled(Role::Label, true, "No builds pinned yet"));
        if response.clicked() {
            actions.push(StripAction::AddBuild);
        }
    }

    fn entry(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        entry: &StripEntry,
        m: &Measured,
        palette: &Palette,
        time: f64,
    ) -> Option<StripAction> {
        let id = ui.scope_id().with(("entry", entry.watch_id.as_str()));
        let response = ui.interact(rect, id, Sense::click());
        let name = entry.accessible_name.clone();
        response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
        let painter = ui.painter_at(rect);
        highlight(&painter, &response, rect, palette);

        let y = rect.center().y;
        let mut x = rect.min.x + PAD;
        let mark_rect = Rect::from_center_size(Pos2::new(x + MARK.x / 2.0, y), MARK);
        paint_provider_mark(&painter, mark_rect, &m.mark, palette.muted);
        x += MARK.x + GAP;
        let color = palette.tone(entry.tone);
        paint_icon(
            &painter,
            Pos2::new(x + ICON / 2.0, y),
            ICON,
            entry.icon,
            color,
            if self.animate { time } else { 0.0 },
        );
        x += ICON + GAP;
        painter.galley(
            Pos2::new(x, y - m.text.size().y / 2.0),
            m.text.clone(),
            palette.text,
        );
        x += m.text.size().x;
        if let Some(note) = &m.note {
            x += GAP;
            painter.galley(
                Pos2::new(x, y - note.size().y / 2.0),
                note.clone(),
                palette.muted,
            );
        }

        if response.hovered() || response.has_focus() {
            details(ui.ctx(), entry, rect, palette);
        }
        if response.clicked() {
            return match (&entry.url, entry.needs_reconnect) {
                (_, true) => Some(StripAction::Reconnect(entry.watch_id.clone())),
                (Some(url), false) => Some(StripAction::Open(url.clone())),
                (None, false) => None,
            };
        }
        None
    }
}

/// Label in the text colour, status word in its tone colour.
fn measure(painter: &egui::Painter, entry: &StripEntry, palette: &Palette) -> Measured {
    let mut job = egui::text::LayoutJob::default();
    let font = FontId::proportional(TEXT_SIZE);
    job.append(
        &format!("{} · ", entry.label),
        0.0,
        egui::TextFormat::simple(font.clone(), palette.text),
    );
    job.append(
        &entry.status_text,
        0.0,
        egui::TextFormat::simple(font, palette.tone(entry.tone)),
    );
    let text = painter.layout_job(job);
    // A note equal to the status ("No runs yet") is not repeated.
    let note = entry
        .note
        .as_ref()
        .filter(|note| **note != entry.status_text)
        .map(|note| {
            painter.layout_no_wrap(note.clone(), FontId::proportional(NOTE_SIZE), palette.muted)
        });
    let width = PAD
        + MARK.x
        + GAP
        + ICON
        + GAP
        + text.size().x
        + note.as_ref().map_or(0.0, |n| GAP + n.size().x)
        + PAD;
    Measured {
        mark: provider_mark(entry.provider).to_owned(),
        text,
        note,
        width: width.ceil(),
    }
}

fn separator(painter: &egui::Painter, x: f32, y: f32, palette: &Palette) {
    painter.vline(
        x + 0.5,
        (y - 7.0)..=(y + 7.0),
        Stroke::new(1.0, palette.separator),
    );
}

fn highlight(painter: &egui::Painter, response: &egui::Response, rect: Rect, palette: &Palette) {
    if response.hovered() || response.has_focus() {
        painter.rect_filled(rect.shrink(1.0), 3.0, palette.highlight);
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect.shrink(1.5),
            3.0,
            Stroke::new(1.5, palette.focus),
            StrokeKind::Inside,
        );
    }
}

fn icon_button(
    ui: &mut Ui,
    rect: Rect,
    name: &str,
    palette: &Palette,
    paint: impl FnOnce(&egui::Painter, Pos2),
) -> bool {
    let response = ui.interact(rect, ui.scope_id().with(name), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, name));
    let painter = ui.painter_at(rect);
    highlight(&painter, &response, rect, palette);
    paint(&painter, rect.center());
    response.clicked()
}

/// Where a popup viewport of `size` should go: above the strip, or below
/// it when the strip is at the top of the screen.
fn popup_position(ctx: &egui::Context, anchor: Rect, size: Vec2) -> Pos2 {
    let (outer, monitor) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().monitor_size));
    let origin = outer.map_or(Pos2::ZERO, |r| r.min);
    let x = origin.x + anchor.min.x;
    let above = origin.y - size.y - 4.0;
    let y = if above < 0.0 {
        origin.y + STRIP_HEIGHT + 4.0
    } else {
        above
    };
    let x = monitor.map_or(x, |m| x.min(m.x - size.x).max(0.0));
    Pos2::new(x, y)
}

fn popup_builder(title: &str, position: Pos2, size: Vec2) -> ViewportBuilder {
    ViewportBuilder::default()
        .with_title(title)
        .with_decorations(false)
        .with_resizable(false)
        .with_taskbar(false)
        .with_active(false)
        .with_always_on_top()
        .with_position(position)
        .with_inner_size(size)
}

/// The hover / focus detail popup for one entry.
fn details(ctx: &egui::Context, entry: &StripEntry, anchor: Rect, palette: &Palette) {
    let size = Vec2::new(330.0, 30.0 + 18.0 * entry.details.len() as f32);
    let position = popup_position(ctx, anchor, size);
    let builder = popup_builder(&format!("{} details", entry.label), position, size)
        .with_mouse_passthrough(true);
    let palette = *palette;
    ctx.show_viewport_immediate(
        ViewportId::from_hash_of("buildrail-details"),
        builder,
        |ui, _| {
            egui::Frame::new()
                .fill(palette.background)
                .stroke(Stroke::new(1.0, palette.separator))
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(entry.text())
                            .color(palette.tone(entry.tone))
                            .strong(),
                    );
                    for line in &entry.details {
                        ui.label(egui::RichText::new(line).color(palette.text));
                    }
                });
        },
    );
}

/// The overflow list: hidden entries as buttons.
fn overflow_list(
    ctx: &egui::Context,
    hidden: &[StripEntry],
    palette: &Palette,
    open: &mut bool,
) -> Option<StripAction> {
    let size = Vec2::new(360.0, 40.0 + 26.0 * hidden.len() as f32);
    let anchor = ctx
        .input(|i| i.viewport().inner_rect)
        .map_or(Rect::ZERO, |r| {
            Rect::from_min_size(Pos2::new(r.width() - size.x, 0.0), Vec2::ZERO)
        });
    let position = popup_position(ctx, anchor, size);
    let builder = popup_builder("More builds", position, size);
    let palette = *palette;
    let mut action = None;
    ctx.show_viewport_immediate(
        ViewportId::from_hash_of("buildrail-overflow"),
        builder,
        |ui, _| {
            if ui.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                *open = false;
            }
            egui::Frame::new()
                .fill(palette.background)
                .stroke(Stroke::new(1.0, palette.separator))
                .inner_margin(8.0)
                .show(ui, |ui| {
                    for entry in hidden {
                        let label =
                            egui::RichText::new(entry.text()).color(palette.tone(entry.tone));
                        let response = ui
                            .add(egui::Button::new(label).frame(false))
                            .on_hover_text(entry.details.join("\n"));
                        let name = entry.accessible_name.clone();
                        response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
                        if response.clicked() {
                            action = match (&entry.url, entry.needs_reconnect) {
                                (_, true) => Some(StripAction::Reconnect(entry.watch_id.clone())),
                                (Some(url), false) => Some(StripAction::Open(url.clone())),
                                (None, false) => None,
                            };
                            *open = false;
                        }
                    }
                });
        },
    );
    action
}
