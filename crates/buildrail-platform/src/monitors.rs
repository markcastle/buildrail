//! Listing displays with their work areas.
//!
//! winit reports each monitor's position, size, scale factor and name, but
//! not its *work area* (the part not covered by the taskbar, Dock or
//! panels). The floating window's default position and the X11 dock both
//! need it, so this module reads it from the OS where possible:
//!
//! * **Windows** – `EnumDisplayMonitors` + `GetMonitorInfoW` (bounds, work
//!   area, primary flag, the `\\.\DISPLAYn` name winit also uses) and
//!   `GetDpiForMonitor` for the per-monitor scale.
//! * **X11** – RandR 1.5 monitors (names match winit's) with the work area
//!   from `_NET_WORKAREA` of the current desktop, clipped to each monitor.
//!   `_NET_WORKAREA` is one rectangle for the whole screen, so on
//!   multi-monitor setups it is only an approximation; X11 has no
//!   per-monitor scale, so the caller passes winit's scale factor.
//! * **macOS** – `NSScreen` (frame and `visibleFrame`, i.e. below the menu
//!   bar and above the Dock), through [`main_thread_monitors`]. AppKit
//!   only answers on the main thread, so the UI reads the list once when
//!   its window is created and wraps it in [`StaticMonitors`]; a display
//!   plugged in later is seen after a restart.
//! * **Wayland** – no source; build [`MonitorInfo`]s from winit with
//!   [`MonitorInfo::without_work_area`] and wrap them in [`StaticMonitors`].

use thiserror::Error;

use crate::geometry::{MonitorId, MonitorInfo, PhysicalRect};

/// Listing monitors failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("cannot list monitors: {0}")]
pub struct MonitorError(pub String);

/// Something that can list the connected monitors.
pub trait MonitorSource: Send + Sync {
    /// The connected monitors, in no particular order.
    fn monitors(&self) -> Result<Vec<MonitorInfo>, MonitorError>;
}

/// A fixed list: for tests, and for platforms where the UI builds the list
/// from winit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StaticMonitors(pub Vec<MonitorInfo>);

impl MonitorSource for StaticMonitors {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, MonitorError> {
        Ok(self.0.clone())
    }
}

/// A rectangle in AppKit's coordinates: points, origin at the bottom-left
/// corner of the primary screen, y growing upwards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppKitRect {
    /// Left edge.
    pub x: f64,
    /// Bottom edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// Converts an AppKit screen (its `frame`, its `visibleFrame`, the
/// primary screen's height, all in points, and its backing scale) into a
/// [`MonitorInfo`] in physical pixels with a top-left origin, which is
/// what the dock and float maths use. The work area is the visible frame:
/// below the menu bar, above the Dock.
///
/// Exact on one display and on displays that share a scale; with mixed
/// scales, positions on the secondary displays are approximate (AppKit
/// lays displays out in points, physical pixels differ per display).
pub fn appkit_monitor(
    index: usize,
    frame: AppKitRect,
    visible: AppKitRect,
    primary_height: f64,
    scale: f64,
) -> MonitorInfo {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let convert = |r: AppKitRect| {
        let top = primary_height - (r.y + r.height);
        // Saturating float-to-int conversion is the intent.
        let px = |v: f64| (v * scale).round() as i32;
        let len = |v: f64| (v.max(0.0) * scale).round() as u32;
        PhysicalRect::new(px(r.x), px(top), len(r.width), len(r.height))
    };
    MonitorInfo {
        id: MonitorId::new(format!("screen-{}", index + 1)),
        bounds: convert(frame),
        work_area: convert(visible),
        scale_factor: scale,
        is_primary: index == 0,
    }
}

/// The monitors as the main thread sees them, where only the main thread
/// can ask (macOS). `None` elsewhere, or when called off the main thread.
pub fn main_thread_monitors() -> Option<Vec<MonitorInfo>> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::screens()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// The OS monitor source, if this platform has one (see the module docs).
/// `x11_scale_factor` is used for every monitor on X11 (pass winit's
/// `scale_factor()`); it is ignored elsewhere.
pub fn platform_monitor_source(x11_scale_factor: f64) -> Option<Box<dyn MonitorSource>> {
    #[cfg(windows)]
    {
        let _ = x11_scale_factor;
        Some(Box::new(crate::win32::monitors::Win32Monitors))
    }
    #[cfg(x11_platform)]
    {
        if crate::dock::session::current_session().is_wayland() {
            return None;
        }
        match crate::x11::X11Connection::connect(None) {
            Ok(conn) => Some(Box::new(crate::x11::X11Monitors::new(
                conn,
                x11_scale_factor,
            ))),
            Err(err) => {
                tracing::warn!("cannot list X11 monitors: {err}");
                None
            }
        }
    }
    #[cfg(not(any(windows, x11_platform)))]
    {
        let _ = x11_scale_factor;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{MonitorId, PhysicalRect};

    #[test]
    fn static_source_returns_its_list() {
        let m = MonitorInfo::without_work_area(
            MonitorId::new("a"),
            PhysicalRect::new(0, 0, 1, 1),
            1.0,
            true,
        );
        let source = StaticMonitors(vec![m.clone()]);
        assert_eq!(source.monitors(), Ok(vec![m]));
        assert_eq!(
            MonitorError("x".into()).to_string(),
            "cannot list monitors: x"
        );
    }

    #[test]
    fn appkit_screens_become_top_left_physical_rects() {
        // A 1512 × 982 pt Retina screen with a 37 pt menu bar and a 70 pt
        // Dock at the bottom.
        let frame = AppKitRect {
            x: 0.0,
            y: 0.0,
            width: 1512.0,
            height: 982.0,
        };
        let visible = AppKitRect {
            x: 0.0,
            y: 70.0,
            width: 1512.0,
            height: 875.0,
        };
        let m = appkit_monitor(0, frame, visible, 982.0, 2.0);
        assert_eq!(m.bounds, PhysicalRect::new(0, 0, 3024, 1964));
        assert_eq!(m.work_area, PhysicalRect::new(0, 74, 3024, 1750));
        assert_eq!(m.work_area.bottom(), 1964 - 140, "above the Dock");
        assert!(m.is_primary);
        assert_eq!(m.id.as_str(), "screen-1");
        // A second display to the right and above: y up becomes y down.
        let side = AppKitRect {
            x: 1512.0,
            y: 500.0,
            width: 1920.0,
            height: 1080.0,
        };
        let m = appkit_monitor(1, side, side, 982.0, f64::NAN);
        assert_eq!(m.bounds, PhysicalRect::new(1512, 982 - 1580, 1920, 1080));
        assert_eq!(m.scale_factor, 1.0);
        assert!(!m.is_primary);
        if cfg!(not(target_os = "macos")) {
            assert_eq!(main_thread_monitors(), None);
        }
    }

    #[test]
    fn platform_source_does_not_panic() {
        if let Some(source) = platform_monitor_source(1.0) {
            let _ = source.monitors();
        }
    }
}
