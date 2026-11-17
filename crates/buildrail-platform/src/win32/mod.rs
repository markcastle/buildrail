//! Win32 shims. Every `unsafe` block in the crate lives in this module, each
//! in a function a few lines long with a `// SAFETY:` comment. All decisions
//! (rect maths, notification decoding, fullscreen rules, quoting) are made
//! by platform-independent code that is unit tested on every OS.
#![allow(unsafe_code)]
#![warn(clippy::undocumented_unsafe_blocks)]

pub(crate) mod appbar;
pub(crate) mod console;
pub(crate) mod fullscreen;
pub(crate) mod instance;
pub(crate) mod monitors;
pub(crate) mod motion;
pub(crate) mod registry;

use windows_sys::Win32::Foundation::{HWND, RECT};

use crate::geometry::PhysicalRect;

/// `GetLastError()` of the calling thread, read through std (no `unsafe`).
fn last_error() -> i64 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .map_or(0, i64::from)
}

fn hwnd_from(raw: isize) -> HWND {
    raw as HWND
}

fn to_rect(rect: &PhysicalRect) -> RECT {
    RECT {
        left: rect.x,
        top: rect.y,
        right: rect.right(),
        bottom: rect.bottom(),
    }
}

/// Converts a Win32 rect. An inverted rect (the shell can return one from
/// `ABM_QUERYPOS` when it squashes a bar) keeps its bottom-right corner,
/// which is what the AppBar maths relies on.
fn from_rect(rect: &RECT) -> PhysicalRect {
    PhysicalRect::from_edges(
        rect.left.min(rect.right),
        rect.top.min(rect.bottom),
        rect.right,
        rect.bottom,
    )
}
