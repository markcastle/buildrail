//! The eframe glue: native window options and the `eframe::App` that
//! connects [`BuildRailUi`] to the real window and the shell thread.
//!
//! This is the only code that needs a real window, so it is kept to
//! translation: shell updates → viewport commands, window handle → dock,
//! window moves → saved floating position. Decisions live in `shell`,
//! `presentation::repaint` and `ui::app`.
//!
//! It never asks for frames on a timer. The shell thread wakes it when the
//! strip must move, hide or show; the controller when data changes; the
//! [`RepaintPlanner`](crate::presentation::RepaintPlanner) (inside
//! [`BuildRailUi`]) when the words on the strip next change.

use std::sync::Arc;

use buildrail_platform::dock::{raw_window_handle_of, ReservationGuarantee, WindowRef};
use buildrail_platform::geometry::STRIP_HEIGHT_LOGICAL;
use buildrail_platform::geometry::{MonitorInfo as PlatformMonitor, PhysicalPosition};
use buildrail_profile::Placement;
use eframe::egui::{self, Pos2, Vec2, ViewportBuilder, ViewportCommand};
use raw_window_handle::RawWindowHandle;

use super::app::BuildRailUi;
use super::settings::{DeviceInfo, MonitorInfo};
use crate::shell::{DockController, ShellCommand, ShellMode, ShellUpdate, ShellWorker, FLOAT_SIZE};

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
        .with_icon(Arc::new(super::icon::window_icon()))
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

/// The sentence Settings shows about the current placement.
pub fn placement_note(mode: &ShellMode) -> Option<String> {
    match mode {
        ShellMode::Docked { guarantee, .. } => Some(
            match guarantee {
                ReservationGuarantee::Enforced => {
                    "Docked above the taskbar. Maximised windows stop above the strip."
                }
                ReservationGuarantee::BestEffort => {
                    "Docked along the bottom edge. Your window manager decides whether maximised windows stop above it."
                }
            }
            .to_owned(),
        ),
        ShellMode::Floating { reason, .. } => Some(reason.explanation()),
        ShellMode::NotStarted => None,
    }
}

/// The Settings display list for `monitors`.
pub fn monitor_list(monitors: &[PlatformMonitor]) -> Vec<MonitorInfo> {
    monitors
        .iter()
        .enumerate()
        .map(|(index, m)| MonitorInfo {
            id: m.id.to_string(),
            name: format!("Display {}", index + 1),
            width: m.bounds.width,
            height: m.bounds.height,
            primary: m.is_primary,
        })
        .collect()
}

/// The eframe application.
pub struct StripWindow {
    ui: BuildRailUi,
    shell: Option<ShellWorker>,
    make_shell: Option<ShellFactory>,
    mode: ShellMode,
    monitors: Vec<PlatformMonitor>,
    settings_was_open: bool,
    last_outer: Option<Pos2>,
}

