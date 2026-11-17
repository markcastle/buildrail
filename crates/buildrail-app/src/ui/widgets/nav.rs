//! An item of the Settings navigation, mirroring Task Rail's
//! `SettingsNav`: 9 × 12 padding, radius 9, `bg-hover` under the pointer;
//! the current page is filled with `progress` mixed into `bg-raised` and
//! set at 600. "Quit" is the only red item.

use eframe::egui::{CornerRadius, Response, Role, Sense, Ui, Vec2, WidgetInfo};

use super::{focus_ring, text};
use crate::ui::theme::{radius, type_style, Tokens, TypeStyle, Weight};

/// What kind of navigation item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavItem {
    /// A page; `true` when it is the current one.
    Page(bool),
    /// Closes Settings ("Done"), set at 600.
    Close,
    /// Quits the app, in `priority`.
    Danger,
}

/// Draws one navigation item, as wide as the column.
pub fn nav_item(ui: &mut Ui, tokens: &Tokens, name: &str, kind: NavItem) -> Response {
    let current = kind == NavItem::Page(true);
    let style = TypeStyle {
        weight: if current || kind == NavItem::Close {
            Weight::Semibold
        } else {
            Weight::Medium
        },
        ..type_style::LABEL
    };
    let ink = match kind {
        NavItem::Danger => tokens.priority,
        _ => tokens.text,
    };
    let galley = text(ui, style, name, ink);
    let pad = Vec2::new(12.0, 9.0);
    let size = Vec2::new(ui.available_width(), galley.size().y + 2.0 * pad.y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let role = match kind {
        NavItem::Page(_) => Role::Tab,
        _ => Role::Button,
    };
    response.widget_info(|| match kind {
        NavItem::Page(on) => WidgetInfo::selected(role, true, on, name),
        _ => WidgetInfo::labeled(role, true, name),
    });
    let r9 = CornerRadius::same(radius::R9);
    let painter = ui.painter();
    if current {
        painter.rect_filled(rect, r9, tokens.nav_current());
    } else if response.hovered() {
        let fill = if kind == NavItem::Danger {
            tokens.danger_hover()
        } else {
            tokens.bg_hover
        };
        painter.rect_filled(rect, r9, fill);
    }
    painter.galley(rect.min + pad, galley, ink);
    if response.has_focus() {
        focus_ring(painter, rect.shrink(2.0), radius::R9 - 2, tokens);
    }
    response
}
