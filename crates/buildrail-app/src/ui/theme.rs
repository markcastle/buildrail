//! The Task Rail design tokens: colour (light and dark), type, space,
//! radius and the one shadow, named as in the design system's
//! `tokens.json`.
//!
//! Every colour the UI paints comes from a [`Tokens`] value. A few roles
//! are *derived* from two tokens with [`mix`], the same `color-mix(in srgb,
//! …)` technique the system's own stylesheet uses (the current settings
//! page, a primary button under the pointer). Those derivations live here,
//! next to the tokens, so no widget invents a colour.
//!
//! # Contrast
//!
//! The strip is read in a glance, so its text is held to WCAG 2.x AA for
//! normal text (4.5 : 1) on both its ground (`bg`) and its hover pill
//! (`bg-hover`), and its glyphs to the 3 : 1 non-text floor. Task Rail
//! documents a few pairs as a hair under the floor (`text-muted` and
//! `priority` on `bg` in light). On the strip those are replaced by the
//! nearest passing treatment; see [`Tokens::strip_muted`] and
//! [`Tokens::status`]. The tests at the bottom check every pair.

use buildrail_core::domain::Tone;
use buildrail_profile::Theme;
use eframe::egui::{self, Color32, FontFamily, FontId};

use super::fonts;

/// A layer of the `card-shadow` token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowLayer {
    /// Vertical offset in points.
    pub offset_y: i8,
    /// Blur radius in points.
    pub blur: u8,
    /// Colour, alpha included.
    pub color: Color32,
}

impl ShadowLayer {
    /// The layer as an egui shadow.
    pub fn egui(&self) -> egui::Shadow {
        egui::Shadow {
            offset: [0, self.offset_y],
            blur: self.blur,
            spread: 0,
            color: self.color,
        }
    }
}

/// Every Task Rail colour token for one theme, named as in `tokens.json`
/// (`bg-deep` is `bg_deep`, and so on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokens {
    /// `true` for the dark theme.
    pub dark: bool,
    /// The one blue: the mark. Use [`Self::accent`] for anything to act on.
    pub brand: Color32,
    /// The ground of the strip and of the Settings window.
    pub bg: Color32,
    /// Wells set into the ground: icon buttons, tracks, inputs.
    pub bg_deep: Color32,
    /// Surfaces lifted off the ground: settings groups, popups.
    pub bg_raised: Color32,
    /// A row, card or quiet button under the pointer.
    pub bg_hover: Color32,
    /// Body text, labels and headings.
    pub text: Color32,
    /// Help, meta lines, counts and placeholders.
    pub text_muted: Color32,
    /// Thin lines between rows and around groups.
    pub line: Color32,
    /// "You can act here": primary buttons, the chosen item, focus rings.
    pub accent: Color32,
    /// The quiet fill behind something accent.
    pub accent_soft: Color32,
    /// Text and glyphs on an accent fill.
    pub accent_text: Color32,
    /// Late or failed.
    pub priority: Color32,
    /// Under way (always the brand blue).
    pub progress: Color32,
    /// Finished or healthy; a switch that is on.
    pub done: Color32,
    /// A calendar event (unused by BuildRail, kept for completeness).
    pub event: Color32,
    /// Reserved palette.
    pub accent_red: Color32,
    /// Reserved palette; BuildRail's in-flight (amber) glyph.
    pub accent_orange: Color32,
    /// Reserved palette.
    pub accent_blue: Color32,
    /// Reserved palette.
    pub accent_green: Color32,
    /// Reserved palette.
    pub accent_purple: Color32,
    /// Reserved palette; an idle dot.
    pub accent_slate: Color32,
    /// Fill of a High chip.
    pub chip_high_bg: Color32,
    /// Text of a High chip; BuildRail's in-flight status word.
    pub chip_high_fg: Color32,
    /// Fill of an Urgent chip.
    pub chip_urgent_bg: Color32,
    /// Text of an Urgent chip; the light strip's "Failed" word.
    pub chip_urgent_fg: Color32,
    /// What the window paints behind the strip (flat, the same as `bg`).
    pub rail_gradient: Color32,
    /// The one shadow, two layers, for settings groups and popups.
    pub card_shadow: [ShadowLayer; 2],
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// `rgba(r, g, b, a)` as egui's premultiplied colour.
const fn rgba(rgb: u32, alpha: f32) -> Color32 {
    const fn scale(c: u32, alpha: f32) -> u8 {
        ((c & 0xff) as f32 * alpha + 0.5) as u8
    }
    Color32::from_rgba_premultiplied(
        scale(rgb >> 16, alpha),
        scale(rgb >> 8, alpha),
        scale(rgb, alpha),
        (alpha * 255.0 + 0.5) as u8,
    )
}

