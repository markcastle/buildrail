//! Renders the strip, its detail card, every Settings page and the main
//! Add-build steps, in both themes at 2× pixels per point, with the CPU
//! renderer in `support`. Ignored by default (it renders about twenty
//! large images); run it with
//!
//! ```text
//! BUILDRAIL_WRITE_SCREENSHOT=1 cargo test -p buildrail-app --test screenshots -- --ignored
//! ```
//!
//! to refresh `docs/images/`. The interface font is looked up as on Linux
//! (Inter, Noto Sans or DejaVu Sans), so the images show a real system
//! face rather than egui's bundled one.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;

use buildrail_app::presentation::StripEntry;
use buildrail_app::ui::fonts::{self, Platform, SystemFontSource};
use buildrail_app::ui::settings::{DeviceInfo, MonitorInfo, Page};
use buildrail_app::ui::strip::detail_card;
use buildrail_app::ui::{style, BuildRailUi, Tokens};
use buildrail_core::domain::BuildStatus;
use buildrail_profile::Theme;
use eframe::egui;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use support::*;

const PPP: f32 = 2.0;
const THEMES: [(Theme, &str); 2] = [(Theme::Light, "light"), (Theme::Dark, "dark")];

fn with_system_font(mut app: BuildRailUi) -> BuildRailUi {
    app.set_font_source(Arc::new(SystemFontSource), Platform::Linux);
    app
}

fn settings_harness(app: BuildRailUi, height: f32) -> Harness<'static, BuildRailUi> {
    Harness::builder()
        .with_size([880.0, height])
        .with_pixels_per_point(PPP)
        .renderer(CpuRenderer::default())
        .build_ui_state(
            |ui, app: &mut BuildRailUi| {
                let full = ui.ctx().content_rect();
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full));
                app.settings_ui(&mut child);
            },
            app,
        )
}

fn settle(h: &mut Harness<'_, BuildRailUi>) {
    for _ in 0..4 {
        h.step();
    }
}

fn shoot(h: &mut Harness<'_, BuildRailUi>, name: &str) {
    settle(h);
    let image = h.render().unwrap();
    save_png(&image, &screenshot_path(name));
}

fn click(h: &mut Harness<'_, BuildRailUi>, label: &str) {
    h.query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("no {label:?}"))
        .click_accesskit();
    settle(h);
}

#[test]
#[ignore = "renders screenshots; run with --ignored"]
fn strip_and_detail_card() {
    for (theme, name) in THEMES {
        let f = Fixture::new();
        let app = with_system_font(demo_ui(&f, theme));
        let mut h = Harness::builder()
            .with_size([1280.0, 24.0])
            .with_pixels_per_point(PPP)
            .renderer(CpuRenderer::default())
            .build_ui_state(root_ui, app);
        step_until(&mut h, "demo loaded", demo_loaded);
        assert!(h.state().font_family().is_some(), "a system font is used");
        h.get_by_label_contains("Billing, GitHub").hover();
        shoot(&mut h, &format!("strip-{name}.png"));

        // The detail card of a failed build, on its own.
        let entry: StripEntry = h
            .state()
            .controller()
            .strip()
            .entries
            .into_iter()
            .find(|e| e.label == "Keevoro")
            .unwrap();
        let tokens = Tokens::for_theme(theme, None);
        let mut card = Harness::builder()
            .with_size([320.0, 230.0])
            .with_pixels_per_point(PPP)
            .renderer(CpuRenderer::default())
            .build_ui_state(
                move |ui, installed: &mut bool| {
                    if !*installed {
                        *installed = true;
                        fonts::install(ui.ctx(), &SystemFontSource, &Platform::Linux);
                        style::install(ui.ctx(), &tokens);
                    }
                    let full = ui.ctx().content_rect();
                    ui.painter().rect_filled(full, 0.0, tokens.bg);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full));
                    detail_card(&mut child, &entry, &tokens);
                },
                false,
            );
        for _ in 0..4 {
            card.step();
        }
        save_png(
            &card.render().unwrap(),
            &screenshot_path(&format!("strip-details-{name}.png")),
        );
    }
}

#[test]
#[ignore = "renders screenshots; run with --ignored"]
fn settings_pages() {
    for (theme, name) in THEMES {
        let f = Fixture::new();
        let mut app = with_system_font(demo_ui(&f, theme));
        app.set_device_info(DeviceInfo {
            monitors: vec![
                MonitorInfo {
                    id: "1".into(),
                    name: "Display 1".into(),
                    width: 2560,
                    height: 1440,
                    primary: true,
                },
                MonitorInfo {
                    id: "2".into(),
                    name: "Display 2".into(),
                    width: 3440,
                    height: 1440,
                    primary: false,
                },
            ],
            placement_note: None,
        });
        let mut h = settings_harness(app, 640.0);
        step_until(&mut h, "demo loaded", |h| {
            h.state()
                .controller()
                .strip()
                .entries
                .iter()
                .all(|e| e.status_text != "Checking…")
        });
        for (page, file) in [
            (Page::Builds, "builds"),
            (Page::Connections, "connections"),
            (Page::Strip, "strip"),
            (Page::Share, "share"),
            (Page::About, "about"),
        ] {
            h.state_mut().settings_mut().page = page;
            shoot(&mut h, &format!("settings-{file}-{name}.png"));
        }
    }
}

#[test]
#[ignore = "renders screenshots; run with --ignored"]
fn wizard_steps() {
    for (theme, name) in THEMES {
        let f = Fixture::new();
        let prefs = buildrail_profile::DevicePrefs {
            theme,
            ..Default::default()
        };
        f.paths.device_prefs_store().save(&prefs).unwrap();
        f.world.github.set(
            "keevoro",
            Ok(vec![record("5", 1, BuildStatus::Passed, 3, "keevoro")]),
        );
        let app = with_system_font(f.ui());
        let mut h = settings_harness(app, 900.0);
        shoot(&mut h, &format!("wizard-choose-{name}.png"));
        click(&mut h, "GitHub Actions workflow");
        shoot(&mut h, &format!("wizard-connect-{name}.png"));
        type_into(&mut h, "Token", "ghp_test_token");
        click(&mut h, "Connect");
        step_until(&mut h, "repositories", |h| {
            h.query_by_label("acme/keevoro").is_some()
        });
        shoot(&mut h, &format!("wizard-repository-{name}.png"));
        click(&mut h, "acme/keevoro");
        step_until(&mut h, "workflows", |h| {
            h.query_by_label_contains("CI, ci.yml").is_some()
        });
        click(&mut h, "CI, ci.yml");
        step_until(&mut h, "branches", |h| {
            h.query_by_label("develop").is_some()
        });
        shoot(&mut h, &format!("wizard-branch-{name}.png"));
        click(&mut h, "Check latest run");
        step_until(&mut h, "preview", |h| {
            h.query_by_label("Use this build").is_some()
        });
        shoot(&mut h, &format!("wizard-preview-{name}.png"));
    }
}
