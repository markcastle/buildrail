//! [`BuildRailUi`]: one frame of the whole UI (strip + Settings viewport),
//! independent of eframe so that `egui_kittest` can drive it exactly as
//! the real window does.

use eframe::egui::{self, Ui, ViewportBuilder, ViewportId};

use super::settings::{DeviceInfo, SettingsWindow};
use super::strip::{StripAction, StripView};
use super::theme::Palette;
use crate::app::AppController;

/// What the host window must do after a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameOutput {
    /// The user started dragging the floating strip.
    pub start_drag: bool,
    /// Placement, monitor or scale preferences changed.
    pub placement_changed: bool,
}

/// The strip and its Settings window.
pub struct BuildRailUi {
    controller: AppController,
    strip: StripView,
    settings: SettingsWindow,
    settings_open: bool,
    device: DeviceInfo,
    movable: bool,
    last_error: Option<String>,
    theme: Option<egui::Theme>,
}

impl BuildRailUi {
    /// Wraps a started controller. Settings opens on Add build when nothing
    /// is pinned yet (first launch).
    pub fn new(controller: AppController) -> Self {
        let mut settings = SettingsWindow::new();
        let first_launch = controller.profile().is_empty() && controller.profile_error().is_none();
        if first_launch {
            settings.open_add_build();
        }
        Self {
            controller,
            strip: StripView::new(),
            settings,
            settings_open: first_launch,
            device: DeviceInfo::default(),
            movable: false,
            last_error: None,
            theme: None,
        }
    }

    /// The controller.
    pub fn controller(&self) -> &AppController {
        &self.controller
    }

    /// The controller, mutably.
    pub fn controller_mut(&mut self) -> &mut AppController {
        &mut self.controller
    }

    /// The Settings window state.
    pub fn settings(&self) -> &SettingsWindow {
        &self.settings
    }

    /// `true` while Settings is shown.
    pub fn is_settings_open(&self) -> bool {
        self.settings_open
    }

    /// Opens or closes Settings.
    pub fn set_settings_open(&mut self, open: bool) {
        self.settings_open = open;
    }

    /// The last error from an action on the strip (e.g. opening a link).
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Facts for the Device tab.
    pub fn set_device_info(&mut self, device: DeviceInfo) {
        self.device = device;
    }

    /// Whether the strip may be dragged (floating window).
    pub fn set_movable(&mut self, movable: bool) {
        self.movable = movable;
    }

    /// Turns the running-spinner animation on or off (tests turn it off so
    /// frames settle).
    pub fn set_animate(&mut self, animate: bool) {
        self.strip.animate = animate;
    }

    /// The palette for the current preferences and OS theme.
    pub fn palette(&self, ctx: &egui::Context) -> Palette {
        let system_dark = ctx.system_theme().map(|t| t == egui::Theme::Dark);
        Palette::for_theme(self.controller.prefs().theme, system_dark)
    }

    /// Runs one frame in `ui` (the root viewport's UI).
    pub fn ui(&mut self, ui: &mut Ui) -> FrameOutput {
        let ctx = ui.ctx().clone();
        let mut output = FrameOutput::default();

        let events = self.controller.poll_events();
        for command in self.settings.handle_events(&events) {
            self.controller.execute(command);
        }

        let palette = self.palette(&ctx);
        if self.theme != Some(palette.egui_theme()) {
            self.theme = Some(palette.egui_theme());
            ctx.set_theme(palette.egui_theme());
        }

        let model = self.controller.strip();
        for action in self.strip.show(ui, &model, &palette, self.movable) {
            match action {
                StripAction::Open(url) => {
                    self.last_error = self.controller.open_url(&url).err();
                }
                StripAction::OpenSettings => self.settings_open = true,
                StripAction::AddBuild => {
                    self.settings.open_add_build();
                    self.settings_open = true;
                }
                StripAction::Reconnect(watch) => {
                    self.settings.open_reconnect(&watch, &self.controller);
                    self.settings_open = true;
                }
                StripAction::StartDrag => output.start_drag = true,
            }
        }

        if self.settings_open {
            let builder = ViewportBuilder::default()
                .with_title("BuildRail Settings")
                .with_app_id("buildrail-settings")
                .with_inner_size([680.0, 560.0])
                .with_min_inner_size([420.0, 320.0]);
            let settings = &mut self.settings;
            let controller = &mut self.controller;
            let device = &self.device;
            let mut close = false;
            let outcome = ctx.show_viewport_immediate(
                ViewportId::from_hash_of("buildrail-settings"),
                builder,
                |ui, class| {
                    if ui.input(|i| i.viewport().close_requested()) {
                        close = true;
                    }
                    let frame = if class == egui::ViewportClass::EmbeddedWindow {
                        egui::Frame::new()
                    } else {
                        egui::Frame::central_panel(ui.style())
                    };
                    frame
                        .show(ui, |ui| settings.ui(ui, controller, device))
                        .inner
                },
            );
            output.placement_changed = outcome.placement_changed;
            if close {
                self.settings_open = false;
            }
        }
        output
    }
}
