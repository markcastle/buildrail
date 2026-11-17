//! Task Rail tokens applied to egui's own [`Style`], so the few stock
//! widgets BuildRail still uses (scroll bars, the drop-down list, the
//! slider, multi-line text) match the custom ones.

use eframe::egui::{
    self, style::Selection, style::WidgetVisuals, CornerRadius, FontFamily, FontId, Margin, Stroke,
    Style, TextStyle, Vec2,
};

use super::theme::{radius, space, type_style, Tokens};

/// Writes `tokens` into `style`: colours, corner radii, strokes, spacing
/// and text styles.
pub fn apply(style: &mut Style, tokens: &Tokens) {
    let t = tokens;
    let r9 = CornerRadius::same(radius::R9);
    let widget = |bg: egui::Color32, weak: egui::Color32, stroke: Stroke, fg| WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: weak,
        bg_stroke: stroke,
        corner_radius: r9,
        fg_stroke: Stroke::new(1.5, fg),
        expansion: 0.0,
    };
    let line = Stroke::new(1.0, t.line);

    let v = &mut style.visuals;
    v.dark_mode = t.dark;
    v.override_text_color = None;
    v.weak_text_color = Some(t.text_muted);
    v.widgets.noninteractive = widget(t.bg_raised, t.bg_raised, line, t.text);
    v.widgets.inactive = widget(t.bg_deep, t.bg, line, t.text);
    v.widgets.hovered = widget(t.bg_hover, t.bg_hover, line, t.text);
    v.widgets.active = widget(
        t.accent_soft,
        t.bg_hover,
        Stroke::new(1.0, t.accent),
        t.text,
    );
    v.widgets.open = widget(t.bg_hover, t.bg_hover, line, t.text);
    v.selection = Selection {
        bg_fill: t.accent_soft,
        stroke: Stroke::new(2.0, t.accent),
    };
    v.hyperlink_color = t.accent;
    v.faint_bg_color = t.bg_deep;
    v.extreme_bg_color = t.bg_deep;
    v.text_edit_bg_color = Some(t.bg_deep);
    v.code_bg_color = t.bg_deep;
    v.warn_fg_color = t.chip_high_fg;
    v.error_fg_color = t.error_text();
    v.window_corner_radius = CornerRadius::same(radius::R12);
    v.window_shadow = t.card_shadow[1].egui();
    v.popup_shadow = t.card_shadow[1].egui();
    v.window_fill = t.bg_raised;
    v.window_stroke = line;
    v.menu_corner_radius = CornerRadius::same(radius::R10);
    v.panel_fill = t.bg;
    v.text_cursor.stroke = Stroke::new(2.0, t.accent);
    v.slider_trailing_fill = true;
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;

    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(space::S8, space::S8);
    s.button_padding = Vec2::new(space::S14, 7.0);
    s.interact_size = Vec2::new(40.0, 30.0);
    s.window_margin = Margin::same(space::S12 as i8);
    s.menu_margin = Margin::same(space::S6 as i8);
    s.combo_height = 240.0;
    s.slider_rail_height = 4.0;
    s.text_edit_width = 320.0;
    s.scroll = egui::style::ScrollStyle::thin();

    style.text_styles = [
        (TextStyle::Small, type_style::META.font()),
        (TextStyle::Body, type_style::BODY.font()),
        (TextStyle::Button, type_style::BUTTON.font()),
        (TextStyle::Heading, type_style::PAGE_TITLE.font()),
        (
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
    ]
    .into();
}

/// Applies `tokens` to the context's style for their theme and selects
/// that theme.
pub fn install(ctx: &egui::Context, tokens: &Tokens) {
    let theme = tokens.egui_theme();
    ctx.style_mut_of(theme, |style| apply(style, tokens));
    ctx.set_theme(theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{DARK, LIGHT};

    #[test]
    fn tokens_reach_egui_visuals() {
        for tokens in [LIGHT, DARK] {
            let mut style = Style::default();
            apply(&mut style, &tokens);
            assert_eq!(style.visuals.panel_fill, tokens.bg);
            assert_eq!(style.visuals.window_fill, tokens.bg_raised);
            assert_eq!(
                style.visuals.selection.stroke,
                Stroke::new(2.0, tokens.accent)
            );
            assert_eq!(style.visuals.widgets.hovered.weak_bg_fill, tokens.bg_hover);
            assert_eq!(style.visuals.dark_mode, tokens.dark);
            assert_eq!(
                style.text_styles[&TextStyle::Button],
                type_style::BUTTON.font()
            );
        }
        let ctx = egui::Context::default();
        install(&ctx, &LIGHT);
        assert_eq!(
            ctx.style_of(egui::Theme::Light).visuals.panel_fill,
            LIGHT.bg
        );
    }
}
