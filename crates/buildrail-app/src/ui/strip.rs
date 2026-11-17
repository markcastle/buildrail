//! The 24 px strip: `Builds │ ✕ Keevoro · Failed  ✓ Site · Deployed  +2 · 1 failed   + ⚙`.
//!
//! Drawn in the Task Rail manner: a flat `bg` ground with a 1 px `line` on
//! the edge facing the desktop (the top edge, or the bottom edge when the
//! strip is docked at the top of the screen), a quiet "Builds" caption, then one entry per pinned build
//! (provider mark, status glyph, label at 500 in `text`, `·`, the status
//! word in its state colour, and a muted note when the data is not
//! current). The pointer lights an entry with a `bg-hover` pill (radius
//! 6); keyboard focus adds the 2 px `accent` ring. Add and Settings are
//! icon-button wells (`bg-deep`) at the right end.
//!
//! The widget is drawn by hand (measured, then painted) because a 24 px
//! row has no room for egui's default widget padding. Every interactive
//! part is still a real egui widget (`ui.interact` + `widget_info`), so it
//! is keyboard focusable (Tab, Shift+Tab, arrow keys), activates on
//! Enter/Space or click, and carries an AccessKit name.
//!
//! When a new BuildRail release is available (and updating automatically
//! is off), one quiet accent "Update" chip sits before the wells.
//! Activating it (click, Enter or Space) opens Settings on About, where
//! Install now and What's new are; a stray click never installs. Hover or
//! focus shows a small card with
//! **Install now**, **What's new** (the release page) and **Not now**. It
//! takes part in the overflow width computation like the wells, so it
//! only pushes builds into the overflow when they would not fit beside
//! it.
//!
//! Hover or keyboard focus reveals an entry's details. The strip window is
//! only 24 px tall, so details (and the overflow list) open in their own
//! small, non-activating viewport, away from the screen edge the strip is
//! docked to; in single-window test harnesses egui embeds them as windows
//! instead. Where a pop-up goes is worked out in physical pixels from the
//! strip window's real rect and the monitor it is on
//! ([`popup_rect`](crate::presentation::popup_rect)); the pop-up window is
//! created hidden, moved until it is really there (its first move may land
//! it on a monitor with another scale), and only then shown
//! ([`PopupPlacement`]). A popup viewport exists only while it is shown
//! (egui closes an immediate viewport the first frame it is not drawn), and
//! the strip never asks for frames of its own beyond the one or two that
//! place a pop-up: when to paint next is decided by
//! [`RepaintPlanner`](crate::presentation::RepaintPlanner).

use std::sync::Arc;

use eframe::egui::{
    self, pos2, CornerRadius, Frame, Galley, Label, Margin, Pos2, Rect, Role, Sense, Stroke,
    StrokeKind, Ui, Vec2, ViewportBuilder, ViewportId, WidgetInfo, WidgetText,
};

use buildrail_platform::geometry::{MonitorInfo, PhysicalRect};

use super::glyphs::{paint_gear, paint_icon, paint_plus, paint_provider_mark};
use super::theme::{radius, type_style, Tokens, TypeStyle};
use super::widgets::{self, button, icon_button_at, ButtonKind};
use crate::presentation::{
    fit, popup_rect, provider_mark, OverflowGroup, PopupAlign, PopupPlacement, PopupRequest,
    PopupStep, StripEntry, StripModel, StripSide, UpdateChip,
};

/// Height of the strip in logical pixels (points).
pub const STRIP_HEIGHT: f32 = 24.0;

/// Left padding before the caption.
const EDGE: f32 = 10.0;
/// Horizontal padding inside an entry's pill.
const PAD: f32 = 8.0;
/// Space between an entry's parts.
const GAP: f32 = 6.0;
/// Space between entries.
const SPACING: f32 = 2.0;
/// Status glyph size.
const ICON: f32 = 13.0;
/// Provider mark size.
const MARK: Vec2 = Vec2::new(17.0, 13.0);
/// Icon-button well size and the gap between wells.
const WELL: f32 = 20.0;
const WELL_GAP: f32 = 4.0;
/// The pill's inset from the strip's top and bottom.
const PILL_INSET: f32 = 2.0;

/// The detail and overflow cards' width, padding and outer margin (room
/// for the card shadow inside the popup window).
const CARD_WIDTH: f32 = 300.0;
const CARD_PAD: Vec2 = Vec2::new(14.0, 12.0);
const CARD_MARGIN: f32 = 10.0;

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
    /// Install the available update now (the card's Install now button).
    InstallUpdate,
    /// Open Settings on About, where the update can be installed (the
    /// chip itself).
    OpenUpdates,
    /// Open the update's release page ("What's new").
    UpdateNotes(String),
    /// "Not now": hide the update offer until the next start.
    DismissUpdate,
}

/// Per-strip UI state that survives frames.
#[derive(Debug, Default)]
pub struct StripView {
    overflow_open: bool,
    update: Option<UpdateChip>,
    update_card: UpdateCardState,
    surroundings: Surroundings,
    popups: Popups,
}