impl StripWindow {
    /// A window around `ui`; the shell is built on the first frame.
    pub fn new(ui: BuildRailUi, make_shell: ShellFactory) -> Self {
        Self {
            ui,
            shell: None,
            make_shell: Some(make_shell),
            mode: ShellMode::NotStarted,
            monitors: Vec::new(),
            settings_was_open: false,
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

    fn start_shell(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(make) = self.make_shell.take() else {
            return;
        };
        let handle = match raw_window_handle_of(frame) {
            Ok(handle) => handle,
            Err(error) => {
                tracing::warn!(%error, "no native window handle; not docking");
                ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                return;
            }
        };
        let shell = make(&handle, ctx);
        let wake = ctx.clone();
        match ShellWorker::spawn(
            shell,
            WindowRef::from_raw(&handle),
            self.ui.controller().prefs(),
            Arc::new(move || wake.request_repaint()),
        ) {
            Ok(worker) => {
                self.ui.controller().lifecycle().hold(worker.link());
                self.shell = Some(worker);
            }
            Err(error) => {
                tracing::warn!(%error, "could not start the shell thread; not docking");
                ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            }
        }
    }

    /// Applies what the shell thread sent since the last frame.
    fn take_updates(&mut self, ctx: &egui::Context) {
        let Some(shell) = &self.shell else { return };
        let mut changed = false;
        for update in shell.drain() {
            match update {
                ShellUpdate::Commands(commands) => Self::apply(ctx, commands),
                ShellUpdate::Mode(mode) => {
                    self.mode = mode;
                    changed = true;
                }
                ShellUpdate::Hidden(hidden) => self.ui.set_strip_visible(!hidden),
                ShellUpdate::Monitors(monitors) => {
                    self.monitors = monitors;
                    changed = true;
                }
                ShellUpdate::Moved(position) => {
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
        if changed {
            self.sync_shell_info();
        }
    }

    fn sync_shell_info(&mut self) {
        self.ui.set_movable(self.mode.is_movable());
        self.ui.set_device_info(DeviceInfo {
            monitors: monitor_list(&self.monitors),
            placement_note: placement_note(&self.mode),
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

    /// Tells the shell where the floating window was dropped.
    fn track_moves(&mut self, ctx: &egui::Context) {
        let Some(shell) = &self.shell else { return };
        if !self.mode.is_movable() {
            return;
        }
        let (outer, dragging) = ctx.input(|i| (i.viewport().outer_rect, i.pointer.any_down()));
        let Some(outer) = outer else { return };
        if dragging || self.last_outer == Some(outer.min) {
            return;
        }
        self.last_outer = Some(outer.min);
        let ppp = ctx.pixels_per_point();
        shell.window_moved(PhysicalPosition {
            x: (outer.min.x * ppp).round() as i32,
            y: (outer.min.y * ppp).round() as i32,
        });
    }
}

impl eframe::App for StripWindow {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.start_shell(ctx, frame);
        self.take_updates(ctx);
        self.track_moves(ctx);
        // Opening Settings: fetch the display list once.
        let open = self.ui.is_settings_open();
        if open && !self.settings_was_open {
            if let Some(shell) = &self.shell {
                shell.refresh_monitors();
            }
        }
        self.settings_was_open = open;
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
            if let Some(shell) = &self.shell {
                shell.apply(self.ui.controller().prefs());
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        tracing::info!("closing");
        // Stops refreshing, writes the snapshot cache one last time and
        // gives the screen edge back (and anything else held) before the
        // window is destroyed.
        self.ui.controller().close();
        self.shell = None;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use buildrail_platform::dock::UnsupportedReason;
    use buildrail_platform::geometry::{MonitorId, PhysicalRect};

    use super::*;
    use crate::shell::FloatReason;

    #[test]
    fn placement_notes_and_monitor_names() {
        let docked = |guarantee| ShellMode::Docked {
            rect: PhysicalRect::new(0, 0, 10, 10),
            guarantee,
        };
        assert!(placement_note(&docked(ReservationGuarantee::Enforced))
            .unwrap()
            .contains("taskbar"));
        assert!(placement_note(&docked(ReservationGuarantee::BestEffort))
            .unwrap()
            .contains("window manager"));
        let floating = ShellMode::Floating {
            rect: PhysicalRect::new(0, 0, 10, 10),
            reason: FloatReason::Unsupported(UnsupportedReason::Wayland),
        };
        assert!(placement_note(&floating).unwrap().contains("Wayland"));
        assert_eq!(placement_note(&ShellMode::NotStarted), None);

        let monitors = monitor_list(&[PlatformMonitor {
            id: MonitorId::new("DP-1"),
            bounds: PhysicalRect::new(0, 0, 1920, 1080),
            work_area: PhysicalRect::new(0, 0, 1920, 1040),
            scale_factor: 1.0,
            is_primary: true,
        }]);
        assert_eq!(monitors[0].name, "Display 1");
        assert!(monitors[0].primary);
        assert_eq!(monitors[0].width, 1920);

        let builder = strip_viewport(&Placement::DockedBottom);
        assert!(builder.icon.is_some());
    }
}
