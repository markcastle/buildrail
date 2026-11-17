//! The docking state machine.
//!
//! ```text
//!            start / apply(prefs)
//!   NotStarted ───────────────┬──▶ Docked { rect }      (placement = docked, dock reserved)
//!                             └──▶ Floating { reason }  (placement = floating, or the dock
//!                                                        said Unsupported / failed)
//!   tick(): Docked → dock.update() every DOCK_UPDATE_INTERVAL (Unsupported → Floating)
//!           any    → fullscreen probe on the strip's monitor → Hide / Show
//!   shutdown(): release the reserved edge
//! ```
//!
//! Positions of the floating window are persisted in logical pixels
//! relative to the chosen monitor's top-left corner
//! ([`buildrail_profile::Placement::Floating`]), so they survive scale
//! changes; [`DockController::window_moved`] converts.

use std::time::{Duration, Instant};

use buildrail_platform::dock::{
    DockError, DockEvent, DockOutcome, EdgeDock, ReservationGuarantee, UnsupportedReason,
};
use buildrail_platform::fullscreen::FullscreenProbe;
use buildrail_platform::geometry::{
    float_rect, resolve_monitor, LogicalSize, MonitorId, MonitorInfo, PhysicalPosition,
    PhysicalRect, STRIP_HEIGHT_LOGICAL,
};
use buildrail_platform::monitors::MonitorSource;
use buildrail_profile::{DevicePrefs, Placement, Position};
use raw_window_handle::RawWindowHandle;

/// Size of the compact movable window.
pub const FLOAT_SIZE: LogicalSize = LogicalSize {
    width: 760,
    height: STRIP_HEIGHT_LOGICAL,
};

/// How often a docked strip re-applies its reservation (taskbar moved,
/// resolution changed). The platform guide asks for a polling fallback of
/// "every few seconds".
pub const DOCK_UPDATE_INTERVAL: Duration = Duration::from_secs(3);

/// What the toolkit layer must do to the strip window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCommand {
    /// Make the window visible.
    Show,
    /// Hide the window (a fullscreen app covers its monitor).
    Hide,
    /// Move and size the floating window (physical pixels, desktop
    /// coordinates). Never sent while docked: the dock moves the window
    /// itself.
    Place(PhysicalRect),
}

/// Why the strip is floating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloatReason {
    /// The user chose the floating window.
    Chosen,
    /// This desktop cannot reserve an edge.
    Unsupported(UnsupportedReason),
    /// Docking failed; the message is for Settings and logs.
    Failed(String),
}

impl FloatReason {
    /// A sentence for Settings.
    pub fn explanation(&self) -> String {
        match self {
            Self::Chosen => {
                "Floating, so you can drag the strip anywhere by its “Builds” caption.".into()
            }
            Self::Unsupported(reason) => {
                // The platform words it as a clause; Settings shows sentences.
                let text = reason.to_string();
                let mut chars = text.chars();
                let first = chars.next().map(|c| c.to_uppercase().to_string());
                format!("{}{}.", first.unwrap_or_default(), chars.as_str())
            }
            Self::Failed(message) => {
                format!("The strip could not dock ({message}), so it floats instead. You can drag it by its caption.")
            }
        }
    }
}

/// Current placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellMode {
    /// [`DockController::start`] not called yet.
    NotStarted,
    /// Docked along the bottom edge.
    Docked {
        /// The reserved strip rect.
        rect: PhysicalRect,
        /// Enforced by the OS or best effort.
        guarantee: ReservationGuarantee,
    },
    /// The compact movable window.
    Floating {
        /// Its rect when last placed.
        rect: PhysicalRect,
        /// Why it floats.
        reason: FloatReason,
    },
}

impl ShellMode {
    /// `true` when the user may drag the window.
    pub fn is_movable(&self) -> bool {
        matches!(self, Self::Floating { .. })
    }
}

