//! Integration tests of the real Windows AppBar against the real shell.
//!
//! Each test opens a real top-level window on its own thread (with a
//! message loop, as winit's is) and docks it with
//! [`buildrail_platform::windows::windows_appbar`], then checks what
//! Explorer and the window really did: the work area
//! (`SPI_GETWORKAREA`) and the window's rect (`GetWindowRect`).
//!
//! They need a desktop session with Explorer running (GitHub's
//! `windows-latest` runners have one). Without a shell (`Shell_TrayWnd`
//! not found) every test prints why and returns, so they never fail for
//! environmental reasons. Every test prints `RAN` or `SKIPPED`; CI runs
//! them with `--nocapture` so the log shows which.
//!
//! The tests share one desktop, so they take turns ([`Desktop`]).
#![cfg(windows)]
#![deny(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use buildrail_platform::dock::{DockOutcome, Edge, EdgeDock, ReservationGuarantee};
use buildrail_platform::geometry::{MonitorInfo, PhysicalRect};
use buildrail_platform::monitors::MonitorSource;
use buildrail_platform::windows::{windows_appbar, Win32Monitors};

/// The little FFI the tests need that the crate does not export: a real
/// window with a message loop, the work area, and reading/moving a
/// window. Kept small; each `unsafe` block says why it is sound.
#[allow(unsafe_code)]
mod desktop {
    #![warn(clippy::undocumented_unsafe_blocks)]

    use std::num::NonZeroIsize;
    use std::sync::mpsc;
    use std::sync::Once;
    use std::thread::JoinHandle;

    use buildrail_platform::geometry::PhysicalRect;
    use raw_window_handle::{RawWindowHandle, Win32WindowHandle};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, GetMessageW, GetWindowRect,
        PostMessageW, PostQuitMessage, RegisterClassW, SetWindowPos, SystemParametersInfoW,
        TranslateMessage, MSG, SPI_GETWORKAREA, SWP_NOACTIVATE, SWP_NOZORDER, WM_CLOSE, WM_DESTROY,
        WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn rect(r: &RECT) -> PhysicalRect {
        PhysicalRect::from_edges(r.left, r.top, r.right, r.bottom)
    }

    /// `true` when Explorer's taskbar exists (there is a shell to talk to).
    pub fn shell_running() -> bool {
        let class = wide("Shell_TrayWnd");
        // SAFETY: `class` is a NUL-terminated UTF-16 string alive for the
        // call; a null window name matches any.
        let tray = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        !tray.is_null()
    }

    /// Physical pixels everywhere, as BuildRail's process (winit makes it
    /// per-monitor aware). Fails harmlessly when already set.
    pub fn per_monitor_dpi_aware() {
        // SAFETY: plain value argument.
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    }

