//! BuildRail platform adapters: everything that touches the operating
//! system's windowing, credential and session services, behind small traits
//! with in-memory fakes so the UI and core can be tested without a desktop.
//!
//! | Module | Trait | Production | Fake |
//! |---|---|---|---|
//! | [`credentials`] | core's `CredentialReader` / `CredentialWriter` | [`credentials::KeyringCredentialStore`] | [`credentials::InMemoryCredentialStore`] |
//! | [`geometry`] | – (pure functions) | dock / floating rects | – |
//! | [`dock`] | [`dock::EdgeDock`] | [`dock::platform_dock`] | [`dock::ScriptedDock`] |
//! | [`fullscreen`] | [`fullscreen::FullscreenProbe`] | [`fullscreen::platform_fullscreen_probe`] | [`fullscreen::FakeFullscreenProbe`] |
//! | [`monitors`] | [`monitors::MonitorSource`] | [`monitors::platform_monitor_source`] | [`monitors::StaticMonitors`] |
//! | [`url_opener`] | [`url_opener::UrlOpener`] | [`url_opener::SystemUrlOpener`] | [`url_opener::RecordingUrlOpener`] |
//! | [`autostart`] | [`autostart::Autostart`] | [`autostart::platform_autostart`] | [`autostart::InMemoryAutostart`] |
//!
//! Native window handles are accepted as `raw-window-handle` 0.6 types, so
//! nothing here depends on egui, eframe or winit.
//!
//! Design rule: OS-specific code (`win32`, `x11`) is a thin shim. Every
//! decision that can be made without the OS (rectangles, strut values,
//! AppBar rect adjustment, notification decoding, fullscreen rules,
//! command-line quoting, `.desktop`/plist text) lives in
//! platform-independent functions that are unit tested on every OS.
//!
//! `unsafe` is denied crate-wide and allowed only inside the `win32`
//! module, one small function per FFI call, each with a `// SAFETY:` note.
#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod autostart;
pub mod credentials;
pub mod dock;
pub mod fullscreen;
pub mod geometry;
pub mod monitors;
pub mod url_opener;
mod wide;

#[cfg(windows)]
mod win32;
#[cfg(x11_platform)]
pub mod x11;

/// Windows implementations, for callers that want them directly rather
/// than through the `platform_*` factories.
#[cfg(windows)]
pub mod windows {
    pub use crate::win32::appbar::WindowsAppBar;
    pub use crate::win32::fullscreen::Win32FullscreenProbe;
    pub use crate::win32::monitors::Win32Monitors;
    pub use crate::win32::registry::CurrentUserRunKey;
}
