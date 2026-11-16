//! Integration tests against a real X server.
//!
//! Run with `xvfb-run -a cargo test -p buildrail-platform`. Without
//! `DISPLAY` every test returns early (and says so), so a plain
//! `cargo test` stays green on machines without X.
#![cfg(x11_platform)]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;

use buildrail_platform::dock::session::LinuxSession;
use buildrail_platform::dock::{DockError, DockOutcome, EdgeDock, LinuxDock, ReservationGuarantee};
use buildrail_platform::fullscreen::{FullscreenProbe, FullscreenState};
use buildrail_platform::geometry::{dock_rect, MonitorId, MonitorInfo, PhysicalRect};
use buildrail_platform::monitors::MonitorSource;
use buildrail_platform::x11::{X11Connection, X11Dock, X11FullscreenProbe, X11Monitors};
use raw_window_handle::{RawWindowHandle, XlibWindowHandle};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, PropMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// A test-side connection, independent of the code under test, the way
/// winit's connection is in the real app.
struct Harness {
    conn: RustConnection,
    root: Window,
    width: u32,
    height: u32,
}

fn harness() -> Option<Harness> {
    if std::env::var_os("DISPLAY").is_none_or(|d| d.is_empty()) {
        eprintln!("DISPLAY not set; skipping X11 test (run under xvfb-run -a)");
        return None;
    }
    let (conn, screen) =
        x11rb::connect(None).expect("DISPLAY is set but the X server is unreachable");
    let root = conn.setup().roots[screen].root;
    let geo = conn.get_geometry(root).unwrap().reply().unwrap();
    Some(Harness {
        conn,
        root,
        width: u32::from(geo.width),
        height: u32::from(geo.height),
    })
}

impl Harness {
    fn window(&self, map: bool) -> Window {
        let id = self.conn.generate_id().unwrap();
        self.conn
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                self.root,
                10,
                10,
                200,
                50,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &CreateWindowAux::new(),
            )
            .unwrap()
            .check()
            .unwrap();
        if map {
            self.conn.map_window(id).unwrap().check().unwrap();
        }
        id
    }

    fn atom(&self, name: &str) -> Atom {
        self.conn
            .intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    }

    fn prop(&self, window: Window, name: &str) -> Vec<u32> {
        let reply = self
            .conn
            .get_property(false, window, self.atom(name), AtomEnum::ANY, 0, 64)
            .unwrap()
            .reply()
            .unwrap();
        reply.value32().map(Iterator::collect).unwrap_or_default()
    }

    fn set_prop(&self, window: Window, name: &str, kind: AtomEnum, data: &[u32]) {
        self.conn
            .change_property32(PropMode::REPLACE, window, self.atom(name), kind, data)
            .unwrap()
            .check()
            .unwrap();
    }

    fn geometry(&self, window: Window) -> PhysicalRect {
        let g = self.conn.get_geometry(window).unwrap().reply().unwrap();
        PhysicalRect::new(
            i32::from(g.x),
            i32::from(g.y),
            u32::from(g.width),
            u32::from(g.height),
        )
    }

    fn monitor(&self, scale: f64) -> MonitorInfo {
        MonitorInfo::without_work_area(
            MonitorId::new("screen"),
            PhysicalRect::new(0, 0, self.width, self.height),
            scale,
            true,
        )
    }
}

fn handle(window: Window) -> RawWindowHandle {
    RawWindowHandle::Xlib(XlibWindowHandle::new(window.into()))
}

fn connection() -> Arc<X11Connection> {
    X11Connection::connect(None).unwrap()
}

#[test]
fn reserving_sets_dock_type_struts_state_and_geometry() {
    let Some(h) = harness() else { return };
    let window = h.window(false);
    let monitor = h.monitor(1.0);
    let mut dock = X11Dock::new(connection());

    let outcome = dock.reserve(handle(window), &monitor, 24).unwrap();
    let rect = PhysicalRect::new(0, h.height as i32 - 24, h.width, 24);
    assert_eq!(
        outcome,
        DockOutcome::Reserved {
            rect,
            guarantee: ReservationGuarantee::BestEffort
        }
    );
    assert_eq!(dock.current_rect(), Some(rect));

    assert_eq!(
        h.prop(window, "_NET_WM_WINDOW_TYPE"),
        vec![h.atom("_NET_WM_WINDOW_TYPE_DOCK")]
    );
    assert_eq!(
        h.prop(window, "_NET_WM_STRUT_PARTIAL"),
        vec![0, 0, 0, 24, 0, 0, 0, 0, 0, 0, 0, h.width - 1]
    );
    assert_eq!(h.prop(window, "_NET_WM_STRUT"), vec![0, 0, 0, 24]);
    let state = h.prop(window, "_NET_WM_STATE");
    assert!(state.contains(&h.atom("_NET_WM_STATE_ABOVE")));
    assert!(state.contains(&h.atom("_NET_WM_STATE_STICKY")));
    assert_eq!(h.prop(window, "_NET_WM_DESKTOP"), vec![0xFFFF_FFFF]);
    // No window manager under Xvfb, so the configure request applies as is.
    assert_eq!(h.geometry(window), rect);

    // A scale change (150 %) re-applies with a 36 px strut.
    let outcome = dock.update(&h.monitor(1.5), 24).unwrap();
    assert!(matches!(outcome, DockOutcome::Reserved { rect, .. } if rect.height == 36));
    assert_eq!(h.prop(window, "_NET_WM_STRUT"), vec![0, 0, 0, 36]);
    assert_eq!(h.geometry(window).height, 36);

    dock.release().unwrap();
    assert!(h.prop(window, "_NET_WM_STRUT_PARTIAL").is_empty());
    assert!(h.prop(window, "_NET_WM_STRUT").is_empty());
    assert_eq!(dock.update(&monitor, 24), Err(DockError::NotReserved));
    dock.release().unwrap();
}

