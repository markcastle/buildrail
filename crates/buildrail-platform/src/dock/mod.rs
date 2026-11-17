//! Reserving a screen edge for the strip ("docking").
//!
//! [`EdgeDock`] is the one trait the UI talks to. [`platform_dock`] returns
//! the right implementation for the running OS; every call either reserves
//! the bottom edge ([`DockOutcome::Reserved`]) or says, with a reason, that
//! this desktop cannot ([`DockOutcome::Unsupported`]), in which case the UI
//! shows the compact movable window instead (placed with
//! [`crate::geometry::float_rect`]).
//!
//! # Per-OS behaviour
//!
//! **Windows** – a real *AppBar* (`SHAppBarMessage`). The shell places the
//! strip directly above the taskbar (or at the screen bottom when the
//! taskbar is elsewhere) and shrinks the work area so maximised windows stop
//! above the strip. The dock moves the window itself (`SetWindowPos`,
//! topmost, no activation). Result: [`ReservationGuarantee::Enforced`].
//! The space is given back on [`EdgeDock::release`] and on drop; if the
//! process is killed, Explorer reclaims it when the window is destroyed.
//!
//! **Linux/BSD on X11** – the window is marked `_NET_WM_WINDOW_TYPE_DOCK`,
//! given `_NET_WM_STRUT_PARTIAL` and `_NET_WM_STRUT` for the bottom edge,
//! made sticky (all workspaces) and kept above, and moved to the computed
//! rect. Whether the work area actually shrinks is up to the window
//! manager: GNOME/Mutter, KDE/KWin, Xfwm, Openbox, i3 and most others honour
//! struts. Result: [`ReservationGuarantee::BestEffort`].
//!
//! **Linux under XWayland** (an X11 window in a Wayland session) – the same
//! properties are set, but the Wayland compositor decides; GNOME ignores
//! struts of X11 clients for the Wayland work area, KDE honours them. Also
//! best effort.
//!
//! **Linux on Wayland** – unsupported. Reserving an edge on Wayland needs
//! the `wlr-layer-shell` protocol (KDE, Sway, Hyprland, COSMIC; not GNOME),
//! and a surface has to be created as a layer surface from the start: a
//! winit toplevel cannot be converted. A future layer-shell backend
//! (for example via `smithay-client-toolkit`, owning its own surface) can
//! implement this same trait. Note that Wayland compositors also ignore
//! requested window positions, so the floating window's remembered position
//! cannot be applied there either; the UI may prefer to force winit's X11
//! backend (XWayland) when placement matters more than native Wayland.
//!
//! **macOS** – unsupported. There is no public API that reserves a screen
//! edge (the menu bar and Dock are private to the system), so the UI uses the
//! floating window, typically with `WindowLevel::AlwaysOnTop` and placed
//! just above the Dock: [`crate::monitors::main_thread_monitors`] reports
//! each screen's `NSScreen.visibleFrame` as its work area (winit does not
//! expose it), and [`crate::geometry::float_rect`] places the window
//! inside it.
//!
//! # Wiring the UI (eframe/winit)
//!
//! 1. Create the viewport with decorations off, `with_inner_size([w, 24.0])`,
//!    and on X11 ideally hidden (`with_visible(false)`): some window managers
//!    read `_NET_WM_WINDOW_TYPE` only when a window is first mapped. egui's
//!    `ViewportBuilder::with_window_type(X11WindowType::Dock)` sets the same
//!    type at creation and is a good belt-and-braces addition.
//! 2. Get the native handle with [`raw_window_handle_of`] (eframe's `Frame`
//!    implements `HasWindowHandle`), pick the monitor (a [`crate::monitors::MonitorSource`]
//!    or winit monitors + [`crate::geometry::resolve_monitor`]) and call
//!    [`EdgeDock::reserve`] with [`crate::geometry::STRIP_HEIGHT_LOGICAL`].
//! 3. On `Reserved { rect, .. }` show the window. The dock has already
//!    moved and sized it to `rect` (physical pixels); do not send position
//!    or size viewport commands while docked. On `Unsupported` place the
//!    floating window yourself and keep it movable.
//! 4. Call [`EdgeDock::update`] when the scale factor or monitor layout
//!    changes, **and every few seconds as a polling fallback** (it is cheap
//!    and only re-applies when the rect changes). This is required on
//!    Windows because eframe gives no direct access to the window
//!    procedure, see below.
//! 5. Call [`EdgeDock::release`] before switching to the floating window
//!    and on exit (it also runs on drop).
//!
//! ## Windows callback messages
//!
//! The shell notifies an AppBar through a private window message whose id
//! is [`native_callback_message`] (registered with
//! `RegisterWindowMessageW`, so it is the same in any thread and can be
//! read before the window exists). To react immediately instead of on the
//! next poll, install a message hook when building eframe's event loop:
//!
//! ```ignore
//! let id = buildrail_platform::dock::native_callback_message();
//! let (tx, rx) = std::sync::mpsc::channel();
//! options.event_loop_builder = Some(Box::new(move |builder| {
//!     use winit::platform::windows::EventLoopBuilderExtWindows;
//!     builder.with_msg_hook(move |msg| {
//!         // SAFETY: winit passes a valid `*const MSG`.
//!         let msg = unsafe { &*(msg as *const windows_sys::Win32::UI::WindowsAndMessaging::MSG) };
//!         if Some(msg.message) == id {
//!             let _ = tx.send((msg.message, msg.wParam, msg.lParam));
//!         }
//!         false // let winit process it as usual
//!     });
//! }));
//! // In App::update: while let Ok((m, w, l)) = rx.try_recv() {
//! //     if let Some(event) = dock.handle_native_message(m, w, l) { ... }
//! // }
//! ```
//!
//! The message hook only sees *posted* messages; if the shell delivers a
//! notification with `SendMessage` instead, the hook misses it and the
//! periodic [`EdgeDock::update`] (plus [`crate::fullscreen`] polling)
//! covers it. Either way nothing breaks; the hook only makes the reaction
//! faster.