    /// The primary monitor's work area.
    pub fn work_area() -> PhysicalRect {
        let mut area = RECT::default();
        // SAFETY: SPI_GETWORKAREA writes one RECT to the pointer, which is
        // valid and writable for the call.
        let ok = unsafe {
            SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut area as *mut RECT).cast(), 0)
        };
        assert_ne!(ok, 0, "SystemParametersInfoW(SPI_GETWORKAREA) failed");
        rect(&area)
    }

    /// Where the window really is.
    pub fn window_rect(hwnd: isize) -> Option<PhysicalRect> {
        let mut r = RECT::default();
        // SAFETY: `r` is a valid, writable RECT; a stale handle only makes
        // the call fail.
        let ok = unsafe { GetWindowRect(hwnd as HWND, &mut r) };
        (ok != 0).then(|| rect(&r))
    }

    /// Moves the window from this (another) thread and waits until its
    /// thread has done it, as something else moving our strip would.
    pub fn move_window(hwnd: isize, to: &PhysicalRect) {
        // SAFETY: plain value arguments; a stale handle only makes it fail.
        let ok = unsafe {
            SetWindowPos(
                hwnd as HWND,
                std::ptr::null_mut(),
                to.x,
                to.y,
                to.width as i32,
                to.height as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        };
        assert_ne!(ok, 0, "SetWindowPos failed");
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_DESTROY {
            // SAFETY: called on the window's own thread, which owns the
            // message loop the quit message ends.
            unsafe { PostQuitMessage(0) };
            return 0;
        }
        // SAFETY: forwarding the arguments we were given, unchanged.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    const CLASS: &str = "BuildRailAppBarTest";

    fn register_class() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let name = wide(CLASS);
            // Leaked on purpose: the class keeps the pointer for the whole
            // process.
            let name: &'static [u16] = Box::leak(name.into_boxed_slice());
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                // SAFETY: a null name asks for this executable's module.
                hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
                lpszClassName: name.as_ptr(),
                ..WNDCLASSW::default()
            };
            // SAFETY: `class` is fully initialised and its strings live
            // for the rest of the process.
            let atom = unsafe { RegisterClassW(&class) };
            assert_ne!(atom, 0, "RegisterClassW failed");
        });
    }

    /// A strip-like window (popup, topmost, never activated) owned by its
    /// own thread, which runs a message loop until the window is closed.
    pub struct TestWindow {
        hwnd: isize,
        thread: Option<JoinHandle<()>>,
    }

    impl TestWindow {
        pub fn open(at: PhysicalRect) -> Self {
            register_class();
            let (tx, rx) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                let class = wide(CLASS);
                let title = wide("BuildRail AppBar test");
                // SAFETY: the class is registered; both strings are
                // NUL-terminated and outlive the call; no parent, menu or
                // creation data.
                let hwnd = unsafe {
                    CreateWindowExW(
                        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                        class.as_ptr(),
                        title.as_ptr(),
                        WS_POPUP | WS_VISIBLE,
                        at.x,
                        at.y,
                        at.width as i32,
                        at.height as i32,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        GetModuleHandleW(std::ptr::null()),
                        std::ptr::null(),
                    )
                };
                tx.send(hwnd as isize).unwrap();
                if hwnd.is_null() {
                    return;
                }
                let mut message = MSG::default();
                // SAFETY: `message` is a valid, writable MSG; the loop runs
                // on the thread that owns the window.
                while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
                    // SAFETY: `message` was just filled in by GetMessageW.
                    unsafe {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            });
            let hwnd = rx.recv().unwrap();
            assert_ne!(hwnd, 0, "CreateWindowExW failed");
            Self {
                hwnd,
                thread: Some(thread),
            }
        }

        pub fn hwnd(&self) -> isize {
            self.hwnd
        }

        pub fn handle(&self) -> RawWindowHandle {
            RawWindowHandle::Win32(Win32WindowHandle::new(
                NonZeroIsize::new(self.hwnd).unwrap(),
            ))
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: plain value arguments; the window's thread destroys
            // it (WM_CLOSE → DestroyWindow → WM_DESTROY → quit).
            unsafe { PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0) };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

use desktop::{move_window, window_rect, work_area, TestWindow};

/// The desktop, one test at a time, or `None` (with a message saying why)
/// when there is no shell to test against.
struct Desktop {
    _turn: MutexGuard<'static, ()>,
    name: &'static str,
}

impl Desktop {
    fn take(name: &'static str) -> Option<Self> {
        static TURN: Mutex<()> = Mutex::new(());
        let turn = TURN.lock().unwrap_or_else(PoisonError::into_inner);
        if !desktop::shell_running() {
            println!(
                "SKIPPED {name}: no Windows shell (Shell_TrayWnd not found), so there is no taskbar or work area to test against"
            );
            return None;
        }
        desktop::per_monitor_dpi_aware();
        println!("RAN {name}: Explorer is running; testing the real AppBar");
        Some(Self { _turn: turn, name })
    }

    fn primary(&self) -> MonitorInfo {
        let monitors = Win32Monitors.monitors().unwrap();
        println!("{}: monitors {monitors:?}", self.name);
        monitors
            .into_iter()
            .find(|m| m.is_primary)
            .expect("a primary monitor")
    }
}

