//! Windows AppBar dock: the FFI behind [`crate::dock::appbar`]'s two ports.
//! Every decision (what to propose, how to read the shell's answer, when
//! to move the window) is made there and unit tested on every OS.

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::Shell::{
    SHAppBarMessage, ABE_BOTTOM, ABE_TOP, ABM_ACTIVATE, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE,
    ABM_SETPOS, ABM_WINDOWPOSCHANGED, APPBARDATA,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, RegisterWindowMessageW, SetWindowPos, HWND_BOTTOM, HWND_TOPMOST,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
};

use super::{hwnd_from, last_error};
use crate::dock::appbar::{AppBar, AppBarShell, AppBarWindow, Edges};
use crate::dock::{DockError, Edge};
use crate::geometry::PhysicalRect;
use crate::wide::to_wide;

const CALLBACK_NAME: &str = "BuildRail.AppBarNotify";

/// How long [`Win32Window::pause`] lets the window's thread catch up
/// with a posted move.
const PAUSE: std::time::Duration = std::time::Duration::from_millis(40);

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

fn edges_to_rect(edges: Edges) -> RECT {
    RECT {
        left: edges.left,
        top: edges.top,
        right: edges.right,
        bottom: edges.bottom,
    }
}

fn rect_to_edges(rect: &RECT) -> Edges {
    Edges {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

/// `SHAppBarMessage` for one callback message id.
#[derive(Debug, Clone, Copy)]
pub struct Win32Shell {
    callback: u32,
}

impl Win32Shell {
    /// A shell port notifying with BuildRail's registered message.
    pub fn new() -> Self {
        Self {
            callback: callback_message().unwrap_or(0),
        }
    }

    fn data(&self, window: isize, edge: Edge, rect: Edges) -> APPBARDATA {
        APPBARDATA {
            cbSize: u32::try_from(std::mem::size_of::<APPBARDATA>()).unwrap_or(0),
            hWnd: hwnd_from(window),
            uCallbackMessage: self.callback,
            uEdge: match edge {
                Edge::Top => ABE_TOP,
                Edge::Bottom => ABE_BOTTOM,
            },
            rc: edges_to_rect(rect),
            lParam: 0,
        }
    }
}

impl Default for Win32Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl AppBarShell for Win32Shell {
    fn register(&mut self, window: isize) -> Result<(), DockError> {
        let mut data = self.data(window, Edge::Bottom, Edges::default());
        if appbar_message(ABM_NEW, &mut data) == 0 {
            return Err(DockError::Win32 {
                operation: "SHAppBarMessage(ABM_NEW)",
                code: last_error(),
            });
        }
        Ok(())
    }

    fn query_pos(&mut self, window: isize, edge: Edge, proposed: Edges) -> Edges {
        let mut data = self.data(window, edge, proposed);
        appbar_message(ABM_QUERYPOS, &mut data);
        rect_to_edges(&data.rc)
    }

    fn set_pos(&mut self, window: isize, edge: Edge, rect: Edges) -> Edges {
        let mut data = self.data(window, edge, rect);
        appbar_message(ABM_SETPOS, &mut data);
        rect_to_edges(&data.rc)
    }

    fn remove(&mut self, window: isize) {
        let mut data = self.data(window, Edge::Bottom, Edges::default());
        appbar_message(ABM_REMOVE, &mut data);
    }

    fn activate(&mut self, window: isize) {
        let mut data = self.data(window, Edge::Bottom, Edges::default());
        appbar_message(ABM_ACTIVATE, &mut data);
    }

    fn window_pos_changed(&mut self, window: isize) {
        let mut data = self.data(window, Edge::Bottom, Edges::default());
        appbar_message(ABM_WINDOWPOSCHANGED, &mut data);
    }

    fn callback_message(&self) -> u32 {
        self.callback
    }
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

/// The window's outer rect, physical pixels (the process is per-monitor
/// DPI aware, so no coordinates are virtualised).
pub(crate) fn window_rect(window: isize) -> Option<PhysicalRect> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a valid, writable RECT; an invalid handle makes the
    // call fail, it is not undefined behaviour.
    let ok = unsafe { GetWindowRect(hwnd_from(window), &mut rect) };
    (ok != 0).then(|| super::from_rect(&rect))
}

/// Moves the strip's window with `SetWindowPos`.
///
/// Moves are posted to the window's own thread (`SWP_ASYNCWINDOWPOS`): the
/// dock runs on BuildRail's shell thread, and a synchronous cross-thread
/// `SetWindowPos` would wait for the UI thread, which itself waits for the
/// shell thread when it gives the screen edge back on exit. From the
/// window's own thread (the tests) the move happens at once.
#[derive(Debug, Clone, Copy, Default)]
pub struct Win32Window;

impl AppBarWindow for Win32Window {
    fn rect(&self, window: isize) -> Option<PhysicalRect> {
        window_rect(window)
    }

    fn place(&mut self, window: isize, rect: &PhysicalRect) -> Result<(), DockError> {
        set_window_pos(
            hwnd_from(window),
            HWND_TOPMOST,
            rect,
            SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        )
    }

    fn set_topmost(&mut self, window: isize, topmost: bool) -> Result<(), DockError> {
        let after = if topmost { HWND_TOPMOST } else { HWND_BOTTOM };
        set_window_pos(
            hwnd_from(window),
            after,
            &PhysicalRect::default(),
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        )
    }

    fn pause(&mut self) {
        std::thread::sleep(PAUSE);
    }
}

/// The Windows AppBar implementation of [`crate::dock::EdgeDock`].
pub type WindowsAppBar = AppBar<Win32Shell, Win32Window>;

/// An unregistered [`WindowsAppBar`].
pub fn windows_appbar() -> WindowsAppBar {
    AppBar::with_ports(Win32Shell::new(), Win32Window)
}
