//! [`BuildRailUi`]: one frame of the whole UI (strip + Settings viewport),
//! independent of eframe so that `egui_kittest` can drive it exactly as
//! the real window does.
//!
//! Each frame ends by scheduling the next one with
//! [`RepaintPlanner`]: when the words on the strip next change on their
//! own, or the next animation step when the user turned Animation on.
//! Nothing else here asks for frames, so an idle strip paints nothing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use eframe::egui::{self, Ui, ViewportBuilder, ViewportId};

use super::fonts::{self, FontSource, Platform, SystemFontSource};
use super::settings::{DeviceInfo, SettingsWindow};
use super::strip::{StripAction, StripView};
use super::style;
use super::theme::Tokens;
use crate::app::AppController;
use crate::presentation::repaint::animation_phase;
use crate::presentation::{Motion, RepaintPlanner};

/// Counts painted frames (UI passes). Cheap to clone and to bump; shared
/// with whoever reports the frame rate.
#[derive(Debug, Clone, Default)]
pub struct FrameCounter(Arc<AtomicU64>);

impl FrameCounter {
    /// A counter at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts one frame.
    pub fn record(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// Frames counted so far.
    pub fn total(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// What the host window must do after a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameOutput {
    /// The user started dragging the floating strip.
    pub start_drag: bool,
    /// Placement, monitor or scale preferences changed.
    pub placement_changed: bool,
    /// The user chose Quit BuildRail, or an installed update is
    /// restarting the app: close the window.
    pub quit: bool,
}

/// Where the interface font comes from.
struct FontSetup {
    source: Arc<dyn FontSource + Send + Sync>,
    platform: Platform,
    installed: bool,
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
    styled: Option<Tokens>,
    fonts: FontSetup,
    font_family: Option<&'static str>,
    frames: FrameCounter,
    /// Motion is allowed at all (tests and screenshots turn it off).
    motion_allowed: bool,
    /// The desktop asked for less motion.
    reduced_motion: bool,
    /// The strip is on screen (not hidden for a fullscreen app).
    strip_visible: bool,
    /// The delay the last frame asked for, for tests.
    next_repaint: Option<std::time::Duration>,
}

impl BuildRailUi {
    /// Wraps a started controller. Settings opens on Add build when nothing
    /// is pinned yet (first launch). The system font is looked for on the
    /// first frame.
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
            styled: None,
            fonts: FontSetup {
                source: Arc::new(SystemFontSource),
                platform: Platform::current(),
                installed: false,
            },
            font_family: None,
            frames: FrameCounter::new(),
            motion_allowed: true,
            reduced_motion: false,
            strip_visible: true,
            next_repaint: None,
        }
    }

    /// Looks for the interface font through `source` as if on `platform`
    /// (tests and screenshots point this at a font they know is there).
    /// Takes effect on the next frame.
    pub fn set_font_source(
        &mut self,
        source: Arc<dyn FontSource + Send + Sync>,
        platform: Platform,
    ) {
        self.fonts = FontSetup {
            source,
            platform,
            installed: false,
        };
    }