/// The light theme.
pub const LIGHT: Tokens = Tokens {
    dark: false,
    brand: hex(0x0b5cff),
    bg: hex(0xf6f7fa),
    bg_deep: hex(0xeef0f5),
    bg_raised: hex(0xffffff),
    bg_hover: hex(0xeef1f7),
    text: hex(0x0f1420),
    text_muted: hex(0x6a7389),
    line: hex(0xe4e7ee),
    accent: hex(0x0b5cff),
    accent_soft: hex(0xe9f0ff),
    accent_text: hex(0xffffff),
    priority: hex(0xd93036),
    progress: hex(0x0b5cff),
    done: hex(0x1a9b57),
    event: hex(0x7c5cff),
    accent_red: hex(0xef4444),
    accent_orange: hex(0xf59e0b),
    accent_blue: hex(0x0b5cff),
    accent_green: hex(0x22c55e),
    accent_purple: hex(0x8b5cf6),
    accent_slate: hex(0x94a3b8),
    chip_high_bg: hex(0xfff1dc),
    chip_high_fg: hex(0x9a5500),
    chip_urgent_bg: hex(0xffe4e4),
    chip_urgent_fg: hex(0xb4232a),
    rail_gradient: hex(0xf6f7fa),
    card_shadow: [
        ShadowLayer {
            offset_y: 1,
            blur: 2,
            color: rgba(0x0f1420, 0.06),
        },
        ShadowLayer {
            offset_y: 4,
            blur: 14,
            color: rgba(0x0f1420, 0.05),
        },
    ],
};

/// The dark theme. Tokens with a single value in `tokens.json` (`brand`,
/// `progress`, `accent-text` and the reserved palette but slate) are the
/// same in both themes.
pub const DARK: Tokens = Tokens {
    dark: true,
    brand: hex(0x0b5cff),
    bg: hex(0x0e1117),
    bg_deep: hex(0x0a0d12),
    bg_raised: hex(0x161a22),
    bg_hover: hex(0x1e2330),
    text: hex(0xf2f4f8),
    text_muted: hex(0x98a2b8),
    line: hex(0x262c38),
    accent: hex(0x2f7bff),
    accent_soft: hex(0x15243d),
    accent_text: hex(0xffffff),
    priority: hex(0xff6b6f),
    progress: hex(0x0b5cff),
    done: hex(0x3ccf7a),
    event: hex(0xa98bff),
    accent_red: hex(0xef4444),
    accent_orange: hex(0xf59e0b),
    accent_blue: hex(0x0b5cff),
    accent_green: hex(0x22c55e),
    accent_purple: hex(0x8b5cf6),
    accent_slate: hex(0x64748b),
    chip_high_bg: hex(0x3a2c12),
    chip_high_fg: hex(0xf3c06b),
    chip_urgent_bg: hex(0x3d1a1c),
    chip_urgent_fg: hex(0xff8a8e),
    rail_gradient: hex(0x0e1117),
    card_shadow: [
        ShadowLayer {
            offset_y: 1,
            blur: 2,
            color: rgba(0x000000, 0.4),
        },
        ShadowLayer {
            offset_y: 6,
            blur: 18,
            color: rgba(0x000000, 0.3),
        },
    ],
};

/// `color-mix(in srgb, a <t·100>%, b)`: `t = 1` is `a`, `t = 0` is `b`.
/// Opaque colours only (alpha is mixed too, but the tokens are opaque).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (f32::from(x) * t + f32::from(y) * (1.0 - t)).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}

/// The colours of one build state on the strip: the glyph and the status
/// word. A state colour never works alone; the word always says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusInk {
    /// The status glyph (3 : 1 floor).
    pub glyph: Color32,
    /// The status word (4.5 : 1 floor).
    pub word: Color32,
}

impl Tokens {
    /// The tokens for a theme preference; `system_dark` is what the OS
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

