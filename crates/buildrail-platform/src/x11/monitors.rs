//! X11 monitor listing via RandR 1.5. See [`crate::monitors`].

use std::sync::Arc;

use x11rb::connection::RequestConnection;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

use super::{x11_err, X11Connection, X11Error};
use crate::geometry::{MonitorId, MonitorInfo, PhysicalRect};
use crate::monitors::{MonitorError, MonitorSource};

/// Lists RandR monitors with work areas from `_NET_WORKAREA`.
#[derive(Debug, Clone)]
pub struct X11Monitors {
    x: Arc<X11Connection>,
    scale_factor: f64,
}

impl X11Monitors {
    /// A source using `connection`. X11 has no per-monitor scale, so every
    /// monitor gets `scale_factor` (pass winit's).
    pub fn new(connection: Arc<X11Connection>, scale_factor: f64) -> Self {
        Self {
            x: connection,
            scale_factor,
        }
    }

    fn list(&self) -> Result<Vec<MonitorInfo>, X11Error> {
        let x = &self.x;
        let a = &x.atoms;
        let desktop = x
            .get_u32s(x.root, a._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL)?
            .first()
            .copied()
            .unwrap_or(0);
        let workareas = x.get_u32s(x.root, a._NET_WORKAREA, AtomEnum::CARDINAL)?;
        let workarea = workarea_for_desktop(&workareas, desktop);
        let with_work = |id: MonitorId, bounds: PhysicalRect, primary: bool| MonitorInfo {
            id,
            bounds,
            work_area: workarea
                .and_then(|w| w.intersection(&bounds))
                .unwrap_or(bounds),
            scale_factor: self.scale_factor,
            is_primary: primary,
        };

        let has_randr = x
            .conn
            .extension_information(randr::X11_EXTENSION_NAME)
            .map_err(x11_err)?
            .is_some();
        let monitors = if has_randr {
            x.conn
                .randr_get_monitors(x.root, true)
                .map_err(x11_err)?
                .reply()
                .map(|r| r.monitors)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if monitors.is_empty() {
            let (width, height) = x.root_size()?;
            return Ok(vec![with_work(
                MonitorId::new("screen"),
                PhysicalRect::new(0, 0, width, height),
                true,
            )]);
        }
        monitors
            .iter()
            .map(|m| {
                let name = x
                    .conn
                    .get_atom_name(m.name)
                    .map_err(x11_err)?
                    .reply()
                    .map_err(x11_err)?;
                let bounds = PhysicalRect::new(
                    i32::from(m.x),
                    i32::from(m.y),
                    u32::from(m.width),
                    u32::from(m.height),
                );
                Ok(with_work(
                    MonitorId::new(String::from_utf8_lossy(&name.name).into_owned()),
                    bounds,
                    m.primary,
                ))
            })
            .collect()
    }
}

impl MonitorSource for X11Monitors {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, MonitorError> {
        self.list().map_err(|err| MonitorError(err.0))
    }
}

/// The work area of `desktop` from a `_NET_WORKAREA` value (four
/// CARDINALs per desktop). Falls back to desktop 0 when `desktop` has no
/// entry; `None` when the property is empty or malformed.
pub fn workarea_for_desktop(values: &[u32], desktop: u32) -> Option<PhysicalRect> {
    let (chunks, _) = values.as_chunks::<4>();
    let [x, y, w, h] = usize::try_from(desktop)
        .ok()
        .and_then(|i| chunks.get(i))
        .or_else(|| chunks.first())?;
    Some(PhysicalRect::new(
        i32::try_from(*x).ok()?,
        i32::try_from(*y).ok()?,
        *w,
        *h,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workarea_picks_the_current_desktop() {
        let values = [0, 0, 1920, 1040, 0, 32, 1920, 1048];
        assert_eq!(
            workarea_for_desktop(&values, 1),
            Some(PhysicalRect::new(0, 32, 1920, 1048))
        );
        assert_eq!(
            workarea_for_desktop(&values, 0),
            Some(PhysicalRect::new(0, 0, 1920, 1040))
        );
        assert_eq!(
            workarea_for_desktop(&values, 9),
            Some(PhysicalRect::new(0, 0, 1920, 1040))
        );
        assert_eq!(workarea_for_desktop(&[], 0), None);
        assert_eq!(workarea_for_desktop(&[1, 2, 3], 0), None);
        assert_eq!(workarea_for_desktop(&[u32::MAX, 0, 1, 1], 0), None);
    }
}