#[test]
fn existing_wm_states_are_kept() {
    let Some(h) = harness() else { return };
    let window = h.window(false);
    let skip = h.atom("_NET_WM_STATE_SKIP_TASKBAR");
    h.set_prop(
        window,
        "_NET_WM_STATE",
        AtomEnum::ATOM,
        &[skip, h.atom("_NET_WM_STATE_ABOVE")],
    );
    let mut dock = X11Dock::new(connection());
    dock.reserve(handle(window), &h.monitor(1.0), 24).unwrap();
    assert_eq!(
        h.prop(window, "_NET_WM_STATE"),
        vec![
            skip,
            h.atom("_NET_WM_STATE_ABOVE"),
            h.atom("_NET_WM_STATE_STICKY")
        ]
    );
}

#[test]
fn mapped_windows_are_docked_through_client_messages() {
    let Some(h) = harness() else { return };
    let window = h.window(true);
    let mut dock = X11Dock::new(connection());
    let outcome = dock.reserve(handle(window), &h.monitor(1.25), 24).unwrap();
    assert!(matches!(outcome, DockOutcome::Reserved { rect, .. } if rect.height == 30));
    assert_eq!(h.prop(window, "_NET_WM_STRUT"), vec![0, 0, 0, 30]);
    // Once mapped, state belongs to the window manager; we must not write it.
    assert!(h.prop(window, "_NET_WM_STATE").is_empty());
}

#[test]
fn strip_does_not_climb_when_the_wm_shrinks_the_work_area() {
    let Some(h) = harness() else { return };
    let window = h.window(false);
    let mut monitor = h.monitor(1.0);
    let mut dock = X11Dock::new(connection());
    let DockOutcome::Reserved { rect: first, .. } =
        dock.reserve(handle(window), &monitor, 24).unwrap()
    else {
        panic!("not reserved");
    };
    // What an EWMH WM would now report as the work area.
    monitor.work_area = PhysicalRect::from_edges(0, 0, h.width as i32, first.y);
    let DockOutcome::Reserved { rect: again, .. } = dock.update(&monitor, 24).unwrap() else {
        panic!("not reserved");
    };
    assert_eq!(again, first);
    assert_eq!(dock_rect(&monitor, 24, Some(&first)), first);
}

#[test]
fn redocking_another_window_releases_the_first() {
    let Some(h) = harness() else { return };
    let (a, b) = (h.window(false), h.window(false));
    let mut dock = X11Dock::new(connection());
    dock.reserve(handle(a), &h.monitor(1.0), 24).unwrap();
    dock.reserve(handle(b), &h.monitor(1.0), 24).unwrap();
    assert!(h.prop(a, "_NET_WM_STRUT").is_empty());
    assert_eq!(h.prop(b, "_NET_WM_STRUT"), vec![0, 0, 0, 24]);
    drop(dock);
    assert!(h.prop(b, "_NET_WM_STRUT").is_empty(), "drop releases");
}

#[test]
fn errors_are_reported_not_panicked() {
    let Some(h) = harness() else { return };
    let mut dock = X11Dock::new(connection());
    // A window id that does not exist.
    let bogus = h.window(false) + 1000;
    assert!(matches!(
        dock.reserve(handle(bogus), &h.monitor(1.0), 24),
        Err(DockError::X11(_))
    ));
    // A monitor outside the X screen cannot be expressed as a strut.
    let window = h.window(false);
    let off_screen = MonitorInfo::without_work_area(
        MonitorId::new("x"),
        PhysicalRect::new(-500, 0, 400, 300),
        1.0,
        false,
    );
    assert!(matches!(
        dock.reserve(handle(window), &off_screen, 24),
        Err(DockError::InvalidGeometry(_))
    ));
}

