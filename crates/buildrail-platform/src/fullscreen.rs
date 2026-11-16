//! "Is a fullscreen app covering the strip's monitor?"
//!
//! The product respects fullscreen apps: while a game, video or slideshow
//! is fullscreen on the strip's monitor, the UI hides the strip. The UI
//! polls [`FullscreenProbe::fullscreen_state`] (once or twice a second is
//! plenty) and hides only on [`FullscreenState::Fullscreen`]; `Unknown`
//! means "keep showing".
//!
//! # Per-OS behaviour
//!
//! * **Windows** – the foreground window is fullscreen when it covers the
//!   whole monitor (borderless "fullscreen windowed" games and browsers) or
//!   when `SHQueryUserNotificationState` reports exclusive Direct3D
//!   fullscreen or presentation mode. The shell, desktop and our own
//!   window never count. The AppBar's `ABN_FULLSCREENAPP` notification
//!   (see [`crate::dock`]) reports the same thing faster when the UI
//!   forwards it.
//! * **Linux/BSD on X11** – the window named by `_NET_ACTIVE_WINDOW` is
//!   fullscreen when its `_NET_WM_STATE` contains `_NET_WM_STATE_FULLSCREEN`
//!   and it covers the monitor. Needs an EWMH window manager (every common
//!   one); without one the answer is `NotFullscreen`.
//! * **Wayland, macOS, others** – no portable way to ask; always
//!   `Unknown`. (On macOS a fullscreen app lives in its own Space, so a
//!   normal window is hidden anyway unless it opts into all Spaces.)

use std::sync::{Arc, Mutex, PoisonError};

use raw_window_handle::RawWindowHandle;

use crate::geometry::{MonitorInfo, PhysicalRect};

/// Whether a fullscreen app covers a monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FullscreenState {
    /// A fullscreen app covers the monitor: hide the strip.
    Fullscreen,
    /// Nothing fullscreen there.
    NotFullscreen,
    /// The platform cannot tell. Treat as "not fullscreen".
    Unknown,
}

impl FullscreenState {
    /// `true` only when the strip should be hidden.
    pub fn should_hide_strip(self) -> bool {
        self == Self::Fullscreen
    }
}

/// Asks the desktop whether a fullscreen app covers a monitor.
pub trait FullscreenProbe: Send + Sync {
    /// The state for `monitor`. Never fails: problems talking to the
    /// desktop give [`FullscreenState::Unknown`].
    fn fullscreen_state(&self, monitor: &MonitorInfo) -> FullscreenState;
}

/// `true` when `window` covers (at least) the whole of `monitor`.
pub fn window_covers_monitor(window: &PhysicalRect, monitor: &PhysicalRect) -> bool {
    !monitor.is_empty() && window.contains_rect(monitor)
}

/// `QUNS_RUNNING_D3D_FULL_SCREEN` from `shellapi.h`.
pub const QUNS_RUNNING_D3D_FULL_SCREEN: i32 = 3;
/// `QUNS_PRESENTATION_MODE` from `shellapi.h`.
pub const QUNS_PRESENTATION_MODE: i32 = 4;

/// The Windows decision, given the `SHQueryUserNotificationState` value
/// (`None` if the call failed) and the foreground window's rect (`None` if
/// there is none, or it is the shell, the desktop or the strip itself).
pub fn decide_windows_fullscreen(
    notification_state: Option<i32>,
    foreground: Option<PhysicalRect>,
    monitor: &PhysicalRect,
) -> FullscreenState {
    if let Some(QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE) = notification_state {
        return FullscreenState::Fullscreen;
    }
    match foreground {
        Some(rect) if window_covers_monitor(&rect, monitor) => FullscreenState::Fullscreen,
        Some(_) => FullscreenState::NotFullscreen,
        None if notification_state.is_some() => FullscreenState::NotFullscreen,
        None => FullscreenState::Unknown,
    }
}

/// A probe that always answers [`FullscreenState::Unknown`] (Wayland,
/// macOS, unsupported platforms).
#[derive(Debug, Clone, Copy, Default)]
pub struct UnknownFullscreenProbe;

impl FullscreenProbe for UnknownFullscreenProbe {
    fn fullscreen_state(&self, _: &MonitorInfo) -> FullscreenState {
        FullscreenState::Unknown
    }
}

/// A settable probe for UI tests. Clones share state.
#[derive(Debug, Clone)]
pub struct FakeFullscreenProbe {
    state: Arc<Mutex<FullscreenState>>,
}

