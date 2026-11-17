//! A row of a picker list inside a group (a repository, a workflow, a
//! branch): a 14/500 title, an optional muted meta line, and a chevron, or
//! a tick when it is the chosen one. Built from `SettingsGroup`'s row
//! metrics (18 px sides); the pointer lights it with an inset `bg-hover`
//! pill, radius 8, as Task Rail lights a task row.

use eframe::egui::{CornerRadius, Response, Role, Sense, Ui, Vec2, WidgetInfo};

use super::{focus_ring, text};
use crate::ui::glyphs::{paint_chevron_right, paint_tick};
use crate::ui::theme::{radius, space, type_style, Tokens};

/// Draws a picker row. `chosen` is `Some(_)` for a single-choice list
/// (a radio button, ticked when chosen) and `None` for a list that moves
/// on (a button with a chevron). The accessible name is the title, then
/// the meta line after a comma. `divider` draws the group's 1 px line
/// above the row (every row but the first).
pub fn list_row(
    ui: &mut Ui,
    tokens: &Tokens,
    title: &str,
    meta: Option<&str>,
    chosen: Option<bool>,
    divider: bool,
) -> Response {
    let width = ui.available_width();
    let title_g = text(ui, type_style::CARD_TITLE, title, tokens.text);
    let meta_g = meta.map(|m| text(ui, type_style::META, m, tokens.text_muted));
    let text_height = title_g.size().y + meta_g.as_ref().map_or(0.0, |m| 1.0 + m.size().y);
    let height = text_height.max(20.0) + 2.0 * 11.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let name = match meta {
        Some(meta) => format!("{title}, {meta}"),
        None => title.to_owned(),
    };
    response.widget_info(|| match chosen {
        Some(on) => WidgetInfo::selected(Role::RadioButton, true, on, &name),
        None => WidgetInfo::labeled(Role::Button, true, &name),
    });
    let painter = ui.painter();
    if divider {
        painter.hline(
            rect.min.x + space::S18..=rect.max.x,
            rect.min.y + 0.5,
            eframe::egui::Stroke::new(1.0, tokens.line),
        );
    }
    let pill = rect.shrink2(Vec2::new(6.0, 3.0));
    if response.hovered() {
        painter.rect_filled(pill, CornerRadius::same(radius::R8), tokens.bg_hover);
    }
    let mut y = rect.center().y - text_height / 2.0;
    let x = rect.min.x + space::S18;
    let title_height = title_g.size().y;
    painter.galley(eframe::egui::pos2(x, y), title_g, tokens.text);
    y += title_height + 1.0;
    if let Some(meta_g) = meta_g {
        painter.galley(eframe::egui::pos2(x, y), meta_g, tokens.text_muted);
    }
    let glyph_at = eframe::egui::pos2(rect.max.x - space::S18 - 7.0, rect.center().y);
    match chosen {
        Some(true) => paint_tick(painter, glyph_at, 15.0, tokens.accent),
        Some(false) => {}
        None => paint_chevron_right(painter, glyph_at, 14.0, tokens.text_muted),
    }
    if response.has_focus() {
        focus_ring(painter, pill.shrink(2.0), radius::R8 - 2, tokens);
    }
    response
}
