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
            if page == Page::About {
                h.state().controller().check_for_updates();
                step_until(&mut h, "answer", |h| {
                    h.query_by_label("Up to date.").is_some()
                });
            }
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

/// The strip offering 0.3.0, its card on its own, and About in its
/// update states.
#[test]
#[ignore = "renders screenshots; run with --ignored"]
fn updates() {
    use buildrail_app::fakes::ScriptedUpdateService;
    use buildrail_app::ui::strip::update_card;

    for (theme, name) in THEMES {
        // The strip with the Update chip at rest (its card is shot below).
        let f = Fixture::new();
        f.world
            .updates
            .answer(ScriptedUpdateService::available("0.3.0"));
        let app = with_system_font(demo_ui(&f, theme));
        app.controller().check_for_updates();
        let mut h = Harness::builder()
            .with_size([1280.0, 24.0])
            .with_pixels_per_point(PPP)
            .renderer(CpuRenderer::default())
            .build_ui_state(root_ui, app);
        let chip = "BuildRail 0.3.0 is available. Open update options";
        step_until(&mut h, "demo and chip", |h| {
            demo_loaded(h) && h.query_by_label(chip).is_some()
        });
        shoot(&mut h, &format!("strip-update-{name}.png"));

        // Its card, on its own.
        let offer = h.state().controller().update_chip().unwrap();
        let tokens = Tokens::for_theme(theme, None);
        let mut card = Harness::builder()
            .with_size([320.0, 110.0])
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
                    update_card(&mut child, &offer, &tokens);
                },
                false,
            );
        for _ in 0..4 {
            card.step();
        }
        save_png(
            &card.render().unwrap(),
            &screenshot_path(&format!("strip-update-card-{name}.png")),
        );

        // About: an update available, then a build that does not update
        // itself.
        let f = Fixture::new();
        f.world
            .updates
            .answer(ScriptedUpdateService::available("0.3.0"));
        let mut app = with_system_font(demo_ui(&f, theme));
        app.settings_mut().page = Page::About;
        app.controller().check_for_updates();
        let mut h = settings_harness(app, 640.0);
        step_until(&mut h, "offer", |h| {
            h.query_by_label("Install now").is_some()
        });
        shoot(&mut h, &format!("settings-about-update-{name}.png"));
        f.world
            .updates
            .answer(buildrail_update::UpdateStatus::NotConfigured);
        h.state().controller().check_for_updates();
        step_until(&mut h, "not configured", |h| {
            h.query_by_label("Open releases page").is_some()
        });
        shoot(&mut h, &format!("settings-about-not-configured-{name}.png"));
    }
}

/// The details panel on its own, in four states, from canned data.
mod panel {
    use buildrail_app::app::{DetailsError, DetailsLoad};
    use buildrail_app::presentation::{DetailsInput, DetailsModel};
    use buildrail_app::ui::details::{details_panel, PanelState, PANEL_WIDTH};
    use buildrail_core::details::{JobSummary, RunDetails, RunRef, Trigger};
    use buildrail_core::domain::{
        BuildRecord, BuildStatus, ConnectionAlias, Freshness, RunId, Watch, WatchId, WatchSnapshot,
        WatchTarget,
    };
    use buildrail_core::ports::ProviderError;
    use chrono::{FixedOffset, TimeDelta};

    use super::*;

    fn watch() -> Watch {
        Watch {
            id: WatchId::new("keevoro"),
            label: "Keevoro".into(),
            connection: ConnectionAlias::new("work"),
            target: WatchTarget::GitHub {
                owner: "acme".into(),
                repo: "keevoro".into(),
                workflow: "ci.yml".into(),
                branch: Some("main".into()),
            },
        }
    }

