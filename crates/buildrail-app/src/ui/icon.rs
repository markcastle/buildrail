//! The BuildRail mark: a rail with three status stops on it, on a dark
//! rounded tile. Drawn in code so the window icon needs no image decoder
//! and the checked-in `assets/buildrail.ico` (generated from the same
//! function by `tests/icon.rs`) can never drift from it.
//!
//! Deliberately not Task Rail's mark (three stacked bars): one horizontal
//! rail, three round stops in the passed, building and failed colours,
//! the middle one hollow (in progress).

use eframe::egui::IconData;

/// The tile.
const TILE: [u8; 3] = [0x17, 0x1d, 0x26];
/// The rail.
const RAIL: [u8; 3] = [0x6b, 0x7a, 0x8c];
/// The stops: passed, in progress, failed.
const STOPS: [[u8; 3]; 3] = [[0x3f, 0xb9, 0x50], [0xe3, 0xa4, 0x2b], [0xf2, 0x5c, 0x54]];

/// Supersampling grid per pixel edge (antialiasing).
const SAMPLES: u32 = 4;

/// A colour and its coverage at a point of the unit square, front-most
/// shape last.
fn shade(x: f32, y: f32) -> Option<[u8; 3]> {
    let mut colour = None;
    // Rounded tile.
    if rounded_rect(x, y, 0.04, 0.96, 0.22) {
        colour = Some(TILE);
    }
    // The rail, with round ends.
    if capsule(x, y, 0.16, 0.84, 0.5, 0.045) {
        colour = Some(RAIL);
    }
    for (index, stop) in STOPS.iter().enumerate() {
        let cx = 0.27 + 0.23 * index as f32;
        let d = ((x - cx).powi(2) + (y - 0.5).powi(2)).sqrt();
        // A ring of tile colour separates each stop from the rail.
        if d <= 0.145 {
            colour = Some(TILE);
        }
        let hollow = index == 1;
        if d <= 0.11 && !(hollow && d < 0.06) {
            colour = Some(*stop);
        }
    }
    colour
}

fn rounded_rect(x: f32, y: f32, min: f32, max: f32, radius: f32) -> bool {
    let cx = x.clamp(min + radius, max - radius);
    let cy = y.clamp(min + radius, max - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
}

fn capsule(x: f32, y: f32, x0: f32, x1: f32, cy: f32, half: f32) -> bool {
    let px = x.clamp(x0, x1);
    (x - px).powi(2) + (y - cy).powi(2) <= half * half
}

/// The mark as straight (not premultiplied) RGBA, `size` × `size`.
pub fn rgba(size: u32) -> Vec<u8> {
    let size = size.max(1);
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    let samples = (SAMPLES * SAMPLES) as f32;
    for py in 0..size {
        for px in 0..size {
            let mut sum = [0.0f32; 3];
            let mut covered = 0.0f32;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    if let Some(c) = shade(x, y) {
                        covered += 1.0;
                        for (s, v) in sum.iter_mut().zip(c) {
                            *s += f32::from(v);
                        }
                    }
                }
            }
            if covered > 0.0 {
                for s in sum {
                    pixels.push((s / covered).round() as u8);
                }
            } else {
                pixels.extend([0, 0, 0]);
            }
            pixels.push((covered / samples * 255.0).round() as u8);
        }
    }
    pixels
}

/// The window icon (64 × 64; the OS scales it for the title bar, the
/// task switcher and the dock).
pub fn window_icon() -> IconData {
    let size = 64;
    IconData {
        rgba: rgba(size),
        width: size,
        height: size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_has_a_transparent_corner_an_opaque_tile_and_three_stops() {
        let size = 64;
        let pixels = rgba(size);
        assert_eq!(pixels.len(), (size * size * 4) as usize);
        let at = |x: u32, y: u32| {
            let i = ((y * size + x) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
        };
        assert_eq!(at(0, 0)[3], 0, "rounded corner");
        assert_eq!(at(32, 10), [TILE[0], TILE[1], TILE[2], 255]);
        let stop = |index: u32| at((size as f32 * (0.27 + 0.23 * index as f32)) as u32 + 4, 32);
        assert_eq!(stop(0)[..3], STOPS[0]);
        assert_eq!(stop(2)[..3], STOPS[2]);
        // The middle stop is hollow.
        assert_eq!(at(32, 32)[..3], TILE);
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (64, 64));
        assert_eq!(rgba(0).len(), 4);
    }
}
