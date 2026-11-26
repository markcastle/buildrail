//! Vector glyphs in the lucide manner Task Rail uses: drawn on a 24-unit
//! grid with 2 to 2.2 unit strokes, round caps and round joins, and
//! antialiased by egui's tessellator.
//!
//! * Status glyphs ([`paint_icon`]): each [`IconKind`] has its own *shape*
//!   (tick, ring with tick, filled disc with a cross, clock, …) so state
//!   reads without colour.
//! * Action glyphs: [`paint_gear`] (settings), [`paint_plus`] (add),
//!   [`paint_chevron_right`], [`paint_arrow_left`] (back), [`paint_tick`].
//! * Provider marks ([`paint_provider_mark`]) are the plain letters "GH" /
//!   "CF" in a small well: deliberately not the providers' logo artwork.
//!
//! A glyph takes the colour it is given, like `currentColor`: the caller
//! picks the token.

use std::f32::consts::{PI, TAU};

use eframe::egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, Vec2};

use super::theme::type_style;
use crate::presentation::IconKind;

/// Maps the 24-unit design grid onto the screen.
#[derive(Clone, Copy)]
struct Grid {
    center: Pos2,
    unit: f32,
    width: f32,
    color: Color32,
}

impl Grid {
    fn new(center: Pos2, size: f32, color: Color32) -> Self {
        let unit = size / 24.0;
        Self {
            center,
            unit,
            // 2.2 units, never thinner than 1.25 pt so small glyphs stay crisp.
            width: (2.2 * unit).max(1.25),
            color,
        }
    }

    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.center + Vec2::new((x - 12.0) * self.unit, (y - 12.0) * self.unit)
    }

    /// An open polyline with round caps and joins.
    fn line(&self, painter: &Painter, points: &[(f32, f32)]) {
        let points: Vec<Pos2> = points.iter().map(|&(x, y)| self.p(x, y)).collect();
        self.path(painter, points, false);
    }

    fn path(&self, painter: &Painter, points: Vec<Pos2>, closed: bool) {
        let stroke = Stroke::new(self.width, self.color);
        for point in &points {
            painter.circle_filled(*point, self.width / 2.0, self.color);
        }
        if closed {
            painter.add(Shape::closed_line(points, stroke));
        } else {
            painter.add(Shape::line(points, stroke));
        }
    }

    /// A circle outline of radius `r` units around (`x`, `y`).
    fn ring(&self, painter: &Painter, x: f32, y: f32, r: f32) {
        painter.circle_stroke(
            self.p(x, y),
            r * self.unit,
            Stroke::new(self.width, self.color),
        );
    }

    /// An arc from `start` sweeping `sweep` radians (clockwise on screen).
    fn arc(&self, painter: &Painter, x: f32, y: f32, r: f32, start: f32, sweep: f32) {
        let steps = ((sweep.abs() / TAU) * 48.0).ceil().max(4.0) as usize;
        let points = (0..=steps)
            .map(|i| {
                let a = start + sweep * i as f32 / steps as f32;
                self.p(x, y) + Vec2::angled(a) * r * self.unit
            })
            .collect();
        self.path(painter, points, false);
    }

    fn dot(&self, painter: &Painter, x: f32, y: f32, r: f32) {
        painter.circle_filled(self.p(x, y), (r * self.unit).max(0.9), self.color);
    }
}

/// Draws `icon` centred on `center`, `size` points across, in `color`.
/// `ground` is the colour behind it (the failed glyph knocks its cross out
/// of a filled disc). `phase` turns the in-progress glyph by an eighth of a
/// turn per step; `0` is its resting pose, which is what the strip shows
/// unless the user turned Animation on.
pub fn paint_icon(
    painter: &Painter,
    center: Pos2,
    size: f32,
    icon: IconKind,
    color: Color32,
    ground: Color32,
    phase: u32,
) {
    let g = Grid::new(center, size, color);
    match icon {
        IconKind::Check => g.line(painter, &[(20.0, 6.0), (9.0, 17.0), (4.0, 12.0)]),
        IconKind::CheckCircle => {
            g.ring(painter, 12.0, 12.0, 10.0);
            g.line(painter, &[(8.5, 12.0), (11.0, 14.5), (15.5, 10.0)]);
        }
        IconKind::Cross => {
            painter.circle_filled(center, 11.0 * g.unit, color);
            let knock = Grid { color: ground, ..g };
            knock.line(painter, &[(15.0, 9.0), (9.0, 15.0)]);
            knock.line(painter, &[(9.0, 9.0), (15.0, 15.0)]);
        }
        IconKind::Spinner => {
            // A faint ring with a bold third of it and a dot at the hub:
            // reads as "in progress" standing still, and differs in shape
            // from queued (clock hands) and no runs (dashed ring).
            let faint = Grid {
                color: color.gamma_multiply(0.3),
                ..g
            };
            faint.ring(painter, 12.0, 12.0, 9.5);
            let start = (phase % 8) as f32 * TAU / 8.0 - PI / 2.0;
            g.arc(painter, 12.0, 12.0, 9.5, start, TAU / 3.0);
            g.dot(painter, 12.0, 12.0, 2.2);
        }
        IconKind::Queued => {
            g.ring(painter, 12.0, 12.0, 10.0);
            g.line(painter, &[(12.0, 7.0), (12.0, 12.0), (15.5, 14.0)]);
        }
        IconKind::Dash => {
            g.ring(painter, 12.0, 12.0, 10.0);
            g.line(painter, &[(8.0, 12.0), (16.0, 12.0)]);
        }
        IconKind::Skip => {
            g.line(painter, &[(6.0, 17.0), (11.0, 12.0), (6.0, 7.0)]);
            g.line(painter, &[(13.0, 17.0), (18.0, 12.0), (13.0, 7.0)]);
        }
        IconKind::Question => {
            g.ring(painter, 12.0, 12.0, 10.0);
            g.arc(painter, 12.0, 9.6, 2.9, PI * 1.05, PI * 1.25);
            g.line(painter, &[(13.4, 12.2), (12.0, 13.4)]);
            g.dot(painter, 12.0, 17.0, 1.3);
        }
        IconKind::Empty => {
            // A dashed ring: eight arcs with gaps between them.
            for i in 0..8 {
                let start = i as f32 / 8.0 * TAU + 0.12;
                g.arc(painter, 12.0, 12.0, 10.0, start, TAU / 8.0 - 0.5);
            }
        }
        IconKind::Pending => {
            for x in [5.0, 12.0, 19.0] {
                g.dot(painter, x, 12.0, 1.6);
            }
        }
        IconKind::Warning => {
            let points = [(12.0, 3.6), (21.4, 19.6), (2.6, 19.6)]
                .iter()
                .map(|&(x, y)| g.p(x, y))
                .collect();
            g.path(painter, points, true);
            g.line(painter, &[(12.0, 9.5), (12.0, 13.0)]);
            g.dot(painter, 12.0, 16.4, 1.25);
        }
    }
}