mod appbar;
#[cfg(x11_platform)]
mod linux;
pub mod session;
pub mod strut;
mod window_ref;

use std::sync::{Arc, Mutex, PoisonError};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use thiserror::Error;

use crate::geometry::{dock_rect, MonitorInfo, PhysicalRect};

pub use appbar::{
    after_query, decode_notification, query_proposal, AppBarNotification, ABN_FULLSCREENAPP,
    ABN_POSCHANGED, ABN_STATECHANGE, ABN_WINDOWARRANGE,
};
#[cfg(x11_platform)]
pub use linux::LinuxDock;
pub use window_ref::WindowRef;

/// Reserves (or declines to reserve) the bottom screen edge for the strip.
/// See the module docs for per-OS behaviour and how the UI wires it.
pub trait EdgeDock: Send {
    /// Docks `window` at the bottom of `monitor`, `strip_height_logical`
    /// logical pixels tall. Calling it again re-docks (for example on
    /// another monitor).
    fn reserve(
        &mut self,
        window: RawWindowHandle,
        monitor: &MonitorInfo,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError>;

    /// Re-applies the reservation after a layout, taskbar or scale change,
    /// or as a periodic poll. Only touches the window when the rect
    /// changed. Fails with [`DockError::NotReserved`] before
    /// [`EdgeDock::reserve`].
    fn update(
        &mut self,
        monitor: &MonitorInfo,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError>;

    /// Gives the reserved space back. Releasing when nothing is reserved is
    /// `Ok`.
    fn release(&mut self) -> Result<(), DockError>;

    /// Feeds a native window message to the dock (Windows AppBar callback,
    /// see the module docs). Other platforms ignore it and return `None`.
    fn handle_native_message(
        &mut self,
        _message: u32,
        _wparam: usize,
        _lparam: isize,
    ) -> Option<DockEvent> {
        None
    }
}

/// What [`EdgeDock::reserve`] / [`EdgeDock::update`] achieved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockOutcome {
    /// The bottom edge is reserved and the window has been moved to `rect`
    /// (physical pixels, global desktop coordinates).
    Reserved {
        /// Where the strip now is.
        rect: PhysicalRect,
        /// How sure we are that other windows will stay out of the way.
        guarantee: ReservationGuarantee,
    },
    /// This desktop cannot reserve an edge; use the floating window.
    Unsupported {
        /// Why, for logs and a settings-page explanation.
        reason: UnsupportedReason,
    },
}

/// Whether a reservation is enforced by the OS or only requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReservationGuarantee {
    /// The OS shell confirmed it (Windows AppBar).
    Enforced,
    /// Requested from the window manager, which may ignore it (X11 struts,
    /// XWayland).
    BestEffort,
}

