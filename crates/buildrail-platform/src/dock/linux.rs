//! Linux/BSD dock: routes X11 windows to [`X11Dock`] and declines Wayland
//! windows. See the [`crate::dock`] module docs.

use raw_window_handle::RawWindowHandle;

use super::session::LinuxSession;
use super::{x11_window_id, DockError, DockOutcome, EdgeDock, UnsupportedReason};
use crate::geometry::MonitorInfo;
use crate::x11::{X11Connection, X11Dock};

/// The dock [`super::platform_dock`] returns on Linux and the BSDs.
#[derive(Debug)]
pub struct LinuxDock {
    session: LinuxSession,
    x11: Option<X11Dock>,
    declined: Option<UnsupportedReason>,
}

impl LinuxDock {
    /// A dock for a session detected with
    /// [`super::session::current_session`]. Connects to the X server only
    /// when an X11 window is first reserved.
    pub fn new(session: LinuxSession) -> Self {
        Self {
            session,
            x11: None,
            declined: None,
        }
    }

    fn decline(&mut self, reason: UnsupportedReason) -> Result<DockOutcome, DockError> {
        self.release()?;
        self.declined = Some(reason);
        Ok(DockOutcome::Unsupported { reason })
    }
}

impl EdgeDock for LinuxDock {
    fn reserve(
        &mut self,
        window: RawWindowHandle,
        monitor: &MonitorInfo,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError> {
        if let RawWindowHandle::Wayland(_) = window {
            return self.decline(UnsupportedReason::Wayland);
        }
        if x11_window_id(&window).is_none() {
            return self.decline(UnsupportedReason::UnsupportedWindowHandle);
        }
        if self.session.is_wayland() {
            tracing::info!(
                "docking an XWayland window: the compositor may ignore the reserved edge"
            );
        }
        self.declined = None;
        let dock = match &mut self.x11 {
            Some(dock) => dock,
            None => self.x11.insert(X11Dock::new(X11Connection::connect(None)?)),
        };
        dock.reserve(window, monitor, strip_height_logical)
    }

    fn update(
        &mut self,
        monitor: &MonitorInfo,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError> {
        if let Some(reason) = self.declined {
            return Ok(DockOutcome::Unsupported { reason });
        }
        match &mut self.x11 {
            Some(dock) => dock.update(monitor, strip_height_logical),
            None => Err(DockError::NotReserved),
        }
    }

    fn release(&mut self) -> Result<(), DockError> {
        match &mut self.x11 {
            Some(dock) => dock.release(),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{MonitorId, PhysicalRect};
    use raw_window_handle::{AppKitWindowHandle, WaylandWindowHandle};
    use std::ptr::NonNull;

    fn monitor() -> MonitorInfo {
        MonitorInfo::without_work_area(
            MonitorId::new("m"),
            PhysicalRect::new(0, 0, 800, 600),
            1.0,
            true,
        )
    }

    #[test]
    fn wayland_windows_are_declined_without_touching_x11() {
        let mut dummy = 0_u8;
        let handle =
            RawWindowHandle::Wayland(WaylandWindowHandle::new(NonNull::from(&mut dummy).cast()));
        let mut dock = LinuxDock::new(LinuxSession::Wayland { xwayland: false });
        assert_eq!(dock.update(&monitor(), 24), Err(DockError::NotReserved));
        let outcome = dock.reserve(handle, &monitor(), 24).unwrap();
        assert_eq!(
            outcome,
            DockOutcome::Unsupported {
                reason: UnsupportedReason::Wayland
            }
        );
        assert_eq!(dock.update(&monitor(), 24), Ok(outcome));
        assert!(dock.x11.is_none());
        assert_eq!(dock.release(), Ok(()));
    }

    #[test]
    fn foreign_handles_are_declined() {
        let mut dummy = 0_u8;
        let handle =
            RawWindowHandle::AppKit(AppKitWindowHandle::new(NonNull::from(&mut dummy).cast()));
        let mut dock = LinuxDock::new(LinuxSession::X11);
        assert_eq!(
            dock.reserve(handle, &monitor(), 24),
            Ok(DockOutcome::Unsupported {
                reason: UnsupportedReason::UnsupportedWindowHandle
            })
        );
    }
}