    /// The system font family in use, once installed (`None` = egui's own).
    pub fn font_family(&self) -> Option<&'static str> {
        self.font_family
    }

    /// The strip widget's state (which popups are open).
    pub fn strip_view(&self) -> &StripView {
        &self.strip
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

    /// The Settings window state, mutably (to open a page).
    pub fn settings_mut(&mut self) -> &mut SettingsWindow {
        &mut self.settings
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

    /// Facts for the Strip page.
    pub fn set_device_info(&mut self, device: DeviceInfo) {
        self.device = device;
    }

    /// Whether the strip may be dragged (floating window).
    pub fn set_movable(&mut self, movable: bool) {
        self.movable = movable;
    }

    /// Allows or forbids motion whatever the preference says (tests and
    /// screenshots forbid it so frames settle).
    pub fn set_animate(&mut self, animate: bool) {
        self.motion_allowed = animate;
    }

    /// Whether the desktop asked for less motion (see
    /// `buildrail_platform::motion`).
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    /// Whether the strip is on screen; a hidden strip never animates.
    pub fn set_strip_visible(&mut self, visible: bool) {
        self.strip_visible = visible;
    }

    /// The frame counter (shared; see [`FrameCounter`]).
    pub fn frame_counter(&self) -> FrameCounter {
        self.frames.clone()
    }

    /// How long after the last frame it asked to be painted again
    /// (`None`: not until something changes).
    pub fn next_repaint(&self) -> Option<std::time::Duration> {
        self.next_repaint
    }

    /// The planner for this frame's preferences and state.
    pub fn planner(&self) -> RepaintPlanner {
        RepaintPlanner::new(Motion {
            enabled: self.motion_allowed && self.controller.prefs().animation,
            reduced: self.reduced_motion,
            visible: self.strip_visible,
        })
    }

    /// The tokens for the current preferences and OS theme.
    pub fn tokens(&self, ctx: &egui::Context) -> Tokens {
        let system_dark = ctx.system_theme().map(|t| t == egui::Theme::Dark);
        Tokens::for_theme(self.controller.prefs().theme, system_dark)
    }

    /// Installs the font (once) and the tokens' style (when they change),
    /// handles controller events, and returns the tokens to paint with.
    fn begin_frame(&mut self, ctx: &egui::Context) -> Tokens {
        if !self.fonts.installed {
            self.fonts.installed = true;
            self.font_family =
                fonts::install(ctx, self.fonts.source.as_ref(), &self.fonts.platform);
            // The fonts apply from the next pass: do not show this one.
            ctx.request_discard("fonts installed");
        }
        let events = self.controller.poll_events();
        for command in self.settings.handle_events(&events) {
            self.controller.execute(command);
        }
        let tokens = self.tokens(ctx);
        if self.styled != Some(tokens) {
            self.styled = Some(tokens);
            style::install(ctx, &tokens);
        }
        tokens
    }

    /// Runs one frame in `ui` (the root viewport's UI).
    pub fn ui(&mut self, ui: &mut Ui) -> FrameOutput {
        let ctx = ui.ctx().clone();
        self.frames.record();
        let mut output = FrameOutput::default();
        let tokens = self.begin_frame(&ctx);
        // An installed update restarts the app: close cleanly.
        output.quit = self.controller.poll_update();
        self.controller
            .maybe_install_automatically(self.settings_open);
        self.strip.set_update(self.controller.update_chip());

        let now = self.controller.now();
        let model = self.controller.strip();
        let planner = self.planner();
        let phase = if planner.animating(&model) {
            animation_phase(ctx.input(|i| i.time))
        } else {
            0
        };
        self.next_repaint = planner.next_repaint(&model, now);
        if let Some(delay) = self.next_repaint {
            ctx.request_repaint_after(delay);
        }
        for action in self.strip.show(ui, &model, &tokens, self.movable, phase) {
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
                StripAction::InstallUpdate => self.controller.install_update(),
                StripAction::OpenUpdates => {
                    self.settings.page = super::settings::Page::About;
                    self.settings_open = true;
                }
                StripAction::UpdateNotes(url) => {
                    self.last_error = self.controller.open_url(&url).err();
                }
                StripAction::DismissUpdate => self.controller.dismiss_update(),
            }
        }

        if self.settings_open {
            let builder = ViewportBuilder::default()
                .with_title("BuildRail settings")
                .with_app_id("buildrail-settings")
                .with_inner_size([880.0, 640.0])
                .with_min_inner_size([640.0, 420.0]);
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
                    if class == egui::ViewportClass::EmbeddedWindow {
                        // A test harness embeds the viewport as a window:
                        // give the page a sensible size there.
                        ui.set_min_size(egui::vec2(820.0, 600.0));
                    }
                    settings.ui(ui, controller, device, &tokens)
                },
            );
            output.placement_changed = outcome.placement_changed;
            output.quit |= outcome.quit;
            if close || outcome.close {
                self.settings_open = false;
            }
        }
        output
    }

    /// Draws only the Settings window, filling `ui` (screenshots, and
    /// tests that look at one page).
    pub fn settings_ui(&mut self, ui: &mut Ui) -> FrameOutput {
        let ctx = ui.ctx().clone();
        let tokens = self.begin_frame(&ctx);
        let restarting = self.controller.poll_update();
        let outcome = self
            .settings
            .ui(ui, &mut self.controller, &self.device, &tokens);
        if outcome.close {
            self.settings_open = false;
        }
        FrameOutput {
            start_drag: false,
            placement_changed: outcome.placement_changed,
            quit: outcome.quit || restarting,
        }
    }
}