/// Why edge reservation is not available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
pub enum UnsupportedReason {
    /// macOS has no public API to reserve a screen edge.
    #[error("macOS does not let apps reserve a screen edge; using a floating window")]
    MacOs,
    /// Native Wayland toplevels cannot reserve an edge.
    #[error(
        "Wayland windows cannot reserve a screen edge (that needs a layer-shell surface); using a floating window"
    )]
    Wayland,
    /// The window handle is of a kind this dock does not understand.
    #[error("this kind of window cannot be docked; using a floating window")]
    UnsupportedWindowHandle,
    /// No dock implementation exists for this operating system.
    #[error("docking is not available on this operating system; using a floating window")]
    OtherPlatform,
}

/// Something the dock noticed while handling a native message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockEvent {
    /// The shell asked us to re-query; the strip was moved to `rect`.
    Repositioned {
        /// The new strip rect.
        rect: PhysicalRect,
    },
    /// A fullscreen app opened on the strip's monitor: hide the strip.
    FullscreenEntered,
    /// The fullscreen app closed: show the strip again.
    FullscreenLeft,
}

/// Docking failed outright (as opposed to being unsupported).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DockError {
    /// [`EdgeDock::update`] was called before [`EdgeDock::reserve`].
    #[error("the strip is not docked")]
    NotReserved,
    /// The toolkit could not provide a native window handle.
    #[error("no native window handle: {0}")]
    WindowHandle(String),
    /// Talking to the X server failed.
    #[error("X11 error: {0}")]
    X11(String),
    /// A Win32 call failed.
    #[error("{operation} failed (Win32 error {code})")]
    Win32 {
        /// The API that failed.
        operation: &'static str,
        /// `GetLastError` / `HRESULT` value.
        code: i64,
    },
    /// The computed rect cannot be expressed (for example a strip outside
    /// the X screen).
    #[error("cannot dock at {0}")]
    InvalidGeometry(PhysicalRect),
}

/// Gets the raw handle from anything that has one (eframe's `Frame`, a
/// winit `Window`).
pub fn raw_window_handle_of(window: &impl HasWindowHandle) -> Result<RawWindowHandle, DockError> {
    window
        .window_handle()
        .map(|handle| handle.as_raw())
        .map_err(|err| DockError::WindowHandle(err.to_string()))
}

/// The X11 window id inside an Xlib or XCB handle.
// `c_ulong` is `u32` on Windows and `u64` on LP64 Unix, so the conversion
// is only "useless" on some targets.
#[allow(clippy::useless_conversion)]
pub fn x11_window_id(handle: &RawWindowHandle) -> Option<u32> {
    match handle {
        RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok().filter(|id| *id != 0),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None,
    }
}