/// Where the strip sits, for its pop-ups and its edge line.
#[derive(Debug, Clone, Default, PartialEq)]
struct Surroundings {
    side: StripSide,
    monitors: Vec<MonitorInfo>,
}

/// One pop-up window's placement, and whether it was drawn this frame.
#[derive(Debug, Default)]
struct PopupSlot {
    placement: PopupPlacement,
    drawn: bool,
}

impl PopupSlot {
    /// Marks the slot drawn this frame and returns its placement.
    fn use_now(&mut self) -> &mut PopupPlacement {
        self.drawn = true;
        &mut self.placement
    }
}

/// The three pop-up windows.
#[derive(Debug, Default)]
struct Popups {
    details: PopupSlot,
    update: PopupSlot,
    overflow: PopupSlot,
}

impl Popups {
    fn slots(&mut self) -> [&mut PopupSlot; 3] {
        [&mut self.details, &mut self.update, &mut self.overflow]
    }

    fn begin_frame(&mut self) {
        for slot in self.slots() {
            slot.drawn = false;
        }
    }

    /// A pop-up not drawn this frame has closed: the next one starts
    /// hidden and unplaced.
    fn end_frame(&mut self) {
        for slot in self.slots() {
            if !slot.drawn {
                slot.placement = PopupPlacement::new();
            }
        }
    }
}

/// Whether the update card is open and why.
#[derive(Debug, Default, Clone, Copy)]
struct UpdateCardState {
    open: bool,
    /// Opened by keyboard focus (closes when focus moves on).
    by_focus: bool,
    /// The pointer was over the card last frame (closes when it leaves).
    had_pointer: bool,
}

struct Measured {
    mark: String,
    label: Arc<Galley>,
    dot: Arc<Galley>,
    status: Arc<Galley>,
    note: Option<Arc<Galley>>,
    width: f32,
}

fn galley(ui: &Ui, style: TypeStyle, text: &str, color: egui::Color32) -> Arc<Galley> {
    widgets::text(ui, style, text, color)
}

fn action_for(entry: &StripEntry) -> Option<StripAction> {
    match (&entry.url, entry.needs_reconnect) {
        (_, true) => Some(StripAction::Reconnect(entry.watch_id.clone())),
        (Some(url), false) => Some(StripAction::Open(url.clone())),
        (None, false) => None,
    }
}

