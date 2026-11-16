//! Windows fullscreen probe. Decision rules: [`crate::fullscreen`] (pure).

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::Shell::{SHQueryUserNotificationState, QUERY_USER_NOTIFICATION_STATE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetShellWindow, GetWindowRect,
};

use super::{from_rect, hwnd_from};
use crate::fullscreen::{decide_windows_fullscreen, FullscreenProbe, FullscreenState};
use crate::geometry::{MonitorInfo, PhysicalRect};
use crate::wide::{from_wide, is_shell_class};

fn notification_state() -> Option<i32> {
    let mut state: QUERY_USER_NOTIFICATION_STATE = 0;
    // SAFETY: `state` is a valid, writable i32 for the duration of the call.
    let hr = unsafe { SHQueryUserNotificationState(&mut state) };
    (hr >= 0).then_some(state)
}

fn foreground_window() -> HWND {
    // SAFETY: no arguments; returns null when there is no foreground window.
    unsafe { GetForegroundWindow() }
}

fn shell_windows() -> [HWND; 2] {
    // SAFETY: no arguments; both return a handle or null.
    unsafe { [GetShellWindow(), GetDesktopWindow()] }
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0_u16; 64];
    let capacity = i32::try_from(buffer.len()).unwrap_or(0);
    // SAFETY: `buffer` is writable for `capacity` UTF-16 units; the call
    // writes at most that many including the NUL.
    let len = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), capacity) };
    let len = usize::try_from(len).unwrap_or(0).min(buffer.len());
    from_wide(&buffer[..len])
}

fn window_rect(hwnd: HWND) -> Option<PhysicalRect> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a valid, writable RECT; a stale handle makes the
    // call return 0.
    let ok = unsafe { GetWindowRect(hwnd, &mut rect) };
    (ok != 0).then(|| from_rect(&rect))
}

/// Fullscreen probe using the foreground window and
/// `SHQueryUserNotificationState`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Win32FullscreenProbe {
    own: Option<isize>,
}

impl Win32FullscreenProbe {
    /// A probe that ignores the strip's own window `own`.
    pub fn new(own: Option<isize>) -> Self {
        Self { own }
    }

    fn foreground_rect(&self) -> Option<PhysicalRect> {
        let hwnd = foreground_window();
        if hwnd.is_null()
            || shell_windows().contains(&hwnd)
            || self.own.map(hwnd_from) == Some(hwnd)
            || is_shell_class(&class_name(hwnd))
        {
            return None;
        }
        window_rect(hwnd)
    }
}

impl FullscreenProbe for Win32FullscreenProbe {
    fn fullscreen_state(&self, monitor: &MonitorInfo) -> FullscreenState {
        decide_windows_fullscreen(
            notification_state(),
            self.foreground_rect(),
            &monitor.bounds,
        )
    }
}