/// See the module docs.
pub struct DockController {
    dock: Box<dyn EdgeDock>,
    fullscreen: Box<dyn FullscreenProbe>,
    monitors: Box<dyn MonitorSource>,
    window: Option<RawWindowHandle>,
    mode: ShellMode,
    /// Monitor the strip is on (fullscreen checks, dock updates).
    monitor: Option<MonitorInfo>,
    /// Monitor floating positions are relative to (the chosen one).
    anchor: Option<MonitorInfo>,
    hidden: bool,
    next_dock_update: Option<Instant>,
}

impl DockController {
    /// A controller over the given ports. Nothing happens until
    /// [`start`](Self::start).
    pub fn new(
        dock: Box<dyn EdgeDock>,
        fullscreen: Box<dyn FullscreenProbe>,
        monitors: Box<dyn MonitorSource>,
    ) -> Self {
        Self {
            dock,
            fullscreen,
            monitors,
            window: None,
            mode: ShellMode::NotStarted,
            monitor: None,
            anchor: None,
            hidden: false,
            next_dock_update: None,
        }
    }

    /// Current placement.
    pub fn mode(&self) -> &ShellMode {
        &self.mode
    }

    /// The monitor the strip is on.
    pub fn monitor(&self) -> Option<&MonitorInfo> {
        self.monitor.as_ref()
    }

    /// `true` while hidden for a fullscreen app.
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Connected monitors (for the Settings monitor list).
    pub fn monitors(&self) -> Vec<MonitorInfo> {
        self.monitors.monitors().unwrap_or_else(|error| {
            tracing::warn!(%error, "cannot list monitors");
            Vec::new()
        })
    }

    /// Places the strip for the first time.
    pub fn start(
        &mut self,
        window: RawWindowHandle,
        prefs: &DevicePrefs,
        now: Instant,
    ) -> Vec<ShellCommand> {
        self.window = Some(window);
        self.apply(prefs, now)
    }

    /// Re-places the strip after the placement or monitor preference
    /// changed: releases a reservation that is no longer wanted, then docks
    /// or floats as asked.
    pub fn apply(&mut self, prefs: &DevicePrefs, now: Instant) -> Vec<ShellCommand> {
        let Some(window) = self.window else {
            return Vec::new();
        };
        let monitors = self.monitors();
        let chosen = prefs
            .monitor
            .as_ref()
            .map(|m| MonitorId::new(m.id.as_str()));
        let Some(mut target) = resolve_monitor(&monitors, chosen.as_ref()).cloned() else {
            tracing::warn!("no monitors reported; leaving the window where it is");
            return vec![ShellCommand::Show];
        };
        // A scale override changes how many physical pixels 24 logical
        // pixels are; the UI applies the same factor as its zoom.
        if let Some(scale) = prefs.scale {
            target.scale_factor = scale;
        }
        self.anchor = Some(target.clone());
        let remembered = match prefs.placement {
            Placement::DockedBottom => {
                match self.dock.reserve(window, &target, STRIP_HEIGHT_LOGICAL) {
                    Ok(DockOutcome::Reserved { rect, guarantee }) => {
                        tracing::info!(%rect, ?guarantee, "strip docked");
                        self.mode = ShellMode::Docked { rect, guarantee };
                        self.monitor = Some(target);
                        self.next_dock_update = Some(now + DOCK_UPDATE_INTERVAL);
                        return self.visible();
                    }
                    Ok(DockOutcome::Unsupported { reason }) => {
                        tracing::info!(%reason, "edge reservation unsupported");
                        return self.float(
                            &monitors,
                            &target,
                            None,
                            FloatReason::Unsupported(reason),
                        );
                    }
                    Err(error) => {
                        tracing::warn!(%error, "docking failed");
                        return self.float(
                            &monitors,
                            &target,
                            None,
                            FloatReason::Failed(error.to_string()),
                        );
                    }
                }
            }
            Placement::Floating { position } => position,
        };
        self.release();
        self.float(&monitors, &target, remembered, FloatReason::Chosen)
    }