/// The `HWND` inside a Win32 handle, as an integer.
pub fn win32_hwnd(handle: &RawWindowHandle) -> Option<isize> {
    match handle {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// The id of the private window message the Windows shell sends AppBar
/// notifications with. `None` on other platforms. See the module docs.
pub fn native_callback_message() -> Option<u32> {
    #[cfg(windows)]
    {
        crate::win32::appbar::callback_message()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The edge dock for the running OS. See the module docs.
pub fn platform_dock() -> Box<dyn EdgeDock> {
    #[cfg(windows)]
    {
        Box::new(crate::win32::appbar::WindowsAppBar::new())
    }
    #[cfg(x11_platform)]
    {
        Box::new(LinuxDock::new(session::current_session()))
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(UnsupportedDock::new(UnsupportedReason::MacOs))
    }
    #[cfg(not(any(windows, x11_platform, target_os = "macos")))]
    {
        Box::new(UnsupportedDock::new(UnsupportedReason::OtherPlatform))
    }
}

/// A dock that always answers [`DockOutcome::Unsupported`]. Used on macOS
/// and as a building block for platforms without docking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedDock {
    reason: UnsupportedReason,
}

impl UnsupportedDock {
    /// A dock that declines with `reason`.
    pub fn new(reason: UnsupportedReason) -> Self {
        Self { reason }
    }
}

impl EdgeDock for UnsupportedDock {
    fn reserve(
        &mut self,
        _: RawWindowHandle,
        _: &MonitorInfo,
        _: u32,
    ) -> Result<DockOutcome, DockError> {
        Ok(DockOutcome::Unsupported {
            reason: self.reason,
        })
    }

    fn update(&mut self, _: &MonitorInfo, _: u32) -> Result<DockOutcome, DockError> {
        Ok(DockOutcome::Unsupported {
            reason: self.reason,
        })
    }

    fn release(&mut self) -> Result<(), DockError> {
        Ok(())
    }
}

/// One call recorded by [`ScriptedDock`].
#[derive(Debug, Clone, PartialEq)]
pub enum DockCall {
    /// [`EdgeDock::reserve`] on this monitor with this height.
    Reserve {
        /// Monitor passed in.
        monitor: MonitorInfo,
        /// Strip height passed in.
        strip_height_logical: u32,
    },
    /// [`EdgeDock::update`].
    Update {
        /// Monitor passed in.
        monitor: MonitorInfo,
        /// Strip height passed in.
        strip_height_logical: u32,
    },
    /// [`EdgeDock::release`].
    Release,
}

/// A fake dock for UI tests: either "reserves" using the real geometry
/// ([`crate::geometry::dock_rect`]) or declines, and records every call.
#[derive(Debug, Clone)]
pub struct ScriptedDock {
    unsupported: Option<UnsupportedReason>,
    reserved: bool,
    calls: Arc<Mutex<Vec<DockCall>>>,
}

impl ScriptedDock {
    /// A dock that succeeds like the Windows AppBar would.
    pub fn reserving() -> Self {
        Self {
            unsupported: None,
            reserved: false,
            calls: Arc::default(),
        }
    }

    /// A dock that declines with `reason`, like macOS or Wayland.
    pub fn unsupported(reason: UnsupportedReason) -> Self {
        Self {
            unsupported: Some(reason),
            ..Self::reserving()
        }
    }

    /// The calls made so far (shared with clones, so keep a clone before
    /// boxing the dock).
    pub fn calls(&self) -> Vec<DockCall> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn record(&self, call: DockCall) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
    }

    fn outcome(&self, monitor: &MonitorInfo, height: u32) -> DockOutcome {
        match self.unsupported {
            Some(reason) => DockOutcome::Unsupported { reason },
            None => DockOutcome::Reserved {
                rect: dock_rect(monitor, height, None),
                guarantee: ReservationGuarantee::Enforced,
            },
        }
    }
}

impl EdgeDock for ScriptedDock {
    fn reserve(
        &mut self,
        _: RawWindowHandle,
        monitor: &MonitorInfo,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError> {
        self.record(DockCall::Reserve {
            monitor: monitor.clone(),
            strip_height_logical,
        });
        self.reserved = self.unsupported.is_none();
        Ok(self.outcome(monitor, strip_height_logical))
    }

    fn update(
        &mut self,
        monitor: &MonitorInfo,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError> {
        self.record(DockCall::Update {
            monitor: monitor.clone(),
            strip_height_logical,
        });
        if self.unsupported.is_none() && !self.reserved {
            return Err(DockError::NotReserved);
        }
        Ok(self.outcome(monitor, strip_height_logical))
    }

    fn release(&mut self) -> Result<(), DockError> {
        self.record(DockCall::Release);
        self.reserved = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::MonitorId;
    use raw_window_handle::{
        AppKitWindowHandle, HandleError, WaylandWindowHandle, Win32WindowHandle, WindowHandle,
        XcbWindowHandle, XlibWindowHandle,
    };
    use std::num::{NonZeroIsize, NonZeroU32};
    use std::ptr::NonNull;

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new("m"),
            bounds: PhysicalRect::new(0, 0, 1920, 1080),
            work_area: PhysicalRect::new(0, 0, 1920, 1040),
            scale_factor: 1.0,
            is_primary: true,
        }
    }

    fn xlib(id: u64) -> RawWindowHandle {
        RawWindowHandle::Xlib(XlibWindowHandle::new(id as _))
    }

    #[test]
    fn extracts_native_ids() {
        assert_eq!(x11_window_id(&xlib(0x40_0001)), Some(0x40_0001));
        assert_eq!(x11_window_id(&xlib(0)), None);
        let xcb = RawWindowHandle::Xcb(XcbWindowHandle::new(NonZeroU32::new(7).unwrap()));
        assert_eq!(x11_window_id(&xcb), Some(7));
        let win =
            RawWindowHandle::Win32(Win32WindowHandle::new(NonZeroIsize::new(0x1234).unwrap()));
        assert_eq!(win32_hwnd(&win), Some(0x1234));
        assert_eq!(x11_window_id(&win), None);
        assert_eq!(win32_hwnd(&xcb), None);
        let mut dummy = 0_u8;
        let wl =
            RawWindowHandle::Wayland(WaylandWindowHandle::new(NonNull::from(&mut dummy).cast()));
        assert_eq!(x11_window_id(&wl), None);
    }

    struct HasHandle(Option<RawWindowHandle>);

    impl HasWindowHandle for HasHandle {
        #[allow(unsafe_code)]
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            match self.0 {
                // SAFETY: test-only; the handle is never dereferenced.
                Some(raw) => Ok(unsafe { WindowHandle::borrow_raw(raw) }),
                None => Err(HandleError::Unavailable),
            }
        }
    }

    #[test]
    fn raw_handle_helper() {
        assert_eq!(raw_window_handle_of(&HasHandle(Some(xlib(5)))), Ok(xlib(5)));
        assert!(matches!(
            raw_window_handle_of(&HasHandle(None)),
            Err(DockError::WindowHandle(_))
        ));
    }

    #[test]
    fn unsupported_dock_always_declines() {
        let mut dock = UnsupportedDock::new(UnsupportedReason::MacOs);
        let mut dummy = 0_u8;
        let handle =
            RawWindowHandle::AppKit(AppKitWindowHandle::new(NonNull::from(&mut dummy).cast()));
        let expected = DockOutcome::Unsupported {
            reason: UnsupportedReason::MacOs,
        };
        assert_eq!(dock.reserve(handle, &monitor(), 24), Ok(expected.clone()));
        assert_eq!(dock.update(&monitor(), 24), Ok(expected));
        assert_eq!(dock.release(), Ok(()));
        assert_eq!(dock.handle_native_message(1, 2, 3), None);
    }

    #[test]
    fn scripted_dock_records_and_reserves() {
        let mut dock = ScriptedDock::reserving();
        let probe = dock.clone();
        assert_eq!(dock.update(&monitor(), 24), Err(DockError::NotReserved));
        let outcome = dock.reserve(xlib(1), &monitor(), 24).unwrap();
        assert_eq!(
            outcome,
            DockOutcome::Reserved {
                rect: PhysicalRect::new(0, 1016, 1920, 24),
                guarantee: ReservationGuarantee::Enforced
            }
        );
        assert!(dock.update(&monitor(), 24).is_ok());
        dock.release().unwrap();
        let calls = probe.calls();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[3], DockCall::Release);
        assert!(matches!(
            calls[1],
            DockCall::Reserve {
                strip_height_logical: 24,
                ..
            }
        ));
    }

    #[test]
    fn scripted_dock_can_decline() {
        let mut dock = ScriptedDock::unsupported(UnsupportedReason::Wayland);
        assert_eq!(
            dock.update(&monitor(), 24),
            Ok(DockOutcome::Unsupported {
                reason: UnsupportedReason::Wayland
            })
        );
        assert!(matches!(
            dock.reserve(xlib(1), &monitor(), 24),
            Ok(DockOutcome::Unsupported { .. })
        ));
    }

    #[test]
    fn reasons_and_errors_read_well() {
        assert!(UnsupportedReason::Wayland
            .to_string()
            .contains("layer-shell"));
        assert!(UnsupportedReason::MacOs.to_string().contains("macOS"));
        assert!(UnsupportedReason::UnsupportedWindowHandle
            .to_string()
            .contains("floating"));
        assert!(UnsupportedReason::OtherPlatform
            .to_string()
            .contains("floating"));
        let err = DockError::Win32 {
            operation: "SHAppBarMessage(ABM_NEW)",
            code: 5,
        };
        assert_eq!(
            err.to_string(),
            "SHAppBarMessage(ABM_NEW) failed (Win32 error 5)"
        );
        assert_eq!(
            DockError::InvalidGeometry(PhysicalRect::new(1, 2, 3, 4)).to_string(),
            "cannot dock at 3x4 at (1, 2)"
        );
        assert_eq!(
            DockError::NotReserved.to_string(),
            "the strip is not docked"
        );
        assert_eq!(DockError::X11("x".into()).to_string(), "X11 error: x");
    }

    #[test]
    fn platform_dock_exists_and_callback_is_windows_only() {
        let _dock = platform_dock();
        assert_eq!(native_callback_message().is_some(), cfg!(windows));
    }
}
