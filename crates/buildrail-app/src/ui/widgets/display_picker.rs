//! Choosing a display by picture, mirroring Task Rail's `DisplayPicker`:
//! each display is a little screen in its real proportions (inside a
//! 60 px slot, so the names line up) with its name and size beneath. The
//! chosen tile takes an `accent` border and ring, and its screen shows the
//! strip as a bar on the edge it docks to.

use eframe::egui::{
    pos2, CornerRadius, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo,
};

use super::{focus_ring, text};
use crate::ui::theme::{mix, radius, type_style, Tokens};

/// One display.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayTile {
    /// Its name ("Display 1", "DELL U2720Q").
    pub name: String,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

impl DisplayTile {
    /// "2560 × 1440".
    pub fn size_text(&self) -> String {
        format!("{} × {}", self.width, self.height)
    }
}

const SLOT: f32 = 60.0;
const SCREEN_WIDTH: f32 = 96.0;
const MIN_TILE: f32 = 128.0;

/// Draws the tiles, wrapping; returns the index of the one clicked.
pub fn display_picker(
    ui: &mut Ui,
    tokens: &Tokens,
    displays: &[DisplayTile],
    chosen: Option<usize>,
) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(12.0);
        for (index, display) in displays.iter().enumerate() {
            let name = text(ui, type_style::LABEL, &display.name, tokens.text);
            let size_text = display.size_text();
            let size = text(ui, type_style::META, &size_text, tokens.text_muted);
            let width = name.size().x.max(size.size().x).max(SCREEN_WIDTH) + 28.0;
            let height = 12.0 + SLOT + 4.0 + name.size().y + 4.0 + size.size().y + 10.0;
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(width.max(MIN_TILE), height), Sense::click());
            let on = chosen == Some(index);
            let label = format!("{}, {size_text}", display.name);
            response.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, on, &label));
            let painter = ui.painter();
            let r12 = CornerRadius::same(radius::R12);
            if on {
                painter.rect_stroke(
                    rect,
                    r12,
                    Stroke::new(3.0, tokens.chosen_ring()),
                    StrokeKind::Outside,
                );
            }
            let fill = if response.hovered() {
                tokens.bg_hover
            } else {
                tokens.bg_raised
            };
            painter.rect_filled(rect, r12, fill);
            let border = if on { tokens.accent } else { tokens.line };
            painter.rect_stroke(rect, r12, Stroke::new(1.0, border), StrokeKind::Inside);

            // The screen, in its own proportions, centred in the slot.
            let aspect = display.height.max(1) as f32 / display.width.max(1) as f32;
            let screen_size = Vec2::new(SCREEN_WIDTH, (SCREEN_WIDTH * aspect).min(SLOT));
            let slot = Rect::from_min_size(
                pos2(rect.min.x, rect.min.y + 12.0),
                Vec2::new(rect.width(), SLOT),
            );
            let screen = Rect::from_center_size(slot.center(), screen_size);
            let (screen_fill, screen_line) = if on {
                (
                    mix(tokens.progress, tokens.bg_deep, 0.14),
                    mix(tokens.progress, tokens.line, 0.6),
                )
            } else {
                (tokens.bg_deep, tokens.line)
            };
            let r5 = CornerRadius::same(5);
            painter.rect_filled(screen, r5, screen_fill);
            if on {
                // The strip, docked along the bottom edge.
                let bar = Rect::from_min_max(
                    pos2(
                        screen.min.x,
                        screen.max.y - (screen.height() * 0.12).max(4.0),
                    ),
                    screen.max,
                );
                painter.rect_filled(
                    bar,
                    CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: 5,
                        se: 5,
                    },
                    tokens.progress,
                );
            }
            painter.rect_stroke(
                screen,
                r5,
                Stroke::new(1.0, screen_line),
                StrokeKind::Inside,
            );

            let name_top = slot.max.y + 4.0;
            let name_height = name.size().y;
            painter.galley(
                pos2(rect.center().x - name.size().x / 2.0, name_top),
                name,
                tokens.text,
            );
            painter.galley(
                pos2(
                    rect.center().x - size.size().x / 2.0,
                    name_top + name_height + 4.0,
                ),
                size,
                tokens.text_muted,
            );
            if response.has_focus() {
                focus_ring(painter, rect, radius::R12, tokens);
            }
            if response.clicked() {
                clicked = Some(index);
            }
        }
    });
    clicked
}
