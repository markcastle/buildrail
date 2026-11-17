//! The eframe glue: native window options and the `eframe::App` that
//! connects [`BuildRailUi`] to the real window and [`DockController`].
//!
//! This is the only code that needs a real window, so it is kept to
//! translation: shell commands → viewport commands, window handle → dock,
//! window moves → saved floating position. Decisions live in `shell` and
//! `ui::app`.

use std::time::{Duration, Instant};

use buildrail_platform::dock::raw_window_handle_of;
use buildrail_platform::geometry::{PhysicalPosition, STRIP_HEIGHT_LOGICAL};
use buildrail_profile::Placement;
use eframe::egui::{self, Pos2, Vec2, ViewportBuilder, ViewportCommand};
use raw_window_handle::RawWindowHandle;

use super::app::BuildRailUi;
use super::settings::{DeviceInfo, MonitorInfo};
use crate::shell::{DockController, ShellCommand, ShellMode, FLOAT_SIZE};

/// How often fullscreen and dock state are polled.
const TICK: Duration = Duration::from_millis(500);

/// Builds the dock controller once the native window exists (the
/// fullscreen probe needs its handle; monitor fallbacks need egui's view
/// of the screen).
pub type ShellFactory = Box<dyn FnOnce(&RawWindowHandle, &egui::Context) -> DockController>;

/// The root viewport: an undecorated, always-on-top, non-activating strip
/// that is not shown in the taskbar. Placement is applied after creation
/// by the dock (docked) or by [`ShellCommand::Place`] (floating).
pub fn strip_viewport(placement: &Placement) -> ViewportBuilder {
    let mut builder = ViewportBuilder::default()
        .with_title("BuildRail")
        .with_app_id("buildrail")
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_active(false)
        .with_inner_size([FLOAT_SIZE.width as f32, STRIP_HEIGHT_LOGICAL as f32])
        .with_min_inner_size([160.0, STRIP_HEIGHT_LOGICAL as f32]);
    if matches!(placement, Placement::DockedBottom) {
        // Some X11 window managers only read the type when a window is first
        // mapped; the dock sets it too, this makes it right from the start.
        builder = builder.with_window_type(egui::X11WindowType::Dock);
    }
    builder
}

/// The eframe application.
pub struct StripWindow {
    ui: BuildRailUi,
    shell: Option<DockController>,
    make_shell: Option<ShellFactory>,
    next_tick: Instant,
    last_outer: Option<Pos2>,
}

impl StripWindow {
    /// A window around `ui`; the shell is built on the first frame.
    pub fn new(ui: BuildRailUi, make_shell: ShellFactory) -> Self {
        Self {
            ui,
            shell: None,
            make_shell: Some(make_shell),
            next_tick: Instant::now(),
            last_outer: None,
        }
    }

    fn apply(ctx: &egui::Context, commands: Vec<ShellCommand>) {
        let ppp = ctx.pixels_per_point();
        for command in commands {
            tracing::debug!(?command, "shell");
            match command {
                ShellCommand::Show => ctx.send_viewport_cmd(ViewportCommand::Visible(true)),
                ShellCommand::Hide => ctx.send_viewport_cmd(ViewportCommand::Visible(false)),
                ShellCommand::Place(rect) => {
                    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(Pos2::new(
                        rect.x as f32 / ppp,
                        rect.y as f32 / ppp,
                    )));
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(
                        rect.width as f32 / ppp,
                        rect.height as f32 / ppp,
                    )));
                }
            }
        }
    }

    fn sync_shell_info(&mut self) {
        let Some(shell) = &self.shell else { return };
        self.ui.set_movable(shell.mode().is_movable());
        let placement_note = match shell.mode() {
            ShellMode::Docked { guarantee, .. } => Some(
                match guarantee {
                    buildrail_platform::dock::ReservationGuarantee::Enforced => {
                        "Docked above the taskbar. Maximised windows stop above the strip."
                    }
                    buildrail_platform::dock::ReservationGuarantee::BestEffort => {
                        "Docked along the bottom edge. Your window manager decides whether maximised windows stop above it."
                    }
                }
                .to_owned(),
            ),
            ShellMode::Floating { reason, .. } => Some(reason.explanation()),
            ShellMode::NotStarted => None,
        };
        let monitors = if self.ui.is_settings_open() {
            shell
                .monitors()
                .into_iter()
                .enumerate()
                .map(|(index, m)| MonitorInfo {
                    id: m.id.to_string(),
                    name: format!("Display {}", index + 1),
                    width: m.bounds.width,
                    height: m.bounds.height,
                    primary: m.is_primary,
                })
                .collect()
        } else {
            Vec::new()
        };
        self.ui.set_device_info(DeviceInfo {
            monitors,
            placement_note,
        });
    }

    fn apply_scale(&self, ctx: &egui::Context) {
        let native = ctx
            .input(|i| i.viewport().native_pixels_per_point)
            .unwrap_or(1.0);
        let zoom = self
            .ui
            .controller()
            .prefs()
            .scale
            .map_or(1.0, |scale| scale as f32 / native.max(0.1));
        if (ctx.zoom_factor() - zoom).abs() > f32::EPSILON {
            ctx.set_zoom_factor(zoom);
        }
    }
}

impl eframe::App for StripWindow {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let now = Instant::now();
        if let Some(make) = self.make_shell.take() {
            match raw_window_handle_of(&*frame) {
                Ok(handle) => {
                    let mut shell = make(&handle, ctx);
                    let commands = shell.start(handle, self.ui.controller().prefs(), now);
                    self.shell = Some(shell);
                    Self::apply(ctx, commands);
                }
                Err(error) => tracing::warn!(%error, "no native window handle; not docking"),
            }
            self.sync_shell_info();
        }
        if now >= self.next_tick {
            self.next_tick = now + TICK;
            if let Some(shell) = &mut self.shell {
                let commands = shell.tick(now);
                Self::apply(ctx, commands);
            }
            self.sync_shell_info();
        }
        // Remember where the user dropped the floating window.
        let (outer, dragging) = ctx.input(|i| (i.viewport().outer_rect, i.pointer.any_down()));
        if let (Some(shell), Some(outer)) = (&mut self.shell, outer) {
            if !dragging && self.last_outer != Some(outer.min) {
                self.last_outer = Some(outer.min);
                let ppp = ctx.pixels_per_point();
                let physical = PhysicalPosition {
                    x: (outer.min.x * ppp).round() as i32,
                    y: (outer.min.y * ppp).round() as i32,
                };
                if let Some(position) = shell.window_moved(physical) {
                    let result = self.ui.controller_mut().update_prefs(|prefs| {
                        prefs.placement = Placement::Floating {
                            position: Some(position),
                        };
                    });
                    if let Err(error) = result {
                        tracing::warn!(%error, "could not remember the window position");
                    }
                }
            }
        }
        ctx.request_repaint_after(TICK);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.apply_scale(ui.ctx());
        let output = self.ui.ui(ui);
        if output.start_drag {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if output.quit {
            ui.ctx()
                .send_viewport_cmd_to(egui::ViewportId::ROOT, ViewportCommand::Close);
        }
        if output.placement_changed {
            if let Some(shell) = &mut self.shell {
                let commands = shell.apply(self.ui.controller().prefs(), Instant::now());
                Self::apply(ui.ctx(), commands);
            }
            self.sync_shell_info();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(shell) = &mut self.shell {
            shell.shutdown();
        }
        self.ui.controller().shutdown();
    }
}
