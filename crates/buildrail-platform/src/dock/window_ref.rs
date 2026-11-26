//! [`WindowRef`]: a native window identity that can cross threads.
//!
//! `raw-window-handle`'s [`RawWindowHandle`] is not `Send` (some variants
//! carry pointers), but the windows BuildRail docks are named by plain
//! integers: an `HWND` on Windows, an X11 window id on Linux. The UI keeps
//! its dock work off the paint thread, so it hands the dock a `WindowRef`
//! and the dock turns it back into a handle where one can be rebuilt.

use std::num::{NonZeroIsize, NonZeroU32};

use raw_window_handle::{RawWindowHandle, Win32WindowHandle, XcbWindowHandle, XlibWindowHandle};

use super::{win32_hwnd, x11_window_id, UnsupportedReason};

/// A window identity that is `Send + Copy`. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowRef {
    /// A Win32 `HWND`.
    Win32(NonZeroIsize),
    /// An X11 window id from an Xlib handle.
    Xlib(NonZeroU32),
    /// An X11 window id from an XCB handle.
    Xcb(NonZeroU32),
    /// A native Wayland surface (cannot be docked).
    Wayland,
    /// A macOS `NSView` (cannot be docked).
    AppKit,
    /// Anything else.
    Other,
}

impl WindowRef {
    /// The identity behind `handle`.
    pub fn from_raw(handle: &RawWindowHandle) -> Self {
        match handle {
            RawWindowHandle::Win32(_) => win32_hwnd(handle)
                .and_then(NonZeroIsize::new)
                .map_or(Self::Other, Self::Win32),
            RawWindowHandle::Xlib(_) => x11_window_id(handle)
                .and_then(NonZeroU32::new)
                .map_or(Self::Other, Self::Xlib),
            RawWindowHandle::Xcb(_) => x11_window_id(handle)
                .and_then(NonZeroU32::new)
                .map_or(Self::Other, Self::Xcb),
            RawWindowHandle::Wayland(_) => Self::Wayland,
            RawWindowHandle::AppKit(_) => Self::AppKit,
            _ => Self::Other,
        }
    }

    /// The handle again, for the kinds that can be docked; `None` for the
    /// others (their handles hold pointers that are not kept).
    pub fn to_raw(self) -> Option<RawWindowHandle> {
        match self {
            Self::Win32(hwnd) => Some(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd))),
            Self::Xlib(id) => Some(RawWindowHandle::Xlib(XlibWindowHandle::new(
                std::os::raw::c_ulong::from(id.get()),
            ))),
            Self::Xcb(id) => Some(RawWindowHandle::Xcb(XcbWindowHandle::new(id))),
            Self::Wayland | Self::AppKit | Self::Other => None,
        }
    }

    /// Why a window of this kind cannot be docked, for the kinds
    /// [`to_raw`](Self::to_raw) cannot rebuild.
    pub fn unsupported_reason(self) -> Option<UnsupportedReason> {
        match self {
            Self::Win32(_) | Self::Xlib(_) | Self::Xcb(_) => None,
            Self::Wayland => Some(UnsupportedReason::Wayland),
            Self::AppKit => Some(UnsupportedReason::MacOs),
            Self::Other => Some(UnsupportedReason::UnsupportedWindowHandle),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ptr::NonNull;

    use raw_window_handle::{AppKitWindowHandle, WaylandWindowHandle, WebWindowHandle};

    use super::*;

    #[test]
    fn dockable_handles_round_trip() {
        let handles = [
            RawWindowHandle::Win32(Win32WindowHandle::new(NonZeroIsize::new(77).unwrap())),
            RawWindowHandle::Xlib(XlibWindowHandle::new(42)),
            RawWindowHandle::Xcb(XcbWindowHandle::new(NonZeroU32::new(43).unwrap())),
        ];
        for handle in handles {
            let window = WindowRef::from_raw(&handle);
            assert_eq!(window.to_raw(), Some(handle));
            assert_eq!(window.unsupported_reason(), None);
        }
    }

    #[test]
    fn other_handles_say_why_they_cannot_dock() {
        let mut probe = 1u8;
        let pointer = NonNull::from(&mut probe).cast();
        let cases = [
            (
                RawWindowHandle::Wayland(WaylandWindowHandle::new(pointer)),
                UnsupportedReason::Wayland,
            ),
            (
                RawWindowHandle::AppKit(AppKitWindowHandle::new(pointer)),
                UnsupportedReason::MacOs,
            ),
            (
                RawWindowHandle::Web(WebWindowHandle::new(1)),
                UnsupportedReason::UnsupportedWindowHandle,
            ),
            (
                RawWindowHandle::Xlib(XlibWindowHandle::new(0)),
                UnsupportedReason::UnsupportedWindowHandle,
            ),
        ];
        for (handle, reason) in cases {
            let window = WindowRef::from_raw(&handle);
            assert_eq!(window.to_raw(), None);
            assert_eq!(window.unsupported_reason(), Some(reason));
        }
    }
}