impl StripView {
    /// A strip view with nothing open.
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` while the overflow list is open.
    pub fn is_overflow_open(&self) -> bool {
        self.overflow_open
    }

    /// Sets the update offer to show (`None`: no chip).
    pub fn set_update(&mut self, chip: Option<UpdateChip>) {
        if chip.is_none() {
            self.update_card = UpdateCardState::default();
        }
        self.update = chip;
    }

    /// `true` while the update card is open.
    pub fn is_update_card_open(&self) -> bool {
        self.update_card.open
    }

    /// Where the strip sits (docked top, docked bottom or floating) and the
    /// connected monitors: pop-ups open away from the screen edge, on the
    /// strip's own monitor, and the edge line faces the desktop.
    pub fn set_surroundings(&mut self, side: StripSide, monitors: Vec<MonitorInfo>) {
        self.surroundings = Surroundings { side, monitors };
    }

    /// How the strip sits, as last set.
    pub fn side(&self) -> StripSide {
        self.surroundings.side
    }

    /// Draws the strip in the top `STRIP_HEIGHT` points of `ui` and returns
    /// what the user asked for. `movable` makes the caption a drag handle;
    /// `phase` is the in-progress glyph's animation step (`0` = at rest).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        model: &StripModel,
        tokens: &Tokens,
        movable: bool,
        phase: u32,
    ) -> Vec<StripAction> {
        self.popups.begin_frame();
        let actions = self.show_strip(ui, model, tokens, movable, phase);
        self.popups.end_frame();
        actions
    }

    fn show_strip(
        &mut self,
        ui: &mut Ui,
        model: &StripModel,
        tokens: &Tokens,
        movable: bool,
        phase: u32,
    ) -> Vec<StripAction> {
        let mut actions = Vec::new();
        let full = ui.max_rect();
        let strip = Rect::from_min_size(full.min, Vec2::new(full.width(), STRIP_HEIGHT));
        let painter = ui.painter_at(strip);
        painter.rect_filled(strip, 0.0, tokens.rail_gradient);
        // The line faces the desktop: under a strip docked at the top.
        let line_y = match self.surroundings.side {
            StripSide::Top => strip.max.y - 0.5,
            StripSide::Bottom | StripSide::Floating => strip.min.y + 0.5,
        };
        painter.hline(
            strip.min.x..=strip.max.x,
            line_y,
            Stroke::new(1.0, tokens.line),
        );
        let y = strip.center().y;

        // Leading caption (drag handle while floating).
        let caption = galley(
            ui,
            type_style::STRIP_CAPTION,
            "Builds",
            tokens.strip_muted(),
        );
        let caption_rect = Rect::from_min_size(
            strip.min,
            Vec2::new(EDGE + caption.size().x + 10.0, STRIP_HEIGHT),
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
            pos2(strip.min.x + EDGE, y - caption.size().y / 2.0),
            caption,
            tokens.strip_muted(),
        );
        // A short rule between the caption and the builds.
        painter.vline(
            caption_rect.max.x - 0.5,
            (y - 6.0)..=(y + 6.0),
            Stroke::new(1.0, tokens.line),
        );

        // Trailing icon buttons: Add build, Settings.
        let well =
            |right: f32| Rect::from_center_size(pos2(right - WELL / 2.0, y), Vec2::splat(WELL));
        let settings_rect = well(strip.max.x - PILL_INSET);
        let mut right_edge = if model.is_empty() {
            settings_rect.min.x
        } else {
            let add_rect = well(settings_rect.min.x - WELL_GAP);
            let response = icon_button_at(
                ui,
                add_rect,
                tokens,
                "Add build",
                "Add a build to the strip",
                &|p, c, s, color| paint_plus(p, c, s, color),
            );
            if response.clicked() {
                actions.push(StripAction::AddBuild);
            }
            add_rect.min.x
        };
        let response = icon_button_at(
            ui,
            settings_rect,
            tokens,
            "Settings",
            "Open BuildRail settings",
            &|p, c, s, color| paint_gear(p, c, s, color),
        );
        if response.clicked() {
            actions.push(StripAction::OpenSettings);
        }

        // The update offer, before the wells; the builds fit beside it.
        if let Some(chip) = self.update.clone() {
            let label = galley(
                ui,
                type_style::STRIP_LABEL,
                chip.label(),
                tokens.accent_ink(),
            );
            let width = (PAD + label.size().x + PAD).ceil();
            let rect = Rect::from_min_size(
                pos2(right_edge - WELL_GAP - width, strip.min.y),
                Vec2::new(width, STRIP_HEIGHT),
            );
            if let Some(action) = self.update_chip(ui, rect, strip, &chip, label, tokens) {
                actions.push(action);
            }
            right_edge = rect.min.x;
        }

        let left = caption_rect.max.x + 4.0;
        if model.is_empty() {
            self.empty_state(ui, left, strip, tokens, &mut actions);
            return actions;
        }

        // Measure, fit, draw.
        let measured: Vec<Measured> = model
            .entries
            .iter()
            .map(|entry| measure(ui, entry, tokens))
            .collect();
        let widths: Vec<f32> = measured.iter().map(|m| m.width).collect();
        let overflow_probe = OverflowGroup {
            hidden: model.entries.len(),
            failures: model.failure_count(),
        };
        let overflow_width = overflow_galleys(ui, &overflow_probe, tokens).2;
        let available = (right_edge - left - 6.0).max(0.0);
        let layout = fit(&model.entries, &widths, SPACING, available, overflow_width);

        let mut x = left;
        for (index, (entry, m)) in model.entries.iter().zip(&measured).enumerate() {
            if index >= layout.visible {
                break;
            }
            if index > 0 {
                x += SPACING;
            }
            let rect = Rect::from_min_size(pos2(x, strip.min.y), Vec2::new(m.width, STRIP_HEIGHT));
            if let Some(action) = self.entry(ui, rect, entry, m, tokens, phase) {
                actions.push(action);
            }
            x += m.width;
        }

        if let Some(group) = &layout.overflow {
            if layout.visible > 0 {
                x += SPACING;
            }
            let rect = Rect::from_min_size(
                pos2(x, strip.min.y),
                Vec2::new(overflow_width, STRIP_HEIGHT),
            );
            let response = ui.interact(rect, ui.scope_id().with("overflow"), Sense::click());
            let name = group.accessible_name();
            response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
            let painter = ui.painter_at(strip);
            pill(&painter, &response, rect, tokens);
            let (count, failed, _) = overflow_galleys(ui, group, tokens);
            let mut at = rect.min.x + PAD;
            let count_width = count.size().x;
            painter.galley(pos2(at, y - count.size().y / 2.0), count, tokens.text);
            at += count_width;
            if let Some(failed) = failed {
                painter.galley(pos2(at, y - failed.size().y / 2.0), failed, tokens.text);
            }
            if response.clicked() {
                self.overflow_open = !self.overflow_open;
            }
            if self.overflow_open {
                let hidden = &model.entries[layout.visible..];
                let host = PopupHost {
                    surroundings: &self.surroundings,
                    slot: &mut self.popups.overflow,
                };
                if let Some(action) =
                    overflow_list(ui.ctx(), hidden, tokens, &mut self.overflow_open, host)
                {
                    actions.push(action);
                }
            }
        } else {
            self.overflow_open = false;
        }
        actions
    }

    /// The "Update" chip: accent words on `accent-soft`, a pill like an
    /// entry's. Returns what was asked for: the chip opens Settings on
    /// About; only the card's labelled Install now button installs.
    fn update_chip(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        strip: Rect,
        chip: &UpdateChip,
        label: Arc<Galley>,
        tokens: &Tokens,
    ) -> Option<StripAction> {
        let response = ui.interact(rect, ui.scope_id().with("update-chip"), Sense::click());
        let name = chip.accessible_name();
        response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
        let painter = ui.painter_at(strip);
        let pill = rect.shrink2(Vec2::new(0.0, PILL_INSET));
        let lit = response.hovered() || response.has_focus();
        let fill = if lit {
            tokens.accent_soft_hover()
        } else {
            tokens.accent_soft
        };
        painter.rect_filled(pill, CornerRadius::same(radius::R6), fill);
        if response.has_focus() {
            painter.rect_stroke(
                pill,
                CornerRadius::same(radius::R6),
                Stroke::new(2.0, tokens.accent),
                StrokeKind::Inside,
            );
        }
        let y = rect.center().y;
        painter.galley(
            pos2(rect.min.x + PAD, y - label.size().y / 2.0),
            label,
            tokens.accent_ink(),
        );

        // The card opens on hover or focus and stays while the pointer
        // moves into it; it closes when the pointer goes elsewhere on the
        // strip or leaves the card, when focus moves on, or on Escape.
        let card = &mut self.update_card;
        let pointer_on_strip = ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|p| strip.contains(p));
        if lit {
            if !card.open {
                card.by_focus = response.has_focus() && !response.hovered();
            }
            card.open = true;
        } else if pointer_on_strip || (card.by_focus && !card.had_pointer) {
            card.open = false;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            card.open = false;
        }
        let mut action = response.clicked().then_some(StripAction::OpenUpdates);
        if card.open {
            let host = PopupHost {
                surroundings: &self.surroundings,
                slot: &mut self.popups.update,
            };
            if let Some(chosen) = update_popup(ui.ctx(), chip, rect, tokens, card, host) {
                action = Some(chosen);
            }
        } else {
            card.had_pointer = false;
        }
        if action.is_some() {
            self.update_card = UpdateCardState::default();
        }
        action
    }

    fn empty_state(
        &mut self,
        ui: &mut Ui,
        left: f32,
        strip: Rect,
        tokens: &Tokens,
        actions: &mut Vec<StripAction>,
    ) {
        let painter = ui.painter_at(strip);
        let text = galley(ui, type_style::STRIP_LABEL, "Add build", tokens.text);
        let glyph = 12.0;
        let width = PAD + glyph + GAP + text.size().x + PAD;
        let rect = Rect::from_min_size(pos2(left, strip.min.y), Vec2::new(width, STRIP_HEIGHT));
        let response = ui.interact(rect, ui.scope_id().with("add-build"), Sense::click());
        response.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Add build"));
        pill(&painter, &response, rect, tokens);
        let y = rect.center().y;
        paint_plus(
            &painter,
            pos2(rect.min.x + PAD + glyph / 2.0, y),
            glyph,
            tokens.accent,
        );
        painter.galley(
            pos2(rect.min.x + PAD + glyph + GAP, y - text.size().y / 2.0),
            text,
            tokens.text,
        );
        let hint = galley(
            ui,
            type_style::META,
            "No builds pinned yet",
            tokens.strip_muted(),
        );
        let hint_rect =
            Rect::from_min_size(pos2(rect.max.x + GAP, y - hint.size().y / 2.0), hint.size());
        painter.galley(hint_rect.min, hint, tokens.strip_muted());
        ui.interact(hint_rect, ui.scope_id().with("empty-hint"), Sense::hover())
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
        tokens: &Tokens,
        phase: u32,
    ) -> Option<StripAction> {
        let id = ui.scope_id().with(("entry", entry.watch_id.as_str()));
        let response = ui.interact(rect, id, Sense::click());
        let name = entry.accessible_name.clone();
        response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
        let painter = ui.painter_at(rect);
        let lit = pill(&painter, &response, rect, tokens);
        let ground = if lit { tokens.bg_hover } else { tokens.bg };

        let y = rect.center().y;
        let mut x = rect.min.x + PAD;
        let mark_rect = Rect::from_center_size(pos2(x + MARK.x / 2.0, y), MARK);
        let mark_fill = if lit { tokens.bg } else { tokens.bg_deep };
        paint_provider_mark(
            &painter,
            mark_rect,
            &m.mark,
            mark_fill,
            tokens.strip_muted(),
        );
        x += MARK.x + GAP;
        let ink = tokens.status(entry.tone);
        paint_icon(
            &painter,
            pos2(x + ICON / 2.0, y),
            ICON,
            entry.icon,
            ink.glyph,
            ground,
            phase,
        );
        x += ICON + GAP;
        for part in [&m.label, &m.dot, &m.status] {
            painter.galley(pos2(x, y - part.size().y / 2.0), part.clone(), tokens.text);
            x += part.size().x;
        }
        if let Some(note) = &m.note {
            x += GAP;
            painter.galley(
                pos2(x, y - note.size().y / 2.0 + 0.5),
                note.clone(),
                tokens.text,
            );
        }

        if response.hovered() || response.has_focus() {
            let host = PopupHost {
                surroundings: &self.surroundings,
                slot: &mut self.popups.details,
            };
            details(ui.ctx(), entry, rect, tokens, host);
        }
        if response.clicked() {
            return action_for(entry);
        }
        None
    }
}

/// Lays out an entry: label in `text` at 500, `·` and a note in the strip's
/// muted colour, the status word in its state colour.
fn measure(ui: &Ui, entry: &StripEntry, tokens: &Tokens) -> Measured {
    let muted = tokens.strip_muted();
    let label = galley(ui, type_style::STRIP_LABEL, &entry.label, tokens.text);
    let dot = galley(ui, type_style::STRIP_STATUS, " · ", muted);
    let status = galley(
        ui,
        type_style::STRIP_STATUS,
        &entry.status_text,
        tokens.status(entry.tone).word,
    );
    // A note equal to the status ("No runs yet") is not repeated.
    let note = entry
        .note
        .as_ref()
        .filter(|note| **note != entry.status_text)
        .map(|note| galley(ui, type_style::META, note, muted));
    let width = PAD
        + MARK.x
        + GAP
        + ICON
        + GAP
        + label.size().x
        + dot.size().x
        + status.size().x
        + note.as_ref().map_or(0.0, |n| GAP + n.size().x)
        + PAD;
    Measured {
        mark: provider_mark(entry.provider).to_owned(),
        label,
        dot,
        status,
        note,
        width: width.ceil(),
    }
}

/// The overflow button's parts: "+3", " · 2 failed" (in the failed
/// colour), and the total width.
fn overflow_galleys(
    ui: &Ui,
    group: &OverflowGroup,
    tokens: &Tokens,
) -> (Arc<Galley>, Option<Arc<Galley>>, f32) {
    let count = galley(
        ui,
        type_style::STRIP_LABEL,
        &format!("+{}", group.hidden),
        tokens.text,
    );
    let failed = (group.failures > 0).then(|| {
        let mut job = egui::text::LayoutJob::default();
        job.append(
            " · ",
            0.0,
            type_style::STRIP_STATUS.format(tokens.strip_muted()),
        );
        job.append(
            &format!("{} failed", group.failures),
            0.0,
            type_style::STRIP_STATUS.format(tokens.status(buildrail_core::domain::Tone::Red).word),
        );
        ui.painter().layout_job(job)
    });
    let width = PAD + count.size().x + failed.as_ref().map_or(0.0, |f| f.size().x) + PAD;
    (count, failed, width.ceil())
}

/// Paints the hover pill (and the focus ring) behind an entry; returns
/// whether the pill is lit.
fn pill(painter: &egui::Painter, response: &egui::Response, rect: Rect, tokens: &Tokens) -> bool {
    let pill = rect.shrink2(Vec2::new(0.0, PILL_INSET));
    let lit = response.hovered() || response.has_focus();
    if lit {
        painter.rect_filled(pill, CornerRadius::same(radius::R6), tokens.bg_hover);
    }
    if response.has_focus() {
        // The strip has no room for the ring to stand 2 px clear, so it
        // traces the pill itself.
        painter.rect_stroke(
            pill,
            CornerRadius::same(radius::R6),
            Stroke::new(2.0, tokens.accent),
            StrokeKind::Inside,
        );
    }
    lit
}

/// What a pop-up needs from the strip: where it sits, and the pop-up
/// window's placement state.
struct PopupHost<'a> {
    surroundings: &'a Surroundings,
    slot: &'a mut PopupSlot,
}

/// A pop-up window's wanted rect, in physical pixels, and its first guess
/// in points for egui (exact when the pop-up is created on the strip's
/// monitor; [`place_popup`] corrects it otherwise).
struct PopupTarget {
    rect: PhysicalRect,
    guess: Pos2,
}

fn to_physical(value: f32) -> i32 {
    // Saturating float-to-int conversion is the intent.
    value.round() as i32
}

/// Where a pop-up of `size` points goes, with its `align` edge at
/// `anchor_x` points from the strip window's left edge. Worked out in
/// physical pixels from the strip window's real rect and the monitor it is
/// on, opening away from the screen edge the strip is docked to.
fn popup_target(
    ctx: &egui::Context,
    surroundings: &Surroundings,
    anchor_x: f32,
    align: PopupAlign,
    size: Vec2,
) -> PopupTarget {
    let ppp = ctx.pixels_per_point();
    let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
    let window = outer.or(inner).unwrap_or(Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(ctx.content_rect().width(), STRIP_HEIGHT),
    ));
    let strip = PhysicalRect::new(
        to_physical(window.min.x * ppp),
        to_physical(window.min.y * ppp),
        (window.width() * ppp).round().max(0.0) as u32,
        (window.height() * ppp).round().max(0.0) as u32,
    );
    let rect = popup_rect(&PopupRequest {
        strip,
        anchor_x: strip.x + to_physical(anchor_x * ppp),
        align,
        width: (size.x * ppp).round().max(1.0) as u32,
        height: (size.y * ppp).round().max(1.0) as u32,
        side: surroundings.side,
        monitors: &surroundings.monitors,
    });
    PopupTarget {
        rect,
        guess: Pos2::new(rect.x as f32 / ppp, rect.y as f32 / ppp),
    }
}

/// Inside a pop-up viewport: moves it until it is really at `wanted`
/// (converting with *this* viewport's own pixels-per-point, so the
/// physical position is exact whatever monitor it was created on), then
/// shows it.
fn place_popup(ctx: &egui::Context, placement: &mut PopupPlacement, wanted: &PhysicalRect) {
    let ppp = ctx.pixels_per_point();
    let actual = ctx.input(|i| i.viewport().outer_rect).map(|r| {
        PhysicalRect::new(
            to_physical(r.min.x * ppp),
            to_physical(r.min.y * ppp),
            (r.width() * ppp).round().max(0.0) as u32,
            (r.height() * ppp).round().max(0.0) as u32,
        )
    });
    match placement.step(actual, wanted) {
        PopupStep::Move => {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(
                wanted.x as f32 / ppp,
                wanted.y as f32 / ppp,
            )));
            // One more frame to see where it landed.
            ctx.request_repaint();
        }
        PopupStep::Show => ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true)),
        PopupStep::Stay => {}
    }
}

fn popup_builder(title: &str, position: Pos2, size: Vec2) -> ViewportBuilder {
    ViewportBuilder::default()
        .with_title(title)
        .with_decorations(false)
        .with_resizable(false)
        .with_taskbar(false)
        .with_active(false)
        .with_always_on_top()
        .with_transparent(true)
        // Shown by `place_popup` once it is where it belongs.
        .with_visible(false)
        .with_position(position)
        .with_inner_size(size)
}

/// The card a popup is drawn on: `bg-raised`, radius 12, a 1 px `line`
/// border and the card shadow.
fn card_frame(tokens: &Tokens) -> Frame {
    Frame::new()
        .fill(tokens.bg_raised)
        .stroke(Stroke::new(1.0, tokens.line))
        .corner_radius(CornerRadius::same(radius::R12))
        .shadow(tokens.card_shadow[1].egui())
        .inner_margin(Margin::symmetric(CARD_PAD.x as i8, CARD_PAD.y as i8))
        .outer_margin(Margin::same(CARD_MARGIN as i8))
}

fn add_text(ui: &mut Ui, style: TypeStyle, text: &str, color: egui::Color32) {
    let width = CARD_WIDTH - 2.0 * CARD_PAD.x;
    let galley = widgets::wrapped(ui, style, text, color, width);
    ui.add(Label::new(WidgetText::Galley(galley)).selectable(false));
}

/// The content of an entry's detail card: its glyph, label and status as
/// the title, the target as a meta line, one fact per line, and the hint
/// about what a click does under a thin rule.
pub fn detail_card(ui: &mut Ui, entry: &StripEntry, tokens: &Tokens) {
    card_frame(tokens).show(ui, |ui| {
        ui.set_width(CARD_WIDTH - 2.0 * CARD_PAD.x);
        ui.spacing_mut().item_spacing = Vec2::new(GAP, 3.0);
        ui.horizontal(|ui| {
            let ink = tokens.status(entry.tone);
            let (glyph, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            paint_icon(
                ui.painter(),
                glyph.center(),
                15.0,
                entry.icon,
                ink.glyph,
                tokens.bg_raised,
                0,
            );
            let mut job = type_style::LABEL.job(&entry.label, tokens.text);
            job.append(" · ", 0.0, type_style::BODY.format(tokens.text_muted));
            job.append(&entry.status_text, 0.0, type_style::LABEL.format(ink.word));
            let title = ui.painter().layout_job(job);
            ui.add(Label::new(WidgetText::Galley(title)).selectable(false));
        });
        let mut lines = entry.details.iter();
        if let Some(target) = lines.next() {
            add_text(ui, type_style::META, target, tokens.text_muted);
        }
        ui.add_space(4.0);
        for line in lines {
            add_text(ui, type_style::HELP, line, tokens.text);
        }
        if let Some(hint) = &entry.hint {
            ui.add_space(6.0);
            let rule = ui.available_rect_before_wrap();
            ui.painter().hline(
                rule.min.x..=rule.max.x,
                rule.min.y,
                Stroke::new(1.0, tokens.line),
            );
            ui.add_space(8.0);
            add_text(ui, type_style::META, hint, tokens.text_muted);
        }
    });
}

/// The popup's size for `entry`, measured from the same text.
fn detail_size(ctx: &egui::Context, entry: &StripEntry) -> Vec2 {
    let width = CARD_WIDTH - 2.0 * CARD_PAD.x;
    let height_of = |style: TypeStyle, text: &str| {
        ctx.fonts_mut(|f| f.layout_job(style.wrapped(text, egui::Color32::PLACEHOLDER, width)))
            .size()
            .y
    };
    let mut height = 2.0 * CARD_PAD.y + 16.0f32.max(height_of(type_style::LABEL, &entry.label));
    let mut lines = entry.details.iter();
    if let Some(target) = lines.next() {
        height += 3.0 + height_of(type_style::META, target);
    }
    height += 4.0;
    for line in lines {
        height += 3.0 + height_of(type_style::HELP, line);
    }
    if let Some(hint) = &entry.hint {
        height += 3.0 + 6.0 + 3.0 + 8.0 + height_of(type_style::META, hint);
    }
    Vec2::new(CARD_WIDTH, height.ceil() + 2.0) + Vec2::splat(2.0 * CARD_MARGIN)
}

/// The hover / focus detail popup for one entry.
fn details(
    ctx: &egui::Context,
    entry: &StripEntry,
    anchor: Rect,
    tokens: &Tokens,
    host: PopupHost<'_>,
) {
    if ctx.embed_viewports() {
        // One window only (a test harness): float the card under the strip.
        let at = pos2(anchor.min.x - CARD_MARGIN, anchor.max.y);
        egui::Area::new(egui::Id::unique("buildrail-details"))
            .fixed_pos(at)
            .constrain(false)
            .order(egui::Order::Tooltip)
            .interactable(false)
            .show(ctx, |ui| detail_card(ui, entry, tokens));
        return;
    }
    let size = detail_size(ctx, entry);
    let target = popup_target(
        ctx,
        host.surroundings,
        anchor.min.x - CARD_MARGIN,
        PopupAlign::Start,
        size,
    );
    let builder = popup_builder(&format!("{} details", entry.label), target.guess, size)
        .with_mouse_passthrough(true);
    let tokens = *tokens;
    let placement = host.slot.use_now();
    ctx.show_viewport_immediate(
        ViewportId::from_hash_of("buildrail-details"),
        builder,
        |ui, _| {
            place_popup(ui.ctx(), placement, &target.rect);
            detail_card(ui, entry, &tokens);
        },
    );
}

/// The content of the update card: the one line, the last install error
/// if there is one, and Install now / What's new / Not now. Returns the
/// button pressed.
pub fn update_card(ui: &mut Ui, chip: &UpdateChip, tokens: &Tokens) -> Option<StripAction> {
    let mut action = None;
    card_frame(tokens).show(ui, |ui| {
        ui.set_width(CARD_WIDTH - 2.0 * CARD_PAD.x);
        ui.spacing_mut().item_spacing = Vec2::new(GAP, 3.0);
        add_text(ui, type_style::LABEL, &chip.headline(), tokens.text);
        if let Some(error) = chip.error() {
            add_text(ui, type_style::HELP, error, tokens.error_text());
        }
        ui.add_space(UPDATE_CARD_GAP);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
            let installing = chip.installing();
            let install = if installing {
                "Installing…"
            } else {
                "Install now"
            };
            if button(ui, tokens, ButtonKind::Primary, install, !installing, true).clicked() {
                action = Some(StripAction::InstallUpdate);
            }
            if button(ui, tokens, ButtonKind::Quiet, "What's new", true, true).clicked() {
                action = Some(StripAction::UpdateNotes(chip.notes_url().to_owned()));
            }
            if button(ui, tokens, ButtonKind::Quiet, "Not now", !installing, true).clicked() {
                action = Some(StripAction::DismissUpdate);
            }
        });
    });
    action
}

/// Space between the update card's words and its buttons.
const UPDATE_CARD_GAP: f32 = 8.0;

/// The update card's popup size, measured from the same text.
fn update_card_size(ctx: &egui::Context, chip: &UpdateChip) -> Vec2 {
    let width = CARD_WIDTH - 2.0 * CARD_PAD.x;
    let height_of = |style: TypeStyle, text: &str| {
        ctx.fonts_mut(|f| f.layout_job(style.wrapped(text, egui::Color32::PLACEHOLDER, width)))
            .size()
            .y
    };
    let mut height = 2.0 * CARD_PAD.y + height_of(type_style::LABEL, &chip.headline());
    if let Some(error) = chip.error() {
        height += 3.0 + height_of(type_style::HELP, error);
    }
    // Compact buttons: 5 px above and below the label.
    height += 3.0 + UPDATE_CARD_GAP + height_of(type_style::BUTTON, "Install now") + 10.0;
    Vec2::new(CARD_WIDTH, height.ceil() + 2.0) + Vec2::splat(2.0 * CARD_MARGIN)
}

/// Shows the update card above the chip (or under the strip in a test
/// harness) and keeps `card` up to date with the pointer.
fn update_popup(
    ctx: &egui::Context,
    chip: &UpdateChip,
    anchor: Rect,
    tokens: &Tokens,
    card: &mut UpdateCardState,
    host: PopupHost<'_>,
) -> Option<StripAction> {
    fn track(over: bool, escape: bool, card: &mut UpdateCardState) {
        if (card.had_pointer && !over) || escape {
            card.open = false;
        }
        card.had_pointer = over;
    }
    let mut action = None;
    if ctx.embed_viewports() {
        let at = pos2(
            (anchor.max.x + CARD_MARGIN - CARD_WIDTH - 2.0 * CARD_MARGIN).max(0.0),
            STRIP_HEIGHT,
        );
        let shown = egui::Area::new(egui::Id::unique("buildrail-update-card"))
            .fixed_pos(at)
            .constrain(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| update_card(ui, chip, tokens));
        action = shown.inner;
        let over = ctx
            .pointer_hover_pos()
            .is_some_and(|p| shown.response.rect.contains(p));
        track(over, false, card);
        return action;
    }
    let size = update_card_size(ctx, chip);
    // Right-aligned with the chip: it sits at the strip's right end.
    let target = popup_target(
        ctx,
        host.surroundings,
        anchor.max.x + CARD_MARGIN,
        PopupAlign::End,
        size,
    );
    let builder = popup_builder("BuildRail update", target.guess, size);
    let tokens = *tokens;
    let placement = host.slot.use_now();
    ctx.show_viewport_immediate(
        ViewportId::from_hash_of("buildrail-update-card"),
        builder,
        |ui, _| {
            place_popup(ui.ctx(), placement, &target.rect);
            let (over, escape) = ui.input(|i| {
                (
                    i.pointer.hover_pos().is_some(),
                    i.key_pressed(egui::Key::Escape) || i.viewport().close_requested(),
                )
            });
            track(over, escape, card);
            action = update_card(ui, chip, &tokens);
        },
    );
    action
}

/// One hidden build in the overflow card; returns its response.
fn overflow_row(ui: &mut Ui, entry: &StripEntry, tokens: &Tokens) -> egui::Response {
    let width = CARD_WIDTH - 2.0 * CARD_PAD.x + 2.0 * PAD;
    let label = galley(ui, type_style::CARD_TITLE, &entry.label, tokens.text);
    let ink = tokens.status(entry.tone);
    let status = galley(
        ui,
        type_style::BODY,
        &format!(" · {}", entry.status_text),
        ink.word,
    );
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
    let name = entry.accessible_name.clone();
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
    let painter = ui.painter();
    if response.hovered() || response.has_focus() {
        painter.rect_filled(rect, CornerRadius::same(radius::R8), tokens.bg_hover);
    }
    if response.has_focus() {
        widgets::focus_ring(painter, rect.shrink(2.0), radius::R8 - 2, tokens);
    }
    let y = rect.center().y;
    let ground = if response.hovered() {
        tokens.bg_hover
    } else {
        tokens.bg_raised
    };
    paint_icon(
        painter,
        pos2(rect.min.x + PAD + ICON / 2.0, y),
        ICON,
        entry.icon,
        ink.glyph,
        ground,
        0,
    );
    let x = rect.min.x + PAD + ICON + GAP + 2.0;
    let label_width = label.size().x;
    painter.galley(pos2(x, y - label.size().y / 2.0), label, tokens.text);
    painter.galley(
        pos2(x + label_width, y - status.size().y / 2.0),
        status,
        tokens.text,
    );
    let mut tip = entry.details.clone();
    tip.extend(entry.hint.clone());
    response.on_hover_text(tip.join("\n"))
}

/// The content of the overflow card: the hidden builds, each a button.
/// Returns the one clicked.
pub fn overflow_card<'a>(
    ui: &mut Ui,
    hidden: &'a [StripEntry],
    tokens: &Tokens,
) -> Option<&'a StripEntry> {
    let mut chosen = None;
    card_frame(tokens)
        .inner_margin(Margin::symmetric((CARD_PAD.x - PAD) as i8, 6))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let label = galley(
                ui,
                type_style::GROUP_LABEL,
                "More builds",
                tokens.text_muted,
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(PAD);
                ui.add(Label::new(WidgetText::Galley(label)).selectable(false));
            });
            ui.add_space(4.0);
            for entry in hidden {
                if overflow_row(ui, entry, tokens).clicked() {
                    chosen = Some(entry);
                }
            }
        });
    chosen
}

/// The overflow list: hidden entries as buttons.
fn overflow_list(
    ctx: &egui::Context,
    hidden: &[StripEntry],
    tokens: &Tokens,
    open: &mut bool,
    host: PopupHost<'_>,
) -> Option<StripAction> {
    if ctx.embed_viewports() {
        let width = ctx.content_rect().width();
        let at = pos2(
            (width - CARD_WIDTH - 2.0 * CARD_MARGIN).max(0.0),
            STRIP_HEIGHT,
        );
        let mut action = None;
        egui::Area::new(egui::Id::unique("buildrail-overflow"))
            .fixed_pos(at)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    *open = false;
                }
                if let Some(entry) = overflow_card(ui, hidden, tokens) {
                    action = action_for(entry);
                    *open = false;
                }
            });
        return action;
    }
    let size = Vec2::new(
        CARD_WIDTH + 2.0 * CARD_MARGIN,
        12.0 + 22.0 + 30.0 * hidden.len() as f32 + 2.0 * CARD_MARGIN + 4.0,
    );
    // At the strip's right end, as the overflow button is.
    let target = popup_target(
        ctx,
        host.surroundings,
        ctx.content_rect().width(),
        PopupAlign::End,
        size,
    );
    let builder = popup_builder("More builds", target.guess, size);
    let tokens = *tokens;
    let mut action = None;
    let placement = host.slot.use_now();
    ctx.show_viewport_immediate(
        ViewportId::from_hash_of("buildrail-overflow"),
        builder,
        |ui, _| {
            place_popup(ui.ctx(), placement, &target.rect);
            if ui.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                *open = false;
            }
            if let Some(entry) = overflow_card(ui, hidden, &tokens) {
                action = action_for(entry);
                *open = false;
            }
        },
    );
    action
}