    /// egui's own theme for these tokens.
    pub fn egui_theme(&self) -> egui::Theme {
        if self.dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        }
    }

    /// Secondary text on the strip. `text-muted` itself in dark; in light
    /// it is 4.2 : 1 on `bg-hover`, so it is mixed 10 % toward `text`, the
    /// smallest step that clears 4.5 : 1.
    pub fn strip_muted(&self) -> Color32 {
        if self.dark {
            self.text_muted
        } else {
            mix(self.text_muted, self.text, 0.9)
        }
    }

    /// Text that reports an error (a refused token, a file that will not
    /// load). `priority` where it passes as small text, `chip-urgent-fg`
    /// in light, where `priority` is 4.4 : 1.
    pub fn error_text(&self) -> Color32 {
        if self.dark {
            self.priority
        } else {
            self.chip_urgent_fg
        }
    }

    /// The colours of a build state on the strip.
    ///
    /// * Failed – `priority` glyph; the word in `priority` (dark) or
    ///   `chip-urgent-fg` (light, where `priority` is under the floor).
    /// * Passed / deployed – `done` glyph; the word in `done` (dark) or
    ///   `done` mixed 25 % toward `text` (light, where `done` is 3.3 : 1).
    /// * Queued / building – amber, as the product requires for in-flight
    ///   builds: the glyph in `accent-orange` (dark) or `accent-orange`
    ///   mixed 60 % toward `chip-high-fg` (light, where it is 1.9 : 1), the
    ///   word in `chip-high-fg`.
    /// * Everything else – `text-muted` glyph, [`Self::strip_muted`] word.
    pub fn status(&self, tone: Tone) -> StatusInk {
        match (tone, self.dark) {
            (Tone::Red, true) => StatusInk {
                glyph: self.priority,
                word: self.priority,
            },
            (Tone::Red, false) => StatusInk {
                glyph: self.priority,
                word: self.chip_urgent_fg,
            },
            (Tone::Green, true) => StatusInk {
                glyph: self.done,
                word: self.done,
            },
            (Tone::Green, false) => StatusInk {
                glyph: self.done,
                word: mix(self.done, self.text, 0.75),
            },
            (Tone::Amber, true) => StatusInk {
                glyph: self.accent_orange,
                word: self.chip_high_fg,
            },
            (Tone::Amber, false) => StatusInk {
                glyph: mix(self.accent_orange, self.chip_high_fg, 0.4),
                word: self.chip_high_fg,
            },
            (Tone::Neutral, _) => StatusInk {
                glyph: self.text_muted,
                word: self.strip_muted(),
            },
        }
    }

    /// A primary button under the pointer: `accent` 12 % toward black (the
    /// light theme's `text`, the darkest token).
    pub fn accent_hover(&self) -> Color32 {
        mix(self.accent, LIGHT.text, 0.88)
    }

    /// The Settings navigation's ground: `bg-raised` at 70 % over `bg`.
    pub fn nav_fill(&self) -> Color32 {
        mix(self.bg_raised, self.bg, 0.7)
    }

    /// The current Settings page in the navigation: `progress` 18 % into
    /// `bg-raised`.
    pub fn nav_current(&self) -> Color32 {
        mix(self.progress, self.bg_raised, 0.18)
    }

    /// The faint ring around a chosen tile or card: `accent` 20 % over the
    /// raised surface.
    pub fn chosen_ring(&self) -> Color32 {
        mix(self.accent, self.bg_raised, 0.2)
    }

    /// The faint fill of a danger button under the pointer: `priority`
    /// 10 % over `bg-raised`.
    pub fn danger_hover(&self) -> Color32 {
        mix(self.priority, self.bg_raised, 0.1)
    }
}

// ---------------------------------------------------------------- space

/// Spacing tokens (`space-N`), in points.
pub mod space {
    /// Between a label and its help line; between navigation items.
    pub const S2: f32 = 2.0;
    /// Between a page title and its sentence; inside the segmented track.
    pub const S4: f32 = 4.0;
    /// Between tick chips; between an icon and its label.
    pub const S6: f32 = 6.0;
    /// The default gap.
    pub const S8: f32 = 8.0;
    /// Between a control and its value; inside a toast.
    pub const S10: f32 = 10.0;
    /// Card padding, top and bottom.
    pub const S12: f32 = 12.0;
    /// A settings row, top and bottom.
    pub const S14: f32 = 14.0;
    /// A settings row and a group, left and right.
    pub const S18: f32 = 18.0;
    /// Between a setting's text and its control; between groups.
    pub const S24: f32 = 24.0;
    /// The Settings page's side padding.
    pub const S36: f32 = 36.0;
}