impl FakeFullscreenProbe {
    /// A probe answering `state` until [`FakeFullscreenProbe::set`].
    pub fn new(state: FullscreenState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Changes the answer.
    pub fn set(&self, state: FullscreenState) {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = state;
    }
}

impl FullscreenProbe for FakeFullscreenProbe {
    fn fullscreen_state(&self, _: &MonitorInfo) -> FullscreenState {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The probe for the running OS. `own_window` is the strip's handle: it
/// picks X11 vs Wayland on Linux and is excluded from the check on Windows.
/// Falls back to [`UnknownFullscreenProbe`] when the desktop cannot be
/// reached.
pub fn platform_fullscreen_probe(own_window: Option<&RawWindowHandle>) -> Box<dyn FullscreenProbe> {
    #[cfg(windows)]
    {
        Box::new(crate::win32::fullscreen::Win32FullscreenProbe::new(
            own_window.and_then(crate::dock::win32_hwnd),
        ))
    }
    #[cfg(x11_platform)]
    {
        let use_x11 = match own_window {
            Some(handle) => crate::dock::x11_window_id(handle).is_some(),
            None => !crate::dock::session::current_session().is_wayland(),
        };
        if use_x11 {
            match crate::x11::X11Connection::connect(None) {
                Ok(conn) => return Box::new(crate::x11::X11FullscreenProbe::new(conn)),
                Err(err) => tracing::warn!("fullscreen detection unavailable: {err}"),
            }
        }
        Box::new(UnknownFullscreenProbe)
    }
    #[cfg(not(any(windows, x11_platform)))]
    {
        let _ = own_window;
        Box::new(UnknownFullscreenProbe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::MonitorId;

    const MONITOR: PhysicalRect = PhysicalRect::new(-1920, 0, 1920, 1080);

    fn info() -> MonitorInfo {
        MonitorInfo::without_work_area(MonitorId::new("m"), MONITOR, 1.0, false)
    }

    #[test]
    fn covering_means_containing_the_whole_monitor() {
        assert!(window_covers_monitor(&MONITOR, &MONITOR));
        assert!(window_covers_monitor(
            &PhysicalRect::new(-1928, -8, 1936, 1096),
            &MONITOR
        ));
        assert!(
            !window_covers_monitor(&PhysicalRect::new(-1920, 0, 1920, 1040), &MONITOR),
            "maximised"
        );
        assert!(
            !window_covers_monitor(&PhysicalRect::new(0, 0, 1920, 1080), &MONITOR),
            "other monitor"
        );
        assert!(!window_covers_monitor(&MONITOR, &PhysicalRect::default()));
    }

    #[test]
    fn windows_decision_table() {
        let full = Some(MONITOR);
        let small = Some(PhysicalRect::new(-1800, 100, 800, 600));
        assert_eq!(
            decide_windows_fullscreen(Some(3), small, &MONITOR),
            FullscreenState::Fullscreen
        );
        assert_eq!(
            decide_windows_fullscreen(Some(4), None, &MONITOR),
            FullscreenState::Fullscreen
        );
        assert_eq!(
            decide_windows_fullscreen(Some(2), full, &MONITOR),
            FullscreenState::Fullscreen
        );
        assert_eq!(
            decide_windows_fullscreen(None, full, &MONITOR),
            FullscreenState::Fullscreen
        );
        assert_eq!(
            decide_windows_fullscreen(Some(2), small, &MONITOR),
            FullscreenState::NotFullscreen
        );
        assert_eq!(
            decide_windows_fullscreen(Some(5), None, &MONITOR),
            FullscreenState::NotFullscreen
        );
        assert_eq!(
            decide_windows_fullscreen(None, None, &MONITOR),
            FullscreenState::Unknown
        );
    }

    #[test]
    fn fakes_and_unknown() {
        assert_eq!(
            UnknownFullscreenProbe.fullscreen_state(&info()),
            FullscreenState::Unknown
        );
        let fake = FakeFullscreenProbe::new(FullscreenState::NotFullscreen);
        let shared = fake.clone();
        let boxed: Box<dyn FullscreenProbe> = Box::new(fake);
        assert!(!boxed.fullscreen_state(&info()).should_hide_strip());
        shared.set(FullscreenState::Fullscreen);
        assert!(boxed.fullscreen_state(&info()).should_hide_strip());
        assert!(!FullscreenState::Unknown.should_hide_strip());
    }

    #[test]
    fn platform_probe_never_panics() {
        let probe = platform_fullscreen_probe(None);
        let _ = probe.fullscreen_state(&info());
    }
}