    /// Periodic work (call a few times per second, also while hidden):
    /// re-applies the dock reservation every [`DOCK_UPDATE_INTERVAL`] and
    /// hides/shows the strip around fullscreen apps.
    pub fn tick(&mut self, now: Instant) -> Vec<ShellCommand> {
        let mut commands = Vec::new();
        if matches!(self.mode, ShellMode::Docked { .. })
            && self.next_dock_update.is_some_and(|due| now >= due)
        {
            self.next_dock_update = Some(now + DOCK_UPDATE_INTERVAL);
            commands.extend(self.update_dock());
        }
        if let Some(monitor) = &self.monitor {
            let hide = self
                .fullscreen
                .fullscreen_state(monitor)
                .should_hide_strip();
            if hide != self.hidden {
                self.hidden = hide;
                commands.push(if hide {
                    ShellCommand::Hide
                } else {
                    ShellCommand::Show
                });
            }
        }
        commands
    }

    /// Forwards a native window message (Windows AppBar callback).
    pub fn handle_native_message(
        &mut self,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> Vec<ShellCommand> {
        match self.dock.handle_native_message(message, wparam, lparam) {
            Some(DockEvent::Repositioned { rect }) => {
                if let ShellMode::Docked { rect: current, .. } = &mut self.mode {
                    *current = rect;
                }
                Vec::new()
            }
            Some(DockEvent::FullscreenEntered) if !self.hidden => {
                self.hidden = true;
                vec![ShellCommand::Hide]
            }
            Some(DockEvent::FullscreenLeft) if self.hidden => {
                self.hidden = false;
                vec![ShellCommand::Show]
            }
            _ => Vec::new(),
        }
    }

    /// The floating window now has its top-left corner at `outer`
    /// (physical). Returns the position to persist, relative to the
    /// chosen monitor in logical pixels, when it changed.
    pub fn window_moved(&mut self, outer: PhysicalPosition) -> Option<Position> {
        let ShellMode::Floating { rect, .. } = &mut self.mode else {
            return None;
        };
        if rect.x == outer.x && rect.y == outer.y {
            return None;
        }
        rect.x = outer.x;
        rect.y = outer.y;
        let moved = *rect;
        let anchor = self.anchor.as_ref()?;
        // The strip may now be on another monitor: fullscreen checks follow it.
        let monitors = self.monitors();
        if let Some(on) = monitors
            .iter()
            .filter(|m| m.bounds.overlap_area(&moved) > 0)
            .max_by_key(|m| m.bounds.overlap_area(&moved))
        {
            self.monitor = Some(on.clone());
        }
        let scale = positive(anchor.scale_factor);
        let logical = |physical: i32, origin: i32| {
            let value = (f64::from(physical) - f64::from(origin)) / scale;
            // Saturating float-to-int conversion is the intent.
            value.round() as i32
        };
        Some(Position {
            x: logical(outer.x, anchor.bounds.x),
            y: logical(outer.y, anchor.bounds.y),
        })
    }

    /// Gives a reserved edge back (on exit).
    pub fn shutdown(&mut self) {
        self.release();
        self.mode = ShellMode::NotStarted;
    }

    fn release(&mut self) {
        if matches!(self.mode, ShellMode::Docked { .. }) {
            if let Err(error) = self.dock.release() {
                tracing::warn!(%error, "could not release the screen edge");
            }
        }
    }

    fn visible(&self) -> Vec<ShellCommand> {
        if self.hidden {
            Vec::new()
        } else {
            vec![ShellCommand::Show]
        }
    }

    fn float(
        &mut self,
        monitors: &[MonitorInfo],
        anchor: &MonitorInfo,
        remembered: Option<Position>,
        reason: FloatReason,
    ) -> Vec<ShellCommand> {
        let scale = positive(anchor.scale_factor);
        let remembered = remembered.map(|p| PhysicalPosition {
            x: offset(anchor.bounds.x, p.x, scale),
            y: offset(anchor.bounds.y, p.y, scale),
        });
        let rect = float_rect(monitors, Some(&anchor.id), FLOAT_SIZE, remembered)
            .unwrap_or_else(|| PhysicalRect::new(0, 0, FLOAT_SIZE.width, FLOAT_SIZE.height));
        self.monitor = monitors
            .iter()
            .filter(|m| m.work_area.overlap_area(&rect) > 0)
            .max_by_key(|m| m.work_area.overlap_area(&rect))
            .cloned()
            .or_else(|| Some(anchor.clone()));
        self.mode = ShellMode::Floating { rect, reason };
        self.next_dock_update = None;
        let mut commands = vec![ShellCommand::Place(rect)];
        commands.extend(self.visible());
        commands
    }

    fn update_dock(&mut self) -> Vec<ShellCommand> {
        let Some(current) = self.monitor.clone() else {
            return Vec::new();
        };
        let monitors = self.monitors();
        let monitor = resolve_monitor(&monitors, Some(&current.id))
            .cloned()
            .unwrap_or(current);
        match self.dock.update(&monitor, STRIP_HEIGHT_LOGICAL) {
            Ok(DockOutcome::Reserved { rect, guarantee }) => {
                self.mode = ShellMode::Docked { rect, guarantee };
                self.monitor = Some(monitor);
                Vec::new()
            }
            Ok(DockOutcome::Unsupported { reason }) => {
                let anchor = self.anchor.clone().unwrap_or(monitor);
                self.float(&monitors, &anchor, None, FloatReason::Unsupported(reason))
            }
            Err(DockError::NotReserved) => {
                // The reservation was lost (e.g. Explorer restarted): take it again.
                let Some(window) = self.window else {
                    return Vec::new();
                };
                match self.dock.reserve(window, &monitor, STRIP_HEIGHT_LOGICAL) {
                    Ok(DockOutcome::Reserved { rect, guarantee }) => {
                        self.mode = ShellMode::Docked { rect, guarantee };
                        Vec::new()
                    }
                    other => {
                        tracing::warn!(?other, "could not re-dock the strip");
                        Vec::new()
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "dock update failed");
                Vec::new()
            }
        }
    }
}

impl Drop for DockController {
    fn drop(&mut self) {
        self.release();
    }
}

fn positive(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn offset(origin: i32, logical: i32, scale: f64) -> i32 {
    // Saturating float-to-int conversion is the intent.
    (f64::from(origin) + f64::from(logical) * scale).round() as i32
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use std::num::NonZeroU32;
    use std::sync::{Arc, Mutex};

    use buildrail_platform::dock::{DockCall, ScriptedDock};
    use buildrail_platform::fullscreen::{FakeFullscreenProbe, FullscreenState};
    use buildrail_platform::monitors::{MonitorError, StaticMonitors};
    use buildrail_profile::MonitorChoice;
    use raw_window_handle::XcbWindowHandle;

    use super::*;

    fn window() -> RawWindowHandle {
        RawWindowHandle::Xcb(XcbWindowHandle::new(NonZeroU32::new(42).unwrap()))
    }

    fn monitor(id: &str, x: i32, scale: f64, primary: bool) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(id),
            bounds: PhysicalRect::new(x, 0, 1920, 1080),
            work_area: PhysicalRect::new(x, 0, 1920, 1040),
            scale_factor: scale,
            is_primary: primary,
        }
    }

    fn two_monitors() -> StaticMonitors {
        StaticMonitors(vec![
            monitor("DP-1", 0, 1.0, true),
            monitor("HDMI-1", 1920, 1.5, false),
        ])
    }

    struct F {
        dock: ScriptedDock,
        probe: FakeFullscreenProbe,
        shell: DockController,
        t0: Instant,
    }

    fn setup(dock: ScriptedDock, monitors: impl MonitorSource + 'static) -> F {
        let probe = FakeFullscreenProbe::new(FullscreenState::NotFullscreen);
        let shell = DockController::new(
            Box::new(dock.clone()),
            Box::new(probe.clone()),
            Box::new(monitors),
        );
        F {
            dock,
            probe,
            shell,
            t0: Instant::now(),
        }
    }

    #[test]
    fn docks_on_chosen_monitor_and_updates_periodically() {
        let mut f = setup(ScriptedDock::reserving(), two_monitors());
        assert_eq!(f.shell.mode(), &ShellMode::NotStarted);
        assert!(
            f.shell.apply(&DevicePrefs::default(), f.t0).is_empty(),
            "no window yet"
        );
        let prefs = DevicePrefs {
            monitor: Some(MonitorChoice {
                id: "HDMI-1".into(),
                name: None,
            }),
            ..DevicePrefs::default()
        };
        let commands = f.shell.start(window(), &prefs, f.t0);
        assert_eq!(commands, vec![ShellCommand::Show]);
        let ShellMode::Docked { rect, guarantee } = f.shell.mode().clone() else {
            panic!("{:?}", f.shell.mode())
        };
        assert_eq!(guarantee, ReservationGuarantee::Enforced);
        // 24 logical px at 150 % = 36 physical px, on the bottom of HDMI-1's work area.
        assert_eq!(rect, PhysicalRect::new(1920, 1004, 1920, 36));
        assert!(!f.shell.mode().is_movable());
        assert_eq!(f.shell.monitor().unwrap().id.as_str(), "HDMI-1");

        assert!(f.shell.tick(f.t0 + Duration::from_secs(1)).is_empty());
        assert_eq!(f.dock.calls().len(), 1, "no update before the interval");
        f.shell.tick(f.t0 + DOCK_UPDATE_INTERVAL);
        assert!(matches!(
            f.dock.calls().last(),
            Some(DockCall::Update {
                strip_height_logical: 24,
                ..
            })
        ));

        f.shell.shutdown();
        assert_eq!(f.dock.calls().last(), Some(&DockCall::Release));
        assert_eq!(f.shell.mode(), &ShellMode::NotStarted);
    }

    #[test]
    fn unsupported_desktop_floats_centred_above_bottom() {
        let mut f = setup(
            ScriptedDock::unsupported(UnsupportedReason::Wayland),
            two_monitors(),
        );
        let commands = f.shell.start(window(), &DevicePrefs::default(), f.t0);
        let [ShellCommand::Place(rect), ShellCommand::Show] = commands.as_slice() else {
            panic!("{commands:?}")
        };
        assert_eq!(rect.height, 24);
        assert_eq!(rect.width, 760);
        assert_eq!(rect.x, (1920 - 760) / 2);
        assert_eq!(rect.bottom(), 1040 - 8);
        let ShellMode::Floating { reason, .. } = f.shell.mode() else {
            panic!()
        };
        assert_eq!(
            reason,
            &FloatReason::Unsupported(UnsupportedReason::Wayland)
        );
        assert!(reason.explanation().contains("Wayland"));
        assert!(f.shell.mode().is_movable());
        // No periodic dock updates while floating.
        f.shell.tick(f.t0 + DOCK_UPDATE_INTERVAL * 2);
        assert!(f
            .dock
            .calls()
            .iter()
            .all(|c| matches!(c, DockCall::Reserve { .. })));
    }

    #[test]
    fn floating_preference_restores_and_persists_position() {
        let mut f = setup(ScriptedDock::reserving(), two_monitors());
        let prefs = DevicePrefs {
            placement: Placement::Floating {
                position: Some(Position { x: 100, y: 200 }),
            },
            monitor: Some(MonitorChoice {
                id: "HDMI-1".into(),
                name: None,
            }),
            ..DevicePrefs::default()
        };
        let commands = f.shell.start(window(), &prefs, f.t0);
        // 100 logical at 150 % from x = 1920 → 2070; y 300.
        assert_eq!(
            commands[0],
            ShellCommand::Place(PhysicalRect::new(2070, 300, 1140, 36))
        );
        assert!(f.dock.calls().is_empty(), "never reserved");
        let ShellMode::Floating { reason, .. } = f.shell.mode() else {
            panic!()
        };
        assert!(reason.explanation().contains("drag"));

        assert_eq!(
            f.shell.window_moved(PhysicalPosition { x: 2070, y: 300 }),
            None
        );
        let moved = f.shell.window_moved(PhysicalPosition { x: 2220, y: 450 });
        assert_eq!(moved, Some(Position { x: 200, y: 300 }));
        // Dragged onto the other monitor: fullscreen checks follow.
        f.shell.window_moved(PhysicalPosition { x: 10, y: 10 });
        assert_eq!(f.shell.monitor().unwrap().id.as_str(), "DP-1");
    }

    #[test]
    fn switching_placement_releases_and_redocks() {
        let mut f = setup(ScriptedDock::reserving(), two_monitors());
        f.shell.start(window(), &DevicePrefs::default(), f.t0);
        assert_eq!(f.shell.window_moved(PhysicalPosition { x: 1, y: 1 }), None);
        let floating = DevicePrefs {
            placement: Placement::Floating { position: None },
            ..DevicePrefs::default()
        };
        f.shell.apply(&floating, f.t0);
        assert!(f.dock.calls().contains(&DockCall::Release));
        assert!(f.shell.mode().is_movable());
        f.shell.apply(&DevicePrefs::default(), f.t0);
        assert!(matches!(f.shell.mode(), ShellMode::Docked { .. }));
    }

    #[test]
    fn hides_while_fullscreen_on_its_monitor() {
        let mut f = setup(ScriptedDock::reserving(), two_monitors());
        f.shell.start(window(), &DevicePrefs::default(), f.t0);
        f.probe.set(FullscreenState::Fullscreen);
        assert_eq!(f.shell.tick(f.t0), vec![ShellCommand::Hide]);
        assert!(f.shell.is_hidden());
        assert!(f.shell.tick(f.t0).is_empty(), "only on change");
        // Re-placing while hidden does not show it.
        assert!(f.shell.apply(&DevicePrefs::default(), f.t0).is_empty());
        f.probe.set(FullscreenState::Unknown);
        assert_eq!(f.shell.tick(f.t0), vec![ShellCommand::Show]);
    }

    /// A dock whose update result is scripted.
    struct FlakyDock {
        updates: Arc<Mutex<Vec<Result<DockOutcome, DockError>>>>,
        reserves: Arc<Mutex<u32>>,
        fail_reserve: bool,
    }

    impl EdgeDock for FlakyDock {
        fn reserve(
            &mut self,
            _: RawWindowHandle,
            monitor: &MonitorInfo,
            _: u32,
        ) -> Result<DockOutcome, DockError> {
            *self.reserves.lock().unwrap() += 1;
            if self.fail_reserve {
                return Err(DockError::X11("no display".into()));
            }
            Ok(DockOutcome::Reserved {
                rect: PhysicalRect::new(monitor.work_area.x, 1016, 1920, 24),
                guarantee: ReservationGuarantee::BestEffort,
            })
        }
        fn update(&mut self, _: &MonitorInfo, _: u32) -> Result<DockOutcome, DockError> {
            self.updates.lock().unwrap().remove(0)
        }
        fn release(&mut self) -> Result<(), DockError> {
            Err(DockError::NotReserved)
        }
        fn handle_native_message(&mut self, message: u32, _: usize, _: isize) -> Option<DockEvent> {
            match message {
                1 => Some(DockEvent::Repositioned {
                    rect: PhysicalRect::new(0, 1000, 1920, 24),
                }),
                2 => Some(DockEvent::FullscreenEntered),
                3 => Some(DockEvent::FullscreenLeft),
                _ => None,
            }
        }
    }

    fn flaky(
        updates: Vec<Result<DockOutcome, DockError>>,
        fail_reserve: bool,
    ) -> (F, Arc<Mutex<u32>>) {
        let reserves = Arc::new(Mutex::new(0));
        let dock = FlakyDock {
            updates: Arc::new(Mutex::new(updates)),
            reserves: reserves.clone(),
            fail_reserve,
        };
        let probe = FakeFullscreenProbe::new(FullscreenState::NotFullscreen);
        let shell = DockController::new(
            Box::new(dock),
            Box::new(probe.clone()),
            Box::new(two_monitors()),
        );
        (
            F {
                dock: ScriptedDock::reserving(),
                probe,
                shell,
                t0: Instant::now(),
            },
            reserves,
        )
    }

    #[test]
    fn dock_failures_and_recoveries() {
        let (mut f, reserves) = flaky(
            vec![
                Err(DockError::NotReserved),
                Err(DockError::X11("boom".into())),
                Ok(DockOutcome::Unsupported {
                    reason: UnsupportedReason::Wayland,
                }),
            ],
            false,
        );
        f.shell.start(window(), &DevicePrefs::default(), f.t0);
        assert!(matches!(
            f.shell.mode(),
            ShellMode::Docked {
                guarantee: ReservationGuarantee::BestEffort,
                ..
            }
        ));
        // Lost reservation → reserve again.
        f.shell.tick(f.t0 + DOCK_UPDATE_INTERVAL);
        assert_eq!(*reserves.lock().unwrap(), 2);
        // Transient error → stays docked.
        f.shell.tick(f.t0 + DOCK_UPDATE_INTERVAL * 2);
        assert!(matches!(f.shell.mode(), ShellMode::Docked { .. }));
        // Native messages.
        assert!(f.shell.handle_native_message(1, 0, 0).is_empty());
        assert!(matches!(f.shell.mode(), ShellMode::Docked { rect, .. } if rect.y == 1000));
        assert_eq!(
            f.shell.handle_native_message(2, 0, 0),
            vec![ShellCommand::Hide]
        );
        assert!(f.shell.handle_native_message(2, 0, 0).is_empty());
        assert_eq!(
            f.shell.handle_native_message(3, 0, 0),
            vec![ShellCommand::Show]
        );
        assert!(f.shell.handle_native_message(9, 0, 0).is_empty());
        // Desktop stops supporting docks → float.
        let commands = f.shell.tick(f.t0 + DOCK_UPDATE_INTERVAL * 3);
        assert!(matches!(commands.first(), Some(ShellCommand::Place(_))));
        assert!(f.shell.mode().is_movable());
        // Release errors are only logged.
        f.shell.shutdown();
    }

    #[test]
    fn reserve_error_falls_back_to_floating() {
        let (mut f, _) = flaky(vec![], true);
        f.shell.start(window(), &DevicePrefs::default(), f.t0);
        let ShellMode::Floating { reason, .. } = f.shell.mode() else {
            panic!()
        };
        assert!(matches!(reason, FloatReason::Failed(m) if m.contains("no display")));
        assert!(reason.explanation().starts_with("The strip could not dock"));
        f.probe.set(FullscreenState::Fullscreen);
        assert_eq!(f.shell.tick(f.t0), vec![ShellCommand::Hide]);
    }

    struct Broken;
    impl MonitorSource for Broken {
        fn monitors(&self) -> Result<Vec<MonitorInfo>, MonitorError> {
            Err(MonitorError("gone".into()))
        }
    }

    #[test]
    fn no_monitors_just_shows() {
        let mut f = setup(ScriptedDock::reserving(), Broken);
        assert_eq!(
            f.shell.start(window(), &DevicePrefs::default(), f.t0),
            vec![ShellCommand::Show]
        );
        assert!(f.shell.monitors().is_empty());
        assert!(f.shell.tick(f.t0).is_empty());
        assert_eq!(positive(f64::NAN), 1.0);
        assert_eq!(offset(10, 5, 2.0), 20);
        let _ = f.dock;
    }
}
