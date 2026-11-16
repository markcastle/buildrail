//! Windows AppBar dock. Protocol and maths: [`crate::dock::appbar`] (pure).

use raw_window_handle::RawWindowHandle;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    SHAppBarMessage, ABE_BOTTOM, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, APPBARDATA,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    RegisterWindowMessageW, SetWindowPos, HWND_BOTTOM, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE,
};

use super::{from_rect, hwnd_from, last_error, to_rect};
use crate::dock::{
    after_query, decode_notification, query_proposal, win32_hwnd, AppBarNotification, DockError,
    DockEvent, DockOutcome, EdgeDock, ReservationGuarantee, UnsupportedReason,
};
use crate::geometry::{MonitorInfo, PhysicalRect};
use crate::wide::to_wide;

const CALLBACK_NAME: &str = "BuildRail.AppBarNotify";

/// The registered callback message id (same value in every thread and
/// process for the same name).
pub(crate) fn callback_message() -> Option<u32> {
    let name = to_wide(CALLBACK_NAME);
    // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call.
    let id = unsafe { RegisterWindowMessageW(name.as_ptr()) };
    (id != 0).then_some(id)
}

fn appbar_message(message: u32, data: &mut APPBARDATA) -> usize {
    // SAFETY: `data` is an initialised APPBARDATA with `cbSize` set, borrowed
    // exclusively for the duration of the call.
    unsafe { SHAppBarMessage(message, data) }
}

fn set_window_pos(
    hwnd: HWND,
    after: HWND,
    rect: &PhysicalRect,
    flags: u32,
) -> Result<(), DockError> {
    let width = i32::try_from(rect.width).unwrap_or(i32::MAX);
    let height = i32::try_from(rect.height).unwrap_or(i32::MAX);
    // SAFETY: plain value arguments; an invalid window handle makes the call
    // fail with an error code, it is not undefined behaviour.
    let ok = unsafe { SetWindowPos(hwnd, after, rect.x, rect.y, width, height, flags) };
    if ok == 0 {
        return Err(DockError::Win32 {
            operation: "SetWindowPos",
            code: last_error(),
        });
    }
    Ok(())
}

/// The Windows AppBar implementation of [`EdgeDock`].
#[derive(Debug)]
pub struct WindowsAppBar {
    hwnd: Option<isize>,
    callback: u32,
    current: Option<PhysicalRect>,
    placement: Option<(MonitorInfo, u32)>,
}

impl WindowsAppBar {
    /// An unregistered AppBar.
    pub fn new() -> Self {
        Self {
            hwnd: None,
            callback: callback_message().unwrap_or(0),
            current: None,
            placement: None,
        }
    }

    fn data(&self, hwnd: isize, rect: Option<&PhysicalRect>) -> APPBARDATA {
        APPBARDATA {
            cbSize: u32::try_from(std::mem::size_of::<APPBARDATA>()).unwrap_or(0),
            hWnd: hwnd_from(hwnd),
            uCallbackMessage: self.callback,
            uEdge: ABE_BOTTOM,
            rc: rect.map(to_rect).unwrap_or_default(),
            lParam: 0,
        }
    }

    fn position(&mut self, hwnd: isize, force: bool) -> Result<DockOutcome, DockError> {
        let (monitor, height) = self.placement.clone().ok_or(DockError::NotReserved)?;
        let mut data = self.data(hwnd, Some(&query_proposal(&monitor, height)));
        appbar_message(ABM_QUERYPOS, &mut data);
        let wanted = after_query(&from_rect(&data.rc), &monitor, height);
        let reserved = |rect| DockOutcome::Reserved {
            rect,
            guarantee: ReservationGuarantee::Enforced,
        };
        if !force && self.current == Some(wanted) {
            return Ok(reserved(wanted));
        }
        data.rc = to_rect(&wanted);
        appbar_message(ABM_SETPOS, &mut data);
        let rect = from_rect(&data.rc);
        set_window_pos(hwnd_from(hwnd), HWND_TOPMOST, &rect, SWP_NOACTIVATE)?;
        self.current = Some(rect);
        Ok(reserved(rect))
    }
}

impl Default for WindowsAppBar {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeDock for WindowsAppBar {
    fn reserve(
        &mut self,
        window: RawWindowHandle,
        monitor: &MonitorInfo,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError> {
        let Some(hwnd) = win32_hwnd(&window) else {
            return Ok(DockOutcome::Unsupported {
                reason: UnsupportedReason::UnsupportedWindowHandle,
            });
        };
        if self.hwnd.is_some_and(|h| h != hwnd) {
            self.release()?;
        }
        if self.hwnd.is_none() {
            let mut data = self.data(hwnd, None);
            if appbar_message(ABM_NEW, &mut data) == 0 {
                return Err(DockError::Win32 {
                    operation: "SHAppBarMessage(ABM_NEW)",
                    code: last_error(),
                });
            }
            self.hwnd = Some(hwnd);
        }
        self.placement = Some((monitor.clone(), strip_height_logical));
        self.position(hwnd, true)
    }

    fn update(
        &mut self,
        monitor: &MonitorInfo,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError> {
        let hwnd = self.hwnd.ok_or(DockError::NotReserved)?;
        self.placement = Some((monitor.clone(), strip_height_logical));
        self.position(hwnd, false)
    }

    fn release(&mut self) -> Result<(), DockError> {
        if let Some(hwnd) = self.hwnd.take() {
            let mut data = self.data(hwnd, None);
            appbar_message(ABM_REMOVE, &mut data);
        }
        self.current = None;
        self.placement = None;
        Ok(())
    }

    fn handle_native_message(
        &mut self,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> Option<DockEvent> {
        let hwnd = self.hwnd?;
        if message != self.callback || self.callback == 0 {
            return None;
        }
        let z_only = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        match decode_notification(wparam, lparam)? {
            AppBarNotification::PositionChanged | AppBarNotification::StateChanged => {
                match self.position(hwnd, false) {
                    Ok(DockOutcome::Reserved { rect, .. }) => {
                        Some(DockEvent::Repositioned { rect })
                    }
                    Ok(DockOutcome::Unsupported { .. }) => None,
                    Err(err) => {
                        tracing::warn!("AppBar reposition failed: {err}");
                        None
                    }
                }
            }
            AppBarNotification::FullscreenApp(entered) => {
                // As the AppBar docs prescribe: drop below the fullscreen
                // app, and come back on top when it closes.
                let after = if entered { HWND_BOTTOM } else { HWND_TOPMOST };
                if let Err(err) =
                    set_window_pos(hwnd_from(hwnd), after, &PhysicalRect::default(), z_only)
                {
                    tracing::debug!("AppBar z-order change failed: {err}");
                }
                Some(if entered {
                    DockEvent::FullscreenEntered
                } else {
                    DockEvent::FullscreenLeft
                })
            }
            AppBarNotification::WindowArrange(_) => None,
        }
    }
}

impl Drop for WindowsAppBar {
    fn drop(&mut self) {
        let _ = self.release();
    }
}
