//! X11 fullscreen detection via EWMH. See [`crate::fullscreen`].

use std::sync::Arc;

use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

use super::{x11_err, X11Connection, X11Error};
use crate::fullscreen::{window_covers_monitor, FullscreenProbe, FullscreenState};
use crate::geometry::{MonitorInfo, PhysicalRect};

/// Reports a fullscreen active window on a monitor.
#[derive(Debug, Clone)]
pub struct X11FullscreenProbe {
    x: Arc<X11Connection>,
}

impl X11FullscreenProbe {
    /// A probe using `connection`.
    pub fn new(connection: Arc<X11Connection>) -> Self {
        Self { x: connection }
    }

    fn query(&self, monitor: &PhysicalRect) -> Result<FullscreenState, X11Error> {
        let x = &self.x;
        let a = &x.atoms;
        let active = x
            .get_u32s(x.root, a._NET_ACTIVE_WINDOW, AtomEnum::WINDOW)?
            .first()
            .copied()
            .filter(|w| *w != 0);
        let Some(active) = active else {
            return Ok(FullscreenState::NotFullscreen);
        };
        let states = x.get_u32s(active, a._NET_WM_STATE, AtomEnum::ATOM)?;
        if !states.contains(&a._NET_WM_STATE_FULLSCREEN) {
            return Ok(FullscreenState::NotFullscreen);
        }
        let geometry = x
            .conn
            .get_geometry(active)
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        let origin = x
            .conn
            .translate_coordinates(active, x.root, 0, 0)
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        let rect = PhysicalRect::new(
            i32::from(origin.dst_x),
            i32::from(origin.dst_y),
            u32::from(geometry.width),
            u32::from(geometry.height),
        );
        Ok(if window_covers_monitor(&rect, monitor) {
            FullscreenState::Fullscreen
        } else {
            FullscreenState::NotFullscreen
        })
    }
}

impl FullscreenProbe for X11FullscreenProbe {
    fn fullscreen_state(&self, monitor: &MonitorInfo) -> FullscreenState {
        self.query(&monitor.bounds).unwrap_or_else(|err| {
            // The active window can vanish between two requests; that is not
            // worth more than a debug line.
            tracing::debug!("X11 fullscreen query failed: {err}");
            FullscreenState::Unknown
        })
    }
}
