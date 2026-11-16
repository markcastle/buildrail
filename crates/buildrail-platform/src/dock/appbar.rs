//! Platform-independent half of the Windows AppBar integration.
//!
//! Everything that is plain arithmetic or decoding lives here so it is unit
//! tested on every OS; `crate::win32::appbar` only forwards these values to
//! `SHAppBarMessage`.
//!
//! The AppBar protocol, in short:
//!
//! 1. `ABM_NEW` registers our window and the private message the shell uses
//!    to notify us.
//! 2. `ABM_QUERYPOS` with a proposed rect on the bottom edge of the
//!    monitor: the shell moves the rect up so it does not overlap the taskbar
//!    or other appbars ([`query_proposal`]).
//! 3. We restore our height from the adjusted bottom edge
//!    ([`after_query`]) and send `ABM_SETPOS`; the shell reserves that area,
//!    so maximised windows stop above it.
//! 4. We move our window there.
//! 5. The shell sends our callback message with `wParam` set to one of the
//!    `ABN_*` codes ([`decode_notification`]) when the taskbar moves, a
//!    fullscreen app opens or closes, and so on.
//! 6. `ABM_REMOVE` gives the space back.

use crate::geometry::{MonitorInfo, PhysicalRect};

/// `ABN_STATECHANGE`: the taskbar's auto-hide or always-on-top state changed.
pub const ABN_STATECHANGE: usize = 0;
/// `ABN_POSCHANGED`: the taskbar or another appbar moved; re-query our
/// position.
pub const ABN_POSCHANGED: usize = 1;
/// `ABN_FULLSCREENAPP`: a fullscreen app opened (`lParam != 0`) or the last
/// one closed (`lParam == 0`).
pub const ABN_FULLSCREENAPP: usize = 2;
/// `ABN_WINDOWARRANGE`: the user is tiling/cascading windows
/// (`lParam != 0` before, `0` after).
pub const ABN_WINDOWARRANGE: usize = 3;

/// A decoded AppBar callback notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppBarNotification {
    /// Taskbar auto-hide / always-on-top state changed.
    StateChanged,
    /// The taskbar or another appbar moved.
    PositionChanged,
    /// A fullscreen app opened (`true`) or closed (`false`).
    FullscreenApp(bool),
    /// Window arrangement started (`true`) or finished (`false`).
    WindowArrange(bool),
}

/// Decodes the `wParam`/`lParam` of our callback message. Unknown codes
/// return `None`.
pub fn decode_notification(wparam: usize, lparam: isize) -> Option<AppBarNotification> {
    match wparam {
        ABN_STATECHANGE => Some(AppBarNotification::StateChanged),
        ABN_POSCHANGED => Some(AppBarNotification::PositionChanged),
        ABN_FULLSCREENAPP => Some(AppBarNotification::FullscreenApp(lparam != 0)),
        ABN_WINDOWARRANGE => Some(AppBarNotification::WindowArrange(lparam != 0)),
        _ => None,
    }
}

/// The rect to propose with `ABM_QUERYPOS`: the full width of the monitor,
/// at the very bottom of its *bounds*. The shell then pushes it above the
/// taskbar and any other bottom appbars.
///
/// Monitor bounds are used deliberately, not the work area: the work area
/// already excludes our own reservation, so proposing from it would move
/// the strip up by its own height every time we re-query.
pub fn query_proposal(monitor: &MonitorInfo, strip_height_logical: u32) -> PhysicalRect {
    let bounds = monitor.bounds;
    let height = monitor.to_physical(strip_height_logical).min(bounds.height);
    PhysicalRect::from_edges(
        bounds.x,
        bounds.bottom() - i32::try_from(height).unwrap_or(0),
        bounds.right(),
        bounds.bottom(),
    )
}

/// After `ABM_QUERYPOS` the shell may have moved the bottom edge up (above
/// the taskbar) without keeping our height. Re-derive the top edge from the
/// adjusted bottom, as the AppBar documentation prescribes for a bottom bar.
pub fn after_query(
    queried: &PhysicalRect,
    monitor: &MonitorInfo,
    strip_height_logical: u32,
) -> PhysicalRect {
    let height = monitor.to_physical(strip_height_logical);
    PhysicalRect::from_edges(
        queried.x,
        queried.bottom() - i32::try_from(height).unwrap_or(0),
        queried.right(),
        queried.bottom(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::MonitorId;

    fn monitor(bounds: PhysicalRect, scale: f64) -> MonitorInfo {
        MonitorInfo::without_work_area(MonitorId::new("m"), bounds, scale, true)
    }

    #[test]
    fn decodes_every_notification() {
        assert_eq!(
            decode_notification(0, 0),
            Some(AppBarNotification::StateChanged)
        );
        assert_eq!(
            decode_notification(1, 0),
            Some(AppBarNotification::PositionChanged)
        );
        assert_eq!(
            decode_notification(2, 1),
            Some(AppBarNotification::FullscreenApp(true))
        );
        assert_eq!(
            decode_notification(2, 0),
            Some(AppBarNotification::FullscreenApp(false))
        );
        assert_eq!(
            decode_notification(3, 1),
            Some(AppBarNotification::WindowArrange(true))
        );
        assert_eq!(
            decode_notification(3, 0),
            Some(AppBarNotification::WindowArrange(false))
        );
        assert_eq!(decode_notification(99, 0), None);
    }

    #[test]
    fn proposal_hugs_the_monitor_bottom() {
        let m = monitor(PhysicalRect::new(0, 0, 1920, 1080), 1.0);
        assert_eq!(query_proposal(&m, 24), PhysicalRect::new(0, 1056, 1920, 24));
        let m = monitor(PhysicalRect::new(-2560, -360, 2560, 1440), 1.5);
        assert_eq!(
            query_proposal(&m, 24),
            PhysicalRect::new(-2560, 1044, 2560, 36)
        );
    }

    #[test]
    fn proposal_ignores_the_work_area() {
        let mut m = monitor(PhysicalRect::new(0, 0, 1920, 1080), 1.0);
        m.work_area = PhysicalRect::new(0, 0, 1920, 1008);
        assert_eq!(query_proposal(&m, 24).bottom(), 1080);
    }

    #[test]
    fn after_query_restores_height_above_the_taskbar() {
        let m = monitor(PhysicalRect::new(0, 0, 1920, 1080), 1.25);
        // The shell moved the bottom up to the taskbar top (1032) and only
        // part of the way for the top, squashing the rect.
        let queried = PhysicalRect::from_edges(0, 1026, 1920, 1032);
        assert_eq!(
            after_query(&queried, &m, 24),
            PhysicalRect::new(0, 1002, 1920, 30)
        );
    }

    #[test]
    fn tiny_monitor_does_not_overflow() {
        let m = monitor(PhysicalRect::new(0, 0, 100, 10), 1.0);
        assert_eq!(query_proposal(&m, 24), PhysicalRect::new(0, 0, 100, 10));
    }
}
