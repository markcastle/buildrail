//! Simple vector glyphs: status icons and the plain-letter provider marks.
//!
//! Each [`IconKind`] has its own *shape* (tick, cross, ring, bar, chevrons,
//! triangle, …) so status is readable without colour. Provider marks are
//! the letters "GH" / "CF" in a rounded box – deliberately not the
//! providers' logo artwork.

use std::f32::consts::TAU;

use eframe::egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, Vec2};

use crate::presentation::IconKind;

/// Draws `icon` centred on `center`, `size` points across. `time` (seconds)
/// rotates the spinner.
pub fn paint_icon(
    painter: &Painter,
    center: Pos2,
    size: f32,
    icon: IconKind,
    color: Color32,
    time: f64,
) {
    let r = size / 2.0;
    let stroke = Stroke::new((size / 7.0).max(1.4), color);
    let p = |x: f32, y: f32| center + Vec2::new(x * r, y * r);
    match icon {
        IconKind::Check => {
            painter.line(vec![p(-0.8, 0.0), p(-0.25, 0.6), p(0.85, -0.65)], stroke);
        }
        IconKind::CheckCircle => {
            painter.circle_stroke(center, r, Stroke::new(stroke.width * 0.8, color));
            painter.line(vec![p(-0.5, 0.0), p(-0.15, 0.4), p(0.5, -0.4)], stroke);
        }
        IconKind::Cross => {
            painter.line_segment([p(-0.7, -0.7), p(0.7, 0.7)], stroke);
            painter.line_segment([p(-0.7, 0.7), p(0.7, -0.7)], stroke);
        }
        IconKind::Spinner => {
            // Faint full ring plus a rotating three-quarter arc.
            painter.circle_stroke(
                center,
                r * 0.85,
                Stroke::new(stroke.width, color.gamma_multiply(0.35)),
            );
            let start = (time as f32 * TAU * 0.8) % TAU;
            let points: Vec<Pos2> = (0..=18)
                .map(|i| {
                    let a = start + (i as f32 / 18.0) * TAU * 0.75;
                    center + Vec2::angled(a) * (r * 0.85)
                })
                .collect();
            painter.line(points, stroke);
        }
        IconKind::Queued => {
            painter.circle_stroke(center, r * 0.85, stroke);
            painter.circle_filled(center, r * 0.25, color);
        }
        IconKind::Dash => {
            painter.line_segment(
                [p(-0.75, 0.0), p(0.75, 0.0)],
                Stroke::new(stroke.width * 1.3, color),
            );
        }
        IconKind::Skip => {
            painter.line(vec![p(-0.8, -0.6), p(-0.2, 0.0), p(-0.8, 0.6)], stroke);
            painter.line(vec![p(0.1, -0.6), p(0.7, 0.0), p(0.1, 0.6)], stroke);
        }
        IconKind::Question => {
            painter.text(
                center,
                Align2::CENTER_CENTER,
                "?",
                FontId::proportional(size * 1.15),
                color,
            );
        }
        IconKind::Empty => {
            painter.circle_stroke(center, r * 0.8, Stroke::new(stroke.width * 0.8, color));
        }
        IconKind::Pending => {
            for x in [-0.65, 0.0, 0.65] {
                painter.circle_filled(p(x, 0.0), r * 0.17, color);
            }
        }
        IconKind::Warning => {
            painter.line(
                vec![p(0.0, -0.9), p(0.9, 0.75), p(-0.9, 0.75), p(0.0, -0.9)],
                Stroke::new(stroke.width * 0.8, color),
            );
            painter.line_segment([p(0.0, -0.3), p(0.0, 0.2)], stroke);
            painter.circle_filled(p(0.0, 0.48), r * 0.1, color);
        }
    }
}

/// Draws a provider mark ("GH" / "CF") in a rounded outline inside `rect`.
pub fn paint_provider_mark(painter: &Painter, rect: Rect, mark: &str, color: Color32) {
    painter.rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, color.gamma_multiply(0.7)),
        eframe::egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        mark,
        FontId::monospace(rect.height() * 0.62),
        color,
    );
}

/// Draws a gear (settings) glyph.
pub fn paint_gear(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let r = size / 2.0;
    let stroke = Stroke::new((size / 8.0).max(1.3), color);
    painter.circle_stroke(center, r * 0.5, stroke);
    for i in 0..8 {
        let a = i as f32 / 8.0 * TAU;
        painter.line_segment(
            [
                center + Vec2::angled(a) * r * 0.62,
                center + Vec2::angled(a) * r * 0.95,
            ],
            stroke,
        );
    }
}

/// Draws a plus (add) glyph.
pub fn paint_plus(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let r = size / 2.0;
    let stroke = Stroke::new((size / 7.0).max(1.4), color);
    painter.line_segment([center - Vec2::X * r, center + Vec2::X * r], stroke);
    painter.line_segment([center - Vec2::Y * r, center + Vec2::Y * r], stroke);
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
                    11.0,
                    *icon,
                    Color32::RED,
                    1.5,
                );
            }
            paint_provider_mark(
                painter,
                Rect::from_min_size(Pos2::ZERO, Vec2::new(19.0, 13.0)),
                "GH",
                Color32::GRAY,
            );
            paint_gear(painter, Pos2::new(300.0, 12.0), 13.0, Color32::WHITE);
            paint_plus(painter, Pos2::new(320.0, 12.0), 10.0, Color32::WHITE);
        });
        output.textures_delta.clear();
        // At least one shape per icon plus the marks.
        assert!(
            output.shapes.len() > icons.len() + 3,
            "{}",
            output.shapes.len()
        );
    }
}
