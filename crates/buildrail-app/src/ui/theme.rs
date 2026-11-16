//! Colours for the strip, light and dark, with their contrast checked.
//!
//! The strip draws its own background, so its text colours are chosen
//! against that background (and against the hover/focus highlight) and
//! unit-tested to meet WCAG 2.x AA for normal text (≥ 4.5 : 1). Status
//! colours are applied to the status *word* and icon; the word and the icon
//! shape carry the meaning on their own, colour is a third cue.

use buildrail_core::domain::Tone;
use buildrail_profile::Theme;
use eframe::egui::{self, Color32};

/// Every colour the strip uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// `true` for the dark palette.
    pub dark: bool,
    /// Strip background.
    pub background: Color32,
    /// Background of a hovered or focused entry.
    pub highlight: Color32,
    /// Labels and captions.
    pub text: Color32,
    /// Notes ("5 min ago · offline") and separators' text.
    pub muted: Color32,
    /// Thin separators between entries.
    pub separator: Color32,
    /// Passed / deployed.
    pub green: Color32,
    /// Queued / running.
    pub amber: Color32,
    /// Failed.
    pub red: Color32,
    /// Cancelled, skipped, unavailable, no runs, problems.
    pub neutral: Color32,
    /// Keyboard focus ring.
    pub focus: Color32,
}

/// The dark palette.
pub const DARK: Palette = Palette {
    dark: true,
    background: Color32::from_rgb(0x1C, 0x1E, 0x22),
    highlight: Color32::from_rgb(0x2B, 0x2F, 0x36),
    text: Color32::from_rgb(0xE8, 0xEA, 0xED),
    muted: Color32::from_rgb(0xA9, 0xAE, 0xB6),
    separator: Color32::from_rgb(0x3E, 0x43, 0x4B),
    green: Color32::from_rgb(0x4C, 0xC3, 0x8A),
    amber: Color32::from_rgb(0xF0, 0xB4, 0x29),
    red: Color32::from_rgb(0xFF, 0x7B, 0x72),
    neutral: Color32::from_rgb(0xAE, 0xB4, 0xBC),
    focus: Color32::from_rgb(0x7A, 0xB7, 0xFF),
};

/// The light palette.
pub const LIGHT: Palette = Palette {
    dark: false,
    background: Color32::from_rgb(0xF5, 0xF6, 0xF8),
    highlight: Color32::from_rgb(0xE6, 0xE9, 0xED),
    text: Color32::from_rgb(0x1F, 0x23, 0x28),
    muted: Color32::from_rgb(0x57, 0x60, 0x6A),
    separator: Color32::from_rgb(0xC9, 0xCE, 0xD4),
    green: Color32::from_rgb(0x11, 0x6A, 0x2E),
    amber: Color32::from_rgb(0x8A, 0x5A, 0x00),
    red: Color32::from_rgb(0xC4, 0x23, 0x2F),
    neutral: Color32::from_rgb(0x59, 0x63, 0x6E),
    focus: Color32::from_rgb(0x0B, 0x5F, 0xCC),
};

impl Palette {
    /// The palette for a theme preference; `system_dark` is what the OS
    /// reports (`None` = unknown, treated as dark, which suits a strip next
    /// to most taskbars).
    pub fn for_theme(theme: Theme, system_dark: Option<bool>) -> Self {
        let dark = match theme {
            Theme::Dark => true,
            Theme::Light => false,
            Theme::System => system_dark.unwrap_or(true),
        };
        if dark {
            DARK
        } else {
            LIGHT
        }
    }

    /// Colour of a status tone.
    pub fn tone(&self, tone: Tone) -> Color32 {
        match tone {
            Tone::Green => self.green,
            Tone::Amber => self.amber,
            Tone::Red => self.red,
            Tone::Neutral => self.neutral,
        }
    }

    /// egui's own theme for the settings window, matching the strip.
    pub fn egui_theme(&self) -> egui::Theme {
        if self.dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        }
    }
}

/// WCAG 2.x relative luminance of an sRGB colour.
pub fn relative_luminance(color: Color32) -> f64 {
    let channel = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

/// WCAG contrast ratio between two colours (1.0 – 21.0).
pub fn contrast_ratio(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2.x AA for normal-size text.
    const AA: f64 = 4.5;

    #[test]
    fn contrast_formula_matches_known_values() {
        assert!((contrast_ratio(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 1e-9);
        // #777 on white is the classic 4.48 "just fails AA" example.
        let grey = Color32::from_rgb(0x77, 0x77, 0x77);
        assert!((contrast_ratio(grey, Color32::WHITE) - 4.48).abs() < 0.01);
    }

    #[test]
    fn every_text_colour_meets_aa_on_background_and_highlight() {
        for palette in [DARK, LIGHT] {
            let texts = [
                ("text", palette.text),
                ("muted", palette.muted),
                ("green", palette.green),
                ("amber", palette.amber),
                ("red", palette.red),
                ("neutral", palette.neutral),
                ("focus", palette.focus),
            ];
            for (name, color) in texts {
                for (bg_name, bg) in [
                    ("background", palette.background),
                    ("highlight", palette.highlight),
                ] {
                    let ratio = contrast_ratio(color, bg);
                    assert!(
                        ratio >= AA,
                        "{} {name} on {bg_name}: {ratio:.2}",
                        if palette.dark { "dark" } else { "light" }
                    );
                }
            }
        }
    }

    #[test]
    fn theme_selection() {
        assert_eq!(Palette::for_theme(Theme::Dark, Some(false)), DARK);
        assert_eq!(Palette::for_theme(Theme::Light, Some(true)), LIGHT);
        assert_eq!(Palette::for_theme(Theme::System, Some(false)), LIGHT);
        assert_eq!(Palette::for_theme(Theme::System, None), DARK);
        assert_eq!(DARK.tone(Tone::Red), DARK.red);
        assert_eq!(DARK.tone(Tone::Green), DARK.green);
        assert_eq!(DARK.tone(Tone::Amber), DARK.amber);
        assert_eq!(DARK.tone(Tone::Neutral), DARK.neutral);
        assert_eq!(DARK.egui_theme(), egui::Theme::Dark);
        assert_eq!(LIGHT.egui_theme(), egui::Theme::Light);
    }
}