#[test]
fn linux_dock_routes_x11_windows_to_the_x11_dock() {
    let Some(h) = harness() else { return };
    let window = h.window(false);
    let mut dock = LinuxDock::new(LinuxSession::Wayland { xwayland: true });
    let outcome = dock.reserve(handle(window), &h.monitor(1.0), 24).unwrap();
    assert!(matches!(outcome, DockOutcome::Reserved { .. }));
    assert!(dock.update(&h.monitor(1.0), 24).is_ok());
    assert_eq!(h.prop(window, "_NET_WM_STRUT"), vec![0, 0, 0, 24]);
    dock.release().unwrap();
    assert!(h.prop(window, "_NET_WM_STRUT").is_empty());
}

#[test]
fn fullscreen_probe_follows_the_active_window() {
    let Some(h) = harness() else { return };
    let window = h.window(true);
    let probe = X11FullscreenProbe::new(connection());
    let monitor = h.monitor(1.0);

    h.set_prop(h.root, "_NET_ACTIVE_WINDOW", AtomEnum::WINDOW, &[0]);
    assert_eq!(
        probe.fullscreen_state(&monitor),
        FullscreenState::NotFullscreen
    );

    h.set_prop(h.root, "_NET_ACTIVE_WINDOW", AtomEnum::WINDOW, &[window]);
    assert_eq!(
        probe.fullscreen_state(&monitor),
        FullscreenState::NotFullscreen,
        "not fullscreen state"
    );

    h.set_prop(
        window,
        "_NET_WM_STATE",
        AtomEnum::ATOM,
        &[h.atom("_NET_WM_STATE_FULLSCREEN")],
    );
    assert_eq!(
        probe.fullscreen_state(&monitor),
        FullscreenState::NotFullscreen,
        "fullscreen state but still 200x50"
    );

    let aux = x11rb::protocol::xproto::ConfigureWindowAux::new()
        .x(0)
        .y(0)
        .width(h.width)
        .height(h.height);
    h.conn
        .configure_window(window, &aux)
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(
        probe.fullscreen_state(&monitor),
        FullscreenState::Fullscreen
    );

    let elsewhere = MonitorInfo::without_work_area(
        MonitorId::new("other"),
        PhysicalRect::new(h.width as i32, 0, 1920, 1080),
        1.0,
        false,
    );
    assert_eq!(
        probe.fullscreen_state(&elsewhere),
        FullscreenState::NotFullscreen
    );

    // An active window that has gone away gives Unknown, not a panic.
    h.set_prop(
        h.root,
        "_NET_ACTIVE_WINDOW",
        AtomEnum::WINDOW,
        &[window + 1000],
    );
    assert_eq!(probe.fullscreen_state(&monitor), FullscreenState::Unknown);
    h.set_prop(h.root, "_NET_ACTIVE_WINDOW", AtomEnum::WINDOW, &[0]);
}

#[test]
fn monitors_come_from_randr_with_the_workarea_clipped() {
    let Some(h) = harness() else { return };
    let source = X11Monitors::new(connection(), 1.5);
    let monitors = source.monitors().unwrap();
    assert!(!monitors.is_empty());
    for m in &monitors {
        assert!(
            PhysicalRect::new(0, 0, h.width, h.height).contains_rect(&m.bounds),
            "{m:?}"
        );
        assert_eq!(m.scale_factor, 1.5);
        assert!(!m.id.as_str().is_empty());
    }

    let reduced = h.height - 40;
    h.set_prop(
        h.root,
        "_NET_WORKAREA",
        AtomEnum::CARDINAL,
        &[0, 0, h.width, reduced],
    );
    let monitors = source.monitors().unwrap();
    assert!(monitors
        .iter()
        .any(|m| m.work_area.bottom() == reduced as i32));
    h.conn
        .delete_property(h.root, h.atom("_NET_WORKAREA"))
        .unwrap()
        .check()
        .unwrap();
}

#[test]
fn platform_factories_work_under_x11() {
    let Some(h) = harness() else { return };
    if buildrail_platform::dock::session::current_session().is_wayland() {
        return;
    }
    let source =
        buildrail_platform::monitors::platform_monitor_source(1.0).expect("X11 monitor source");
    assert!(!source.monitors().unwrap().is_empty());
    let window = h.window(false);
    let probe = buildrail_platform::fullscreen::platform_fullscreen_probe(Some(&handle(window)));
    let _ = probe.fullscreen_state(&h.monitor(1.0));
    let mut dock = buildrail_platform::dock::platform_dock();
    assert!(matches!(
        dock.reserve(handle(window), &h.monitor(1.0), 24),
        Ok(DockOutcome::Reserved { .. })
    ));
}