/// Corner radius tokens (`radius-N`), in points.
pub mod radius {
    /// A small coloured bar.
    pub const R2: u8 = 2;
    /// A progress track.
    pub const R3: u8 = 3;
    /// Chips; the strip's hover pill.
    pub const R6: u8 = 6;
    /// A segment inside the segmented control.
    pub const R8: u8 = 8;
    /// Buttons, icon buttons, text inputs, navigation items.
    pub const R9: u8 = 9;
    /// The segmented track, a toast.
    pub const R10: u8 = 10;
    /// A card, a monitor tile, a choice card, a popup.
    pub const R12: u8 = 12;
    /// A settings group, the largest surface.
    pub const R14: u8 = 14;
    /// Switches, counts.
    pub const PILL: u8 = u8::MAX;
}

// ----------------------------------------------------------------- type

/// A font weight. Each maps to a real face registered by
/// [`fonts`](super::fonts), never a faked bold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    /// 400.
    Regular,
    /// 500.
    Medium,
    /// 600 (and the system's 650).
    Semibold,
    /// 700.
    Bold,
}

impl Weight {
    /// The egui font family holding this weight.
    pub fn family(self) -> FontFamily {
        match self {
            Self::Regular => FontFamily::Proportional,
            Self::Medium => FontFamily::Name(fonts::MEDIUM.into()),
            Self::Semibold => FontFamily::Name(fonts::SEMIBOLD.into()),
            Self::Bold => FontFamily::Name(fonts::BOLD.into()),
        }
    }
}

/// One of the system's type styles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeStyle {
    /// Size in points.
    pub size: f32,
    /// Weight.
    pub weight: Weight,
    /// Letter spacing in em (`-0.01` tightens titles).
    pub tracking: f32,
    /// Line height as a multiple of the size, when the style sets one.
    pub line_height: Option<f32>,
    /// Set in capitals (the group label).
    pub uppercase: bool,
}

impl TypeStyle {
    const fn new(size: f32, weight: Weight) -> Self {
        Self {
            size,
            weight,
            tracking: 0.0,
            line_height: None,
            uppercase: false,
        }
    }

    const fn tracked(mut self, tracking: f32) -> Self {
        self.tracking = tracking;
        self
    }

    const fn leading(mut self, line_height: f32) -> Self {
        self.line_height = Some(line_height);
        self
    }

    /// The font.
    pub fn font(&self) -> FontId {
        FontId::new(self.size, self.weight.family())
    }

    /// An egui text format in `color`.
    pub fn format(&self, color: Color32) -> egui::TextFormat {
        egui::TextFormat {
            font_id: self.font(),
            extra_letter_spacing: self.tracking * self.size,
            line_height: self.line_height.map(|l| l * self.size),
            color,
            ..Default::default()
        }
    }

    /// `text` as a one-style layout job (capitalised for the group label).
    pub fn job(&self, text: &str, color: Color32) -> egui::text::LayoutJob {
        let text = if self.uppercase {
            text.to_uppercase()
        } else {
            text.to_owned()
        };
        egui::text::LayoutJob::single_section(text, self.format(color))
    }

    /// Like [`Self::job`], wrapped at `width`.
    pub fn wrapped(&self, text: &str, color: Color32, width: f32) -> egui::text::LayoutJob {
        let mut job = self.job(text, color);
        job.wrap.max_width = width;
        job
    }
}

/// The type styles of `tokens.json`, plus the strip's own sizes.
pub mod type_style {
    use super::{TypeStyle, Weight};