/// Polls `check` for up to three seconds (Explorer and the window's thread
/// work asynchronously).
fn eventually<T: PartialEq + std::fmt::Debug>(what: &str, wanted: T, read: impl Fn() -> T) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let value = read();
        if value == wanted {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: wanted {wanted:?}, still {value:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn reserved(outcome: DockOutcome) -> PhysicalRect {
    match outcome {
        DockOutcome::Reserved { rect, guarantee } => {
            assert_eq!(guarantee, ReservationGuarantee::Enforced);
            rect
        }
        other => panic!("not reserved: {other:?}"),
    }
}

#[test]
fn bottom_and_top_edges_shrink_the_work_area_and_hold_the_window() {
    let Some(desk) = Desktop::take("bottom_and_top_edges") else {
        return;
    };
    let monitor = desk.primary();
    let height = monitor.to_physical(24);
    let before = work_area();
    println!("work area before: {before}");
    let window = TestWindow::open(PhysicalRect::new(200, 200, 400, 24));
    let mut bar = windows_appbar();

    // Bottom: directly above the taskbar, one strip less of work area.
    let rect = reserved(
        bar.reserve(window.handle(), &monitor, Edge::Bottom, 24)
            .unwrap(),
    );
    println!("reserved bottom: {rect}");
    assert_eq!(rect.height, height);
    assert_eq!(rect.bottom(), before.bottom(), "sits on the taskbar");
    let shrunk = PhysicalRect::from_edges(before.x, before.y, before.right(), rect.y);
    eventually("work area after reserving", shrunk, work_area);
    eventually("window on the reserved rect", Some(rect), || {
        window_rect(window.hwnd())
    });

    // Repeated updates: same rect, no creep, window untouched.
    for _ in 0..5 {
        assert_eq!(
            reserved(bar.update(&monitor, Edge::Bottom, 24).unwrap()),
            rect
        );
    }
    assert_eq!(work_area(), shrunk, "no creep");
    assert_eq!(window_rect(window.hwnd()), Some(rect));

    // Something else moves the window: the next update puts it back.
    move_window(window.hwnd(), &PhysicalRect::new(300, 300, 500, 48));
    assert_eq!(
        window_rect(window.hwnd()),
        Some(PhysicalRect::new(300, 300, 500, 48))
    );
    assert_eq!(
        reserved(bar.update(&monitor, Edge::Bottom, 24).unwrap()),
        rect
    );
    eventually("window put back", Some(rect), || window_rect(window.hwnd()));
    assert_eq!(work_area(), shrunk);

    // The top edge: the same reservation moves there.
    let top = reserved(bar.update(&monitor, Edge::Top, 24).unwrap());
    println!("reserved top: {top}");
    assert_eq!(top.y, before.y, "at the top of the work area");
    assert_eq!(top.height, height);
    let below_top =
        PhysicalRect::from_edges(before.x, top.bottom(), before.right(), before.bottom());
    eventually("work area with a top strip", below_top, work_area);
    eventually("window at the top", Some(top), || {
        window_rect(window.hwnd())
    });

    // Released: the work area is whole again.
    bar.release().unwrap();
    eventually("work area after release", before, work_area);
    println!("work area after release: {}", work_area());
}

#[test]
fn dropping_the_dock_without_release_gives_the_edge_back() {
    let Some(desk) = Desktop::take("drop_without_release") else {
        return;
    };
    let monitor = desk.primary();
    let before = work_area();
    let window = TestWindow::open(PhysicalRect::new(200, 200, 400, 24));
    let mut bar = windows_appbar();
    let rect = reserved(
        bar.reserve(window.handle(), &monitor, Edge::Bottom, 24)
            .unwrap(),
    );
    eventually(
        "work area after reserving",
        PhysicalRect::from_edges(before.x, before.y, before.right(), rect.y),
        work_area,
    );
    drop(bar);
    eventually("work area after drop", before, work_area);
    drop(window);
}

/// A second appbar on the same edge (another BuildRail-like bar, or a
/// toolbar) stacks above ours instead of overlapping it: the whole-monitor
/// proposal lets Explorer trim against every bar.
#[test]
fn a_second_appbar_on_the_same_edge_stacks_above() {
    let Some(desk) = Desktop::take("second_appbar_stacks") else {
        return;
    };
    let monitor = desk.primary();
    let before = work_area();
    let (first_window, second_window) = (
        TestWindow::open(PhysicalRect::new(200, 200, 400, 24)),
        TestWindow::open(PhysicalRect::new(200, 300, 400, 24)),
    );
    let (mut first, mut second) = (windows_appbar(), windows_appbar());
    let one = reserved(
        first
            .reserve(first_window.handle(), &monitor, Edge::Bottom, 24)
            .unwrap(),
    );
    let two = reserved(
        second
            .reserve(second_window.handle(), &monitor, Edge::Bottom, 24)
            .unwrap(),
    );
    println!("first {one}, second {two}");
    assert_eq!(two.bottom(), one.y, "stacked directly above");
    eventually("both windows placed", (Some(one), Some(two)), || {
        (
            window_rect(first_window.hwnd()),
            window_rect(second_window.hwnd()),
        )
    });
    // Whether the first keeps its place when it asks again depends on how
    // Explorer orders appbars on one edge; logged for diagnosis, not
    // asserted (BuildRail's own case is a side-edge Task Rail).
    let again = reserved(first.update(&monitor, Edge::Bottom, 24).unwrap());
    println!(
        "first after re-query: {again} ({})",
        if again == one {
            "kept its place"
        } else {
            "moved above the second"
        }
    );
    second.release().unwrap();
    first.release().unwrap();
    eventually("work area after both released", before, work_area);
}
