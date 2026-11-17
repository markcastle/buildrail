//! One line (or a few) of text, mirroring Task Rail's `TextField`: a
//! `bg-deep` well, radius 9, 1 px `line` border and 8 × 11 padding, whose
//! border becomes the 2 px `accent` ring while it has focus. The
//! placeholder is a real example of what goes in, in `text-muted`; secrets
//! are masked.

use eframe::egui::{self, CornerRadius, Frame, Margin, Response, Stroke, TextEdit, Ui};

use crate::ui::theme::{radius, type_style, Tokens};

/// A text field. Build it, then [`TextField::show`] it.
pub struct TextField<'a> {
    text: &'a mut String,
    name: &'a str,
    placeholder: &'a str,
    password: bool,
    enabled: bool,
    rows: usize,
    width: Option<f32>,
    mono: bool,
}

impl<'a> TextField<'a> {
    /// A one-line field for `text`, named `name` for assistive technology.
    pub fn new(text: &'a mut String, name: &'a str) -> Self {
        Self {
            text,
            name,
            placeholder: "",
            password: false,
            enabled: true,
            rows: 1,
            width: None,
            mono: false,
        }
    }

    /// An example of what goes in, shown while the field is empty.
    pub fn placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = placeholder;
        self
    }

    /// Masks the text (a token).
    pub fn password(mut self, password: bool) -> Self {
        self.password = password;
        self
    }

    /// Greys the field out and ignores input.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// A multi-line field of `rows` lines, in the monospace face (for a
    /// profile someone pastes).
    pub fn multiline(mut self, rows: usize) -> Self {
        self.rows = rows.max(2);
        self.mono = true;
        self
    }

    /// The field's outer width (default: the system's 320 px, or the
    /// available width when narrower).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Draws the field. The response is the text edit's (changed when the
    /// text changed, focus, Enter…).
    pub fn show(self, ui: &mut Ui, tokens: &Tokens) -> Response {
        let pad = Margin::symmetric(11, 8);
        let outer = self
            .width
            .unwrap_or(320.0)
            .min(ui.available_width())
            .max(80.0);
        let inner = outer - pad.sum().x - 2.0;
        let id = ui.scope_id().with(("text-field", self.name));
        let focused = ui.memory(|m| m.has_focus(id));
        let stroke = if focused {
            Stroke::new(2.0, tokens.accent)
        } else {
            Stroke::new(1.0, tokens.line)
        };
        let font = if self.mono {
            egui::FontId::monospace(12.5)
        } else {
            type_style::BODY.font()
        };
        let hint = egui::RichText::new(self.placeholder)
            .color(tokens.text_muted)
            .font(font.clone());
        let fill = if self.enabled {
            tokens.bg_deep
        } else {
            tokens.bg_deep.gamma_multiply(0.55)
        };
        ui.vertical(|ui| {
            Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(radius::R9))
                .inner_margin(pad)
                .show(ui, |ui| {
                    // The well's padding sets the height, not egui's minimum.
                    ui.spacing_mut().interact_size.y = 0.0;
                    let edit = if self.rows > 1 {
                        TextEdit::multiline(self.text).desired_rows(self.rows)
                    } else {
                        TextEdit::singleline(self.text)
                    };
                    let edit = edit
                        .id(id)
                        .frame(Frame::NONE)
                        .hint_text(hint)
                        .password(self.password)
                        .font(font)
                        .text_color(tokens.text)
                        .desired_width(inner);
                    ui.add_enabled(self.enabled, edit)
                        .accessible_name(self.name)
                })
                .inner
        })
        .inner
    }
}