    /// The sentence at the top of a view (21/700, -0.02em).
    pub const HEADLINE: TypeStyle = TypeStyle::new(21.0, Weight::Bold)
        .tracked(-0.02)
        .leading(1.2);
    /// The product name beside the mark (17/700).
    pub const BRAND: TypeStyle = TypeStyle::new(17.0, Weight::Bold).tracked(-0.01);
    /// A section heading (15/700).
    pub const SECTION_TITLE: TypeStyle = TypeStyle::new(15.0, Weight::Bold).tracked(-0.01);
    /// A card's title (14/500).
    pub const CARD_TITLE: TypeStyle = TypeStyle::new(14.0, Weight::Medium).leading(1.35);
    /// The line under a title (12/400, in `text-muted`).
    pub const META: TypeStyle = TypeStyle::new(12.0, Weight::Regular);
    /// A chip (11/600).
    pub const CHIP: TypeStyle = TypeStyle::new(11.0, Weight::Semibold);
    /// A Settings page title (24/700, -0.01em).
    pub const PAGE_TITLE: TypeStyle = TypeStyle::new(24.0, Weight::Bold).tracked(-0.01);
    /// A heading between groups (15/650).
    pub const SUBHEAD: TypeStyle = TypeStyle::new(15.0, Weight::Semibold);
    /// A setting's name, a navigation item (14/600).
    pub const LABEL: TypeStyle = TypeStyle::new(14.0, Weight::Semibold);
    /// Running text (14/400).
    pub const BODY: TypeStyle = TypeStyle::new(14.0, Weight::Regular).leading(1.45);
    /// The sentence under a setting's name (12.5/400, line height 1.4).
    pub const HELP: TypeStyle = TypeStyle::new(12.5, Weight::Regular).leading(1.4);
    /// A divider label inside a group (13/600, +0.04em, capitals).
    pub const GROUP_LABEL: TypeStyle = TypeStyle {
        uppercase: true,
        ..TypeStyle::new(13.0, Weight::Semibold).tracked(0.04)
    };
    /// Button labels (13/500).
    pub const BUTTON: TypeStyle = TypeStyle::new(13.0, Weight::Medium);
    /// A strip entry's label (13/500): `card-title` set one step smaller
    /// to fit the 24 px strip.
    pub const STRIP_LABEL: TypeStyle = TypeStyle::new(13.0, Weight::Medium);
    /// A strip entry's status word (13/400).
    pub const STRIP_STATUS: TypeStyle = TypeStyle::new(13.0, Weight::Regular);
    /// The strip's "Builds" caption (12/600).
    pub const STRIP_CAPTION: TypeStyle = TypeStyle::new(12.0, Weight::Semibold);
}

