//! macOS shims (no `unsafe`: objc2's AppKit bindings are safe to call on
//! the main thread, which [`MainThreadMarker`] proves).

use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;

use crate::geometry::MonitorInfo;
use crate::monitors::{appkit_monitor, AppKitRect};

fn rect(r: objc2_foundation::NSRect) -> AppKitRect {
    AppKitRect {
        x: r.origin.x,
        y: r.origin.y,
        width: r.size.width,
        height: r.size.height,
    }
}

/// Every screen, the primary (menu bar) one first. `None` off the main
/// thread or without screens.
pub(crate) fn screens() -> Option<Vec<MonitorInfo>> {
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    let primary_height = screens.iter().next()?.frame().size.height;
    Some(
        screens
            .iter()
            .enumerate()
            .map(|(index, screen)| {
                appkit_monitor(
                    index,
                    rect(screen.frame()),
                    rect(screen.visibleFrame()),
                    primary_height,
                    screen.backingScaleFactor(),
                )
            })
            .collect(),
    )
}