    fn run(id: &str, status: BuildStatus, minutes_ago: i64, took: i64) -> BuildRecord {
        let created = t0() - TimeDelta::minutes(minutes_ago);
        BuildRecord {
            run_id: RunId::new(id),
            attempt: 1,
            status,
            url: format!("https://github.com/acme/keevoro/actions/runs/{id}"),
            branch: Some("main".into()),
            commit: Some("a7c412e9f0b1c2d3e4f5".into()),
            failing_job: None,
            created_at: created,
            updated_at: created + TimeDelta::seconds(took),
            observed_at: t0(),
        }
    }

    fn job(
        name: &str,
        status: BuildStatus,
        start: i64,
        secs: Option<i64>,
        step: Option<&str>,
    ) -> JobSummary {
        let started = t0() - TimeDelta::seconds(start);
        JobSummary {
            name: name.into(),
            status,
            started_at: (status != BuildStatus::Queued).then_some(started),
            finished_at: secs.map(|s| started + TimeDelta::seconds(s)),
            url: Some(format!(
                "https://github.com/acme/keevoro/actions/runs/7/job/{name}"
            )),
            failing_step: step.map(str::to_owned),
        }
    }

    fn history(current: &BuildRecord) -> Vec<BuildRecord> {
        vec![
            current.clone(),
            run("6", BuildStatus::Passed, 95, 240),
            run("5", BuildStatus::Passed, 180, 251),
            run("4", BuildStatus::Failed, 260, 200),
            run("3", BuildStatus::Passed, 400, 245),
            run("2", BuildStatus::Passed, 1_500, 239),
        ]
    }

    /// (file name, snapshot, details, recent, load)
    /// One shot: file name, snapshot, details, recent runs, load state.
    type State = (
        &'static str,
        WatchSnapshot,
        Option<RunDetails>,
        Option<Vec<BuildRecord>>,
        DetailsLoad,
    );

    fn states() -> Vec<State> {
        let w = watch();
        let snapshot = |record: BuildRecord| WatchSnapshot {
            watch_id: w.id.clone(),
            record: Some(record),
            freshness: Freshness::Fresh,
            last_success: Some(t0()),
        };

        let failed = run("7", BuildStatus::Failed, 14, 252);
        let mut failed_details = RunDetails::new(RunRef::from(&failed));
        failed_details.commit_message =
            Some("Fix the flaky login test on Windows\n\nRetries the socket.".into());
        failed_details.actor = Some("octocat".into());
        failed_details.trigger = Trigger::Push;
        failed_details.run_number = Some(562);
        failed_details.started_at = Some(failed.created_at + TimeDelta::seconds(8));
        failed_details.finished_at = Some(failed.updated_at);
        let start = (t0() - failed.created_at).num_seconds() - 10;
        failed_details.jobs = Some(vec![
            job("lint", BuildStatus::Passed, start, Some(48), None),
            job(
                "test (ubuntu-latest)",
                BuildStatus::Failed,
                start,
                Some(175),
                Some("cargo test --workspace"),
            ),
            job(
                "test (windows-latest)",
                BuildStatus::Passed,
                start,
                Some(236),
                None,
            ),
            job(
                "test (macos-latest)",
                BuildStatus::Passed,
                start,
                Some(201),
                None,
            ),
            job("build", BuildStatus::Skipped, start, None, None),
        ]);
        failed_details.jobs_total = 5;

        let mut running = run("8", BuildStatus::Running, 3, 150);
        running.attempt = 2;
        let mut running_details = RunDetails::new(RunRef::from(&running));
        running_details.commit_message = Some("Show the invoice total in the header".into());
        running_details.actor = Some("octocat".into());
        running_details.triggered_by = Some("mona".into());
        running_details.trigger = Trigger::Push;
        running_details.run_number = Some(563);
        running_details.started_at = Some(running.created_at + TimeDelta::seconds(5));
        running_details.jobs = Some(vec![
            job("lint", BuildStatus::Passed, 170, Some(44), None),
            job(
                "test (ubuntu-latest)",
                BuildStatus::Running,
                120,
                None,
                None,
            ),
            job(
                "test (windows-latest)",
                BuildStatus::Running,
                118,
                None,
                None,
            ),
            job("build", BuildStatus::Queued, 0, None, None),
        ]);
        running_details.jobs_total = 4;

        vec![
            (
                "failed",
                snapshot(failed.clone()),
                Some(failed_details),
                Some(history(&failed)),
                DetailsLoad::Loaded,
            ),
            (
                "running",
                snapshot(running.clone()),
                Some(running_details),
                Some(history(&running)),
                DetailsLoad::Loaded,
            ),
            (
                "loading",
                snapshot(failed.clone()),
                None,
                None,
                DetailsLoad::Loading,
            ),
            (
                "load-failed",
                snapshot(failed),
                None,
                None,
                DetailsLoad::Failed(DetailsError::Provider(
                    buildrail_core::domain::ProviderKind::GitHub,
                    ProviderError::Network("dns".into()),
                )),
            ),
        ]
    }

