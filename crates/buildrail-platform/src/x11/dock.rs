//! X11 strut-based dock. See the [`crate::dock`] module docs.

use std::sync::Arc;

use raw_window_handle::RawWindowHandle;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConfigureWindowAux, ConnectionExt as _, MapState, Window};

use super::{
    merge_atoms, x11_err, X11Connection, X11Error, ALL_DESKTOPS, NET_WM_STATE_ADD,
    SOURCE_APPLICATION,
};
use crate::dock::strut::{bottom_strut, StrutPartial};
use crate::dock::{
    x11_window_id, DockError, DockOutcome, EdgeDock, ReservationGuarantee, UnsupportedReason,
};
use crate::geometry::{dock_rect, MonitorInfo, PhysicalRect};

/// Docks an X11 window at the bottom of a monitor with EWMH struts.
#[derive(Debug)]
pub struct X11Dock {
    x: Arc<X11Connection>,
    window: Option<Window>,
    current: Option<PhysicalRect>,
}

impl X11Dock {
    /// A dock using `connection`.
    pub fn new(connection: Arc<X11Connection>) -> Self {
        Self {
            x: connection,
            window: None,
            current: None,
        }
    }

    /// The rect currently reserved, if any.
    pub fn current_rect(&self) -> Option<PhysicalRect> {
        self.current
    }

    fn apply(
        &mut self,
        window: Window,
        monitor: &MonitorInfo,
        height: u32,
        force: bool,
    ) -> Result<DockOutcome, DockError> {
        let own = self.current.filter(|_| self.window == Some(window));
        let rect = dock_rect(monitor, height, own.as_ref());
        let reserved = DockOutcome::Reserved {
            rect,
            guarantee: ReservationGuarantee::BestEffort,
        };
        if !force && own == Some(rect) {
            return Ok(reserved);
        }
        let (screen_width, screen_height) = self.x.root_size()?;
        let strut = bottom_strut(screen_width, screen_height, &rect)
            .ok_or(DockError::InvalidGeometry(rect))?;
        self.mark_as_dock(window)?;
        self.set_strut(window, &strut)?;
        self.place(window, &rect)?;
        self.window = Some(window);
        self.current = Some(rect);
        tracing::debug!("X11 dock reserved {rect}");
        Ok(reserved)
    }

    fn mark_as_dock(&self, window: Window) -> Result<(), X11Error> {
        let x = &self.x;
        let a = &x.atoms;
        x.set_u32s(
            window,
            a._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[a._NET_WM_WINDOW_TYPE_DOCK],
        )?;
        let attributes = x
            .conn
            .get_window_attributes(window)
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        let wanted = [a._NET_WM_STATE_ABOVE, a._NET_WM_STATE_STICKY];
        if attributes.map_state == MapState::UNMAPPED {
            // Before mapping, EWMH says to set the properties directly.
            let existing = x.get_u32s(window, a._NET_WM_STATE, AtomEnum::ATOM)?;
            x.set_u32s(
                window,
                a._NET_WM_STATE,
                AtomEnum::ATOM,
                &merge_atoms(&existing, &wanted),
            )?;
            x.set_u32s(
                window,
                a._NET_WM_DESKTOP,
                AtomEnum::CARDINAL,
                &[ALL_DESKTOPS],
            )
        } else {
            // Once mapped, the window manager owns these: ask it.
            x.client_message(
                window,
                a._NET_WM_STATE,
                [
                    NET_WM_STATE_ADD,
                    wanted[0],
                    wanted[1],
                    SOURCE_APPLICATION,
                    0,
                ],
            )?;
            x.client_message(
                window,
                a._NET_WM_DESKTOP,
                [ALL_DESKTOPS, SOURCE_APPLICATION, 0, 0, 0],
            )
        }
    }

    fn set_strut(&self, window: Window, strut: &StrutPartial) -> Result<(), X11Error> {
        let a = &self.x.atoms;
        self.x.set_u32s(
            window,
            a._NET_WM_STRUT_PARTIAL,
            AtomEnum::CARDINAL,
            &strut.to_cardinals(),
        )?;
        self.x.set_u32s(
            window,
            a._NET_WM_STRUT,
            AtomEnum::CARDINAL,
            &strut.to_legacy_cardinals(),
        )
    }

    fn place(&self, window: Window, rect: &PhysicalRect) -> Result<(), X11Error> {
        let aux = ConfigureWindowAux::new()
            .x(rect.x)
            .y(rect.y)
            .width(rect.width)
            .height(rect.height);
        self.x
            .conn
            .configure_window(window, &aux)
            .map_err(x11_err)?
            .check()
            .map_err(x11_err)?;
        self.x.conn.flush().map_err(x11_err)
    }

    fn clear(&mut self, window: Window) -> Result<(), X11Error> {
        let a = &self.x.atoms;
        self.current = None;
        self.window = None;
        self.x.delete(window, a._NET_WM_STRUT_PARTIAL)?;
        self.x.delete(window, a._NET_WM_STRUT)?;
        self.x.conn.flush().map_err(x11_err)
    }
}

impl EdgeDock for X11Dock {
    fn reserve(
        &mut self,
        window: RawWindowHandle,
        monitor: &MonitorInfo,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError> {
        let Some(id) = x11_window_id(&window) else {
            return Ok(DockOutcome::Unsupported {
                reason: UnsupportedReason::UnsupportedWindowHandle,
            });
        };
        if self.window.is_some_and(|w| w != id) {
            self.release()?;
        }
        self.apply(id, monitor, strip_height_logical, true)
    }

    fn update(
        &mut self,
        monitor: &MonitorInfo,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError> {
        let window = self.window.ok_or(DockError::NotReserved)?;
        self.apply(window, monitor, strip_height_logical, false)
    }

    fn release(&mut self) -> Result<(), DockError> {
        match self.window {
            Some(window) => Ok(self.clear(window)?),
            None => Ok(()),
        }
    }
}

impl Drop for X11Dock {
    fn drop(&mut self) {
        if let Err(err) = self.release() {
            tracing::debug!("releasing the X11 dock on drop failed: {err}");
        }
    }
}