/// Draws the settings glyph: a cog with eight teeth around a ring.
pub fn paint_gear(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let g = Grid::new(center, size, color);
    let (inner, outer) = (7.6, 10.4);
    let mut points = Vec::with_capacity(32);
    for tooth in 0..8 {
        let a = tooth as f32 / 8.0 * TAU;
        for (r, da) in [(inner, -0.30), (outer, -0.17), (outer, 0.17), (inner, 0.30)] {
            points.push(g.p(12.0, 12.0) + Vec2::angled(a + da) * r * g.unit);
        }
    }
    g.path(painter, points, true);
    g.ring(painter, 12.0, 12.0, 3.0);
}

/// Draws the add glyph.
pub fn paint_plus(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let g = Grid::new(center, size, color);
    g.line(painter, &[(5.0, 12.0), (19.0, 12.0)]);
    g.line(painter, &[(12.0, 5.0), (12.0, 19.0)]);
}

/// Draws a chevron pointing right (a row that opens something).
pub fn paint_chevron_right(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    Grid::new(center, size, color).line(painter, &[(9.0, 18.0), (15.0, 12.0), (9.0, 6.0)]);
}

/// Draws an arrow pointing left (the way back).
pub fn paint_arrow_left(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let g = Grid::new(center, size, color);
    g.line(painter, &[(19.0, 12.0), (5.0, 12.0)]);
    g.line(painter, &[(12.0, 19.0), (5.0, 12.0), (12.0, 5.0)]);
}

/// Draws a plain tick (a picked option, a saved change).
pub fn paint_tick(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    Grid::new(center, size, color).line(painter, &[(20.0, 6.0), (9.0, 17.0), (4.0, 12.0)]);
}

/// Draws a provider mark ("GH" / "CF"): two letters in `ink` in a small
/// rounded well of `fill`.
pub fn paint_provider_mark(painter: &Painter, rect: Rect, mark: &str, fill: Color32, ink: Color32) {
    painter.rect_filled(rect, CornerRadius::same(4), fill);
    let font = type_style::CHIP.font();
    let font = eframe::egui::FontId::new(rect.height() * 0.62, font.family);
    painter.text(rect.center(), Align2::CENTER_CENTER, mark, font, ink);
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Color32, Pos2, Rect, Vec2};

    use super::*;

    #[test]
    fn every_glyph_paints_shapes() {
        let ctx = egui::Context::default();
        let icons = [
            IconKind::Check,
            IconKind::CheckCircle,
            IconKind::Cross,
            IconKind::Spinner,
            IconKind::Queued,
            IconKind::Dash,
            IconKind::Skip,
            IconKind::Question,
            IconKind::Empty,
            IconKind::Pending,
            IconKind::Warning,
        ];
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter();
            for (i, icon) in icons.iter().enumerate() {
                paint_icon(
                    painter,
                    Pos2::new(10.0 + 20.0 * i as f32, 12.0),
                    12.0,
                    *icon,
                    Color32::RED,
                    Color32::BLACK,
                    3,
                );
            }
            paint_provider_mark(
                painter,
                Rect::from_min_size(Pos2::ZERO, Vec2::new(18.0, 14.0)),
                "GH",
                Color32::DARK_GRAY,
                Color32::GRAY,
            );
            paint_gear(painter, Pos2::new(300.0, 12.0), 14.0, Color32::WHITE);
            paint_plus(painter, Pos2::new(320.0, 12.0), 14.0, Color32::WHITE);
            paint_chevron_right(painter, Pos2::new(340.0, 12.0), 14.0, Color32::WHITE);
            paint_arrow_left(painter, Pos2::new(360.0, 12.0), 14.0, Color32::WHITE);
            paint_tick(painter, Pos2::new(380.0, 12.0), 14.0, Color32::WHITE);
        });
        output.textures_delta.clear();
        // Several shapes per glyph (strokes plus their round caps).
        assert!(
            output.shapes.len() > 3 * icons.len(),
            "{}",
            output.shapes.len()
        );
    }
}