// ------------------------------------------------------------- contrast

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
    /// WCAG 2.x non-text contrast (glyphs, focus rings).
    const NON_TEXT: f64 = 3.0;

    fn name(tokens: &Tokens) -> &'static str {
        if tokens.dark {
            "dark"
        } else {
            "light"
        }
    }

    #[test]
    fn contrast_formula_matches_known_values() {
        assert!((contrast_ratio(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 1e-9);
        // #777 on white is the classic 4.48 "just fails AA" example.
        let grey = Color32::from_rgb(0x77, 0x77, 0x77);
        assert!((contrast_ratio(grey, Color32::WHITE) - 4.48).abs() < 0.01);
    }

    #[test]
    fn tokens_match_tokens_json() {
        assert_eq!(LIGHT.bg, Color32::from_rgb(0xf6, 0xf7, 0xfa));
        assert_eq!(DARK.bg_raised, Color32::from_rgb(0x16, 0x1a, 0x22));
        assert_eq!(LIGHT.accent, LIGHT.brand);
        assert_eq!(
            DARK.progress, DARK.brand,
            "progress is always the brand blue"
        );
        assert_eq!(LIGHT.rail_gradient, LIGHT.bg);
        assert_eq!(DARK.rail_gradient, DARK.bg);
        // rgba(15, 20, 32, 0.06), premultiplied.
        assert_eq!(LIGHT.card_shadow[0].color.a(), 15);
        assert_eq!(DARK.card_shadow[1].egui().blur, 18);
    }

    #[test]
    fn every_strip_text_colour_meets_aa_on_bg_and_hover() {
        for tokens in [DARK, LIGHT] {
            let mut texts = vec![("text", tokens.text), ("strip muted", tokens.strip_muted())];
            for tone in [Tone::Red, Tone::Green, Tone::Amber, Tone::Neutral] {
                texts.push(("status word", tokens.status(tone).word));
            }
            for (what, color) in texts {
                for (ground, bg) in [("bg", tokens.bg), ("bg-hover", tokens.bg_hover)] {
                    let ratio = contrast_ratio(color, bg);
                    assert!(
                        ratio >= AA,
                        "{} {what} {color:?} on {ground}: {ratio:.2}",
                        name(&tokens)
                    );
                }
            }
        }
    }

    #[test]
    fn every_strip_glyph_and_focus_ring_meets_three_to_one() {
        for tokens in [DARK, LIGHT] {
            let mut marks = vec![("focus ring", tokens.accent)];
            for tone in [Tone::Red, Tone::Green, Tone::Amber, Tone::Neutral] {
                marks.push(("status glyph", tokens.status(tone).glyph));
            }
            for (what, color) in marks {
                for (ground, bg) in [("bg", tokens.bg), ("bg-hover", tokens.bg_hover)] {
                    let ratio = contrast_ratio(color, bg);
                    assert!(
                        ratio >= NON_TEXT,
                        "{} {what} on {ground}: {ratio:.2}",
                        name(&tokens)
                    );
                }
            }
        }
    }

    #[test]
    fn settings_text_meets_aa_on_raised_surfaces() {
        for tokens in [DARK, LIGHT] {
            for (what, color) in [
                ("text", tokens.text),
                ("text-muted", tokens.text_muted),
                ("error", tokens.error_text()),
                ("danger", tokens.priority),
            ] {
                let ratio = contrast_ratio(color, tokens.bg_raised);
                assert!(ratio >= AA, "{} {what}: {ratio:.2}", name(&tokens));
            }
            // A primary button's label (light only: dark is documented at
            // 3.9 : 1 and kept to 13 px medium or larger).
            if !tokens.dark {
                assert!(contrast_ratio(tokens.accent_text, tokens.accent) >= AA);
            }
        }
    }

    #[test]
    fn amber_stays_amber() {
        // The in-flight glyph keeps an orange hue in both themes.
        for tokens in [DARK, LIGHT] {
            let glyph = tokens.status(Tone::Amber).glyph;
            assert!(glyph.r() > glyph.g() && glyph.g() > glyph.b(), "{glyph:?}");
        }
    }

    #[test]
    fn theme_selection_and_mix() {
        assert_eq!(Tokens::for_theme(Theme::Dark, Some(false)), DARK);
        assert_eq!(Tokens::for_theme(Theme::Light, Some(true)), LIGHT);
        assert_eq!(Tokens::for_theme(Theme::System, Some(false)), LIGHT);
        assert_eq!(Tokens::for_theme(Theme::System, None), DARK);
        assert_eq!(DARK.egui_theme(), egui::Theme::Dark);
        assert_eq!(LIGHT.egui_theme(), egui::Theme::Light);
        assert_eq!(mix(Color32::WHITE, Color32::BLACK, 1.0), Color32::WHITE);
        assert_eq!(mix(Color32::WHITE, Color32::BLACK, 0.0), Color32::BLACK);
        assert_eq!(
            mix(Color32::WHITE, Color32::BLACK, 0.5),
            Color32::from_rgb(128, 128, 128)
        );
        assert_ne!(LIGHT.nav_current(), LIGHT.bg_raised);
        assert_ne!(DARK.accent_hover(), DARK.accent);
        assert_ne!(LIGHT.chosen_ring(), LIGHT.danger_hover());
        assert_eq!(DARK.nav_fill(), mix(DARK.bg_raised, DARK.bg, 0.7));
    }

    #[test]
    fn type_styles() {
        use type_style::*;
        assert_eq!(LABEL.size, 14.0);
        assert_eq!(LABEL.weight, Weight::Semibold);
        assert_eq!(PAGE_TITLE.weight, Weight::Bold);
        assert_eq!(HELP.size, 12.5);
        let job = GROUP_LABEL.job("Startup", Color32::RED);
        assert_eq!(job.text, "STARTUP");
        assert!((job.sections[0].format.extra_letter_spacing - 0.52).abs() < 1e-4);
        let wrapped = BODY.wrapped("x", Color32::RED, 100.0);
        assert_eq!(wrapped.wrap.max_width, 100.0);
        assert_eq!(BUTTON.font().family, Weight::Medium.family());
        assert_eq!(Weight::Regular.family(), FontFamily::Proportional);
        assert_eq!(META.format(Color32::RED).line_height, None);
        assert_eq!(HELP.format(Color32::RED).line_height, Some(12.5 * 1.4));
        for style in [
            HEADLINE,
            BRAND,
            SECTION_TITLE,
            CARD_TITLE,
            CHIP,
            SUBHEAD,
            STRIP_LABEL,
            STRIP_STATUS,
            STRIP_CAPTION,
        ] {
            assert!(style.size >= 11.0);
        }
    }
}
