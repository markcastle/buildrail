//! X11 adapters (Linux/BSD), written against `x11rb`'s pure-Rust
//! connection: no Xlib, no system headers.
//!
//! Window ids are global to the X server, so these adapters open their own
//! connection (from `DISPLAY`) and operate on the window winit created,
//! identified by the id inside its `RawWindowHandle`.

mod dock;
mod fullscreen;
mod monitors;

use std::fmt::Display;
use std::sync::Arc;

use thiserror::Error;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, ClientMessageEvent, ConnectionExt as _, EventMask, PropMode, Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub use dock::X11Dock;
pub use fullscreen::X11FullscreenProbe;
pub use monitors::{workarea_for_desktop, X11Monitors};

use crate::dock::DockError;

mod atoms {
    #![allow(missing_docs)]
    x11rb::atom_manager! {
        pub Atoms: AtomsCookie {
            _NET_WM_WINDOW_TYPE,
            _NET_WM_WINDOW_TYPE_DOCK,
            _NET_WM_STRUT,
            _NET_WM_STRUT_PARTIAL,
            _NET_WM_STATE,
            _NET_WM_STATE_ABOVE,
            _NET_WM_STATE_STICKY,
            _NET_WM_STATE_FULLSCREEN,
            _NET_WM_DESKTOP,
            _NET_ACTIVE_WINDOW,
            _NET_WORKAREA,
            _NET_CURRENT_DESKTOP,
        }
    }
}
use atoms::Atoms;

/// `_NET_WM_DESKTOP` value meaning "on every desktop".
const ALL_DESKTOPS: u32 = 0xFFFF_FFFF;
/// EWMH `_NET_WM_STATE` client message action "add".
const NET_WM_STATE_ADD: u32 = 1;
/// EWMH source indication "normal application".
const SOURCE_APPLICATION: u32 = 1;
/// Longest property we read, in 32-bit units.
const MAX_PROPERTY_LEN: u32 = 1024;

/// Talking to the X server failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct X11Error(pub String);

impl From<X11Error> for DockError {
    fn from(err: X11Error) -> Self {
        DockError::X11(err.0)
    }
}

fn x11_err(err: impl Display) -> X11Error {
    X11Error(err.to_string())
}

/// A connection to the X server plus the EWMH atoms the adapters use.
/// Share one between the dock, the fullscreen probe and the monitor source
/// with `Arc`.
#[derive(Debug)]
pub struct X11Connection {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
}

impl X11Connection {
    /// Connects to `display` (`None` = the `DISPLAY` environment variable).
    pub fn connect(display: Option<&str>) -> Result<Arc<Self>, X11Error> {
        let (conn, screen) = x11rb::connect(display).map_err(x11_err)?;
        let root = conn
            .setup()
            .roots
            .get(screen)
            .map(|s| s.root)
            .ok_or_else(|| X11Error(format!("X server has no screen {screen}")))?;
        let atoms = Atoms::new(&conn)
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        Ok(Arc::new(Self { conn, root, atoms }))
    }

    /// The root window of the default screen.
    pub fn root(&self) -> Window {
        self.root
    }

    /// Size of the whole X screen (root window), queried live so RandR
    /// changes are seen.
    fn root_size(&self) -> Result<(u32, u32), X11Error> {
        let geo = self
            .conn
            .get_geometry(self.root)
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        Ok((u32::from(geo.width), u32::from(geo.height)))
    }

    /// Reads a 32-bit-format property; missing or differently-typed
    /// properties read as empty.
    fn get_u32s(
        &self,
        window: Window,
        property: Atom,
        kind: impl Into<Atom>,
    ) -> Result<Vec<u32>, X11Error> {
        let reply = self
            .conn
            .get_property(false, window, property, kind, 0, MAX_PROPERTY_LEN)
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        Ok(reply.value32().map(Iterator::collect).unwrap_or_default())
    }

    /// Replaces a 32-bit-format property and waits for the server to
    /// confirm (so a bad window id surfaces here, not later).
    fn set_u32s(
        &self,
        window: Window,
        property: Atom,
        kind: impl Into<Atom>,
        data: &[u32],
    ) -> Result<(), X11Error> {
        self.conn
            .change_property32(PropMode::REPLACE, window, property, kind, data)
            .map_err(x11_err)?
            .check()
            .map_err(x11_err)
    }

    fn delete(&self, window: Window, property: Atom) -> Result<(), X11Error> {
        self.conn
            .delete_property(window, property)
            .map_err(x11_err)?
            .check()
            .map_err(x11_err)
    }

    /// Sends an EWMH client message about `window` to the root window,
    /// where the window manager listens.
    fn client_message(&self, window: Window, kind: Atom, data: [u32; 5]) -> Result<(), X11Error> {
        let event = ClientMessageEvent::new(32, window, kind, data);
        self.conn
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .map_err(x11_err)?
            .check()
            .map_err(x11_err)
    }
}

/// `existing` plus every atom of `add` not already present, order kept.
pub(crate) fn merge_atoms(existing: &[u32], add: &[u32]) -> Vec<u32> {
    let mut merged = existing.to_vec();
    for atom in add {
        if !merged.contains(atom) {
            merged.push(*atom);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_atoms_keeps_order_and_skips_duplicates() {
        assert_eq!(merge_atoms(&[5, 7], &[7, 9, 9]), vec![5, 7, 9]);
        assert_eq!(merge_atoms(&[], &[1, 2]), vec![1, 2]);
    }

    #[test]
    fn errors_convert_to_dock_errors() {
        assert_eq!(
            DockError::from(X11Error("boom".into())),
            DockError::X11("boom".into())
        );
        assert_eq!(x11_err("x").to_string(), "x");
    }

    #[test]
    fn connecting_to_a_missing_display_fails_cleanly() {
        let err = X11Connection::connect(Some(":4097")).unwrap_err();
        assert!(!err.0.is_empty());
    }
}