    fn render(model: &DetailsModel, tokens: Tokens, height: f32) -> (image::RgbaImage, f32) {
        let model = model.clone();
        let margin = 12.0;
        let mut h = Harness::builder()
            .with_size([PANEL_WIDTH + 2.0 * margin, height])
            .with_pixels_per_point(PPP)
            .renderer(CpuRenderer::default())
            .build_ui_state(
                move |ui, state: &mut (bool, PanelState)| {
                    if !state.0 {
                        state.0 = true;
                        fonts::install(ui.ctx(), &SystemFontSource, &Platform::Linux);
                        style::install(ui.ctx(), &tokens);
                    }
                    let full = ui.ctx().content_rect();
                    ui.painter().rect_filled(full, 0.0, tokens.bg);
                    let mut child =
                        ui.new_child(egui::UiBuilder::new().max_rect(full.shrink(margin)));
                    details_panel(&mut child, &model, &tokens, &mut state.1);
                },
                (false, PanelState::default()),
            );
        for _ in 0..4 {
            h.step();
        }
        let measured = h.state().1.height + 2.0 * margin;
        (h.render().unwrap(), measured)
    }

    #[test]
    #[ignore = "renders screenshots; run with --ignored"]
    fn details_panel_states() {
        let w = watch();
        let utc = FixedOffset::east_opt(0).unwrap();
        for (theme, name) in THEMES {
            let tokens = Tokens::for_theme(theme, None);
            for (state, snapshot, details, recent, load) in states() {
                let model = DetailsModel::build(DetailsInput {
                    watch: &w,
                    snapshot: &snapshot,
                    details: details.as_ref(),
                    recent: recent.as_deref(),
                    load: &load,
                    now: t0(),
                    offset: utc,
                });
                // Measure, then shoot at exactly the card's height.
                let (_, height) = render(&model, tokens, 900.0);
                let (image, _) = render(&model, tokens, height.ceil());
                save_png(
                    &image,
                    &screenshot_path(&format!("details-{state}-{name}.png")),
                );
            }
        }
    }

    /// The panel in place under the demo strip (as from a strip docked at
    /// the top), opened with a click.
    #[test]
    #[ignore = "renders screenshots; run with --ignored"]
    fn details_panel_on_the_strip() {
        for (theme, name) in THEMES {
            let f = Fixture::new();
            let app = with_system_font(demo_ui(&f, theme));
            let mut h = Harness::builder()
                .with_size([1280.0, 760.0])
                .with_pixels_per_point(PPP)
                .renderer(CpuRenderer::default())
                .build_ui_state(root_ui, app);
            step_until(&mut h, "demo loaded", demo_loaded);
            h.get_by_label_contains("Keevoro, GitHub").click();
            step_until(&mut h, "details", |h| {
                h.query_by_label_contains("Trigger: ").is_some()
                    && h.query_by_label_contains("of the last").is_some()
            });
            h.get_by_label_contains("Keevoro, GitHub").hover();
            shoot(&mut h, &format!("strip-panel-{name}.png"));
        }
    }
}
