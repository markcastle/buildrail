//! A settings group, mirroring Task Rail's `SettingsGroup`: one raised
//! surface (`bg-raised`, radius 14, 1 px `line` border, the card shadow,
//! at most 640 px wide) holding rows separated by 1 px lines.
//!
//! A row ([`Group::row`]) is a label (14/600), an optional help sentence
//! (12.5, `text-muted`) and exactly one control on the right, padded
//! 14 × 18 with 24 px between the words and the control. A control that
//! needs the width sits under its words instead ([`Group::stack`]). A
//! group label ([`Group::label`]) starts a new run of rows under a small
//! capital heading.

use eframe::egui::{
    self, pos2, Align, CornerRadius, Frame, Label, Layout, Margin, Rect, Sense, Shape, Stroke, Ui,
    UiBuilder, Vec2, WidgetText,
};

use super::wrapped;
use crate::ui::theme::{radius, space, type_style, Tokens};

/// The widest a group grows.
pub const MAX_WIDTH: f32 = 640.0;
/// The narrowest the words of a row get before the control gives way.
const MIN_TEXT: f32 = 150.0;

/// Draws a group; `contents` adds its rows.
pub fn group<R>(ui: &mut Ui, tokens: &Tokens, contents: impl FnOnce(&mut Group<'_>) -> R) -> R {
    let width = ui.available_width().min(MAX_WIDTH);
    let shadows = [ui.painter().add(Shape::Noop), ui.painter().add(Shape::Noop)];
    let r14 = CornerRadius::same(radius::R14);
    let shown = Frame::new()
        .fill(tokens.bg_raised)
        .stroke(Stroke::new(1.0, tokens.line))
        .corner_radius(r14)
        .show(ui, |ui| {
            let width = width - 2.0;
            ui.set_width(width);
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let mut group = Group {
                ui,
                tokens,
                width,
                first: true,
            };
            contents(&mut group)
        });
    let rect = shown.response.rect;
    for (slot, layer) in shadows.into_iter().zip(tokens.card_shadow) {
        ui.painter().set(slot, layer.egui().as_shape(rect, r14));
    }
    shown.inner
}

/// The inside of a [`group`]. Each method adds one row-like element and
/// draws the 1 px line above it (except the first).
pub struct Group<'a> {
    ui: &'a mut Ui,
    tokens: &'a Tokens,
    width: f32,
    first: bool,
}

