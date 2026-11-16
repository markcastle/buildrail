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
//! * **Wayland, macOS** – no source; build [`MonitorInfo`]s from winit
//!   with [`MonitorInfo::without_work_area`] and wrap them in
//!   [`StaticMonitors`].

use thiserror::Error;

use crate::geometry::MonitorInfo;

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
    fn platform_source_does_not_panic() {
        if let Some(source) = platform_monitor_source(1.0) {
            let _ = source.monitors();
        }
    }
}