impl Group<'_> {
    /// The tokens the group paints with.
    pub fn tokens(&self) -> &Tokens {
        self.tokens
    }

    fn separator(&mut self) {
        if !self.first {
            let y = self.ui.cursor().min.y;
            let left = self.ui.max_rect().min.x;
            self.ui.painter().hline(
                left..=left + self.width,
                y + 0.5,
                Stroke::new(1.0, self.tokens.line),
            );
        }
        self.first = false;
    }

    fn put_text(ui: &mut Ui, at: egui::Pos2, galley: std::sync::Arc<egui::Galley>) {
        let rect = Rect::from_min_size(at, galley.size());
        ui.put(
            rect,
            Label::new(WidgetText::Galley(galley)).selectable(false),
        );
    }

    /// Lays out a row's words at `width`: (label, help, total height).
    fn words(
        &self,
        label: &str,
        help: Option<&str>,
        width: f32,
    ) -> (
        std::sync::Arc<egui::Galley>,
        Option<std::sync::Arc<egui::Galley>>,
        f32,
    ) {
        let t = self.tokens;
        let label = wrapped(self.ui, type_style::LABEL, label, t.text, width);
        let help = help.map(|help| wrapped(self.ui, type_style::HELP, help, t.text_muted, width));
        let height = label.size().y + help.as_ref().map_or(0.0, |h| 3.0 + h.size().y);
        (label, help, height)
    }

    fn draw_words(
        &mut self,
        top_left: egui::Pos2,
        label: std::sync::Arc<egui::Galley>,
        help: Option<std::sync::Arc<egui::Galley>>,
    ) {
        let label_height = label.size().y;
        Self::put_text(self.ui, top_left, label);
        if let Some(help) = help {
            Self::put_text(self.ui, top_left + Vec2::new(0.0, label_height + 3.0), help);
        }
    }

    /// A row: words on the left, `control` on the right, vertically centred.
    pub fn row<R>(
        &mut self,
        label: &str,
        help: Option<&str>,
        control: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.separator();
        let id = self.ui.scope_id().with(("row", label));
        // The control's size from the last pass; a new size asks egui for
        // one more pass so nothing jumps (as egui's own Grid does).
        let known: Option<Vec2> = self.ui.data(|d| d.get_temp(id));
        let control_size = known.unwrap_or(Vec2::new(0.0, 26.0));
        let inner_width = self.width - 2.0 * space::S18;
        let text_width = (inner_width - control_size.x - space::S24).max(MIN_TEXT);
        let (label_g, help_g, text_height) = self.words(label, help, text_width);
        let height = text_height.max(control_size.y) + 2.0 * space::S14;
        let (rect, _) = self
            .ui
            .allocate_exact_size(Vec2::new(self.width, height), Sense::hover());
        let inner = rect.shrink2(Vec2::new(space::S18, space::S14));
        let top = inner.center().y - text_height / 2.0;
        self.draw_words(pos2(inner.min.x, top), label_g, help_g);

        let room = (inner_width - MIN_TEXT - space::S24).max(0.0);
        let area = Rect::from_min_max(pos2(inner.max.x - room, inner.min.y), inner.max);
        let shown = self.ui.scope_builder(
            UiBuilder::new()
                .max_rect(area)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(space::S10, 0.0);
                control(ui)
            },
        );
        // Drawing inside the row moved the cursor; put it under the row.
        self.ui.advance_cursor_after_rect(rect);
        let used = shown.response.rect.size();
        if known.is_none_or(|k| (k - used).length() > 0.5) {
            self.ui.data_mut(|d| d.insert_temp(id, used));
            self.ui.ctx().request_discard("settings row size");
        }
        shown.inner
    }

    /// A row whose control needs the width: words, then `control` under
    /// them, 12 px apart.
    pub fn stack<R>(
        &mut self,
        label: &str,
        help: Option<&str>,
        control: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.separator();
        let inner_width = self.width - 2.0 * space::S18;
        let (label_g, help_g, text_height) = self.words(label, help, inner_width);
        let (rect, _) = self.ui.allocate_exact_size(
            Vec2::new(self.width, space::S14 + text_height + space::S12),
            Sense::hover(),
        );
        self.draw_words(
            rect.min + Vec2::new(space::S18, space::S14),
            label_g,
            help_g,
        );
        self.ui.advance_cursor_after_rect(rect);
        self.framed(
            Margin {
                left: space::S18 as i8,
                right: space::S18 as i8,
                top: 0,
                bottom: space::S14 as i8,
            },
            control,
        )
    }

    /// A divider label inside the group (13/600, capitals, `text-muted`).
    pub fn label(&mut self, text: &str) {
        self.separator();
        let galley = wrapped(
            self.ui,
            type_style::GROUP_LABEL,
            text,
            self.tokens.text_muted,
            self.width,
        );
        let (rect, _) = self.ui.allocate_exact_size(
            Vec2::new(self.width, 16.0 + galley.size().y + 2.0),
            Sense::hover(),
        );
        Self::put_text(self.ui, rect.min + Vec2::new(space::S18, 16.0), galley);
        self.ui.advance_cursor_after_rect(rect);
        // The rows under a label start without a line of their own.
        self.first = true;
    }

    /// Anything else (a card, a list, a sentence), with `margin` around it
    /// and the line above.
    pub fn padded<R>(&mut self, margin: Margin, contents: impl FnOnce(&mut Ui) -> R) -> R {
        self.separator();
        self.framed(margin, contents)
    }

    fn framed<R>(&mut self, margin: Margin, contents: impl FnOnce(&mut Ui) -> R) -> R {
        let width = self.width;
        Frame::new()
            .inner_margin(margin)
            .show(self.ui, |ui| {
                ui.set_width(width - margin.sum().x);
                ui.spacing_mut().item_spacing = Vec2::new(space::S8, space::S8);
                contents(ui)
            })
            .inner
    }

    /// Like [`Self::padded`] with the row padding (14 × 18).
    pub fn block<R>(&mut self, contents: impl FnOnce(&mut Ui) -> R) -> R {
        self.padded(
            Margin::symmetric(space::S18 as i8, space::S14 as i8),
            contents,
        )
    }

    /// Like [`Self::padded`] without padding (list rows draw their own).
    pub fn flush<R>(&mut self, contents: impl FnOnce(&mut Ui) -> R) -> R {
        self.separator();
        let width = self.width;
        self.ui
            .vertical(|ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                contents(ui)
            })
            .inner
    }
}
