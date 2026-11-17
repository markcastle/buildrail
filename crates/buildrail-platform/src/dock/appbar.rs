//! The Windows AppBar integration, minus the FFI.
//!
//! [`AppBar`] is the whole AppBar protocol and its window bookkeeping,
//! written against two small ports: [`AppBarShell`] (the `ABM_*` messages
//! sent with `SHAppBarMessage`) and [`AppBarWindow`] (reading and moving
//! the strip's native window). `crate::win32::appbar` implements the ports
//! with a few lines of FFI each and names the result
//! `WindowsAppBar`; the tests below drive the same code with a model of
//! Explorer and of winit, so every decision is unit tested on every OS.
//!
//! The AppBar protocol, in short:
//!
//! 1. `ABM_NEW` registers our window and the private message the shell uses
//!    to notify us; `ABM_ACTIVATE` tells the shell it is ours and active.
//! 2. `ABM_QUERYPOS` with the full rect of the monitor
//!    ([`query_proposal`]): the shell trims it so it does not overlap the
//!    taskbar or other appbars (a second appbar docked to a side edge trims
//!    our left or right end; one on our edge pushes us inwards).
//! 3. We fix the free dimension from the edge the shell left us
//!    ([`after_query`]: the bottom edge stays and the top is our height
//!    above it, or the other way round at the top) and send `ABM_SETPOS`;
//!    the shell reserves that area, so maximised windows stay clear of it.
//! 4. We ask for our window to be moved there. The move is posted to the
//!    window's own thread, so it lands *after* the call returns: a check
//!    never reads the window back straight after asking, and never asks
//!    twice. The window's real rect is part of the invariant: every check
//!    reads it ([`WindowFollow`]), confirms (once, at info level) when it is
//!    on the reserved rect, and otherwise asks again. Anything that moves or
//!    resizes the window afterwards (winit keeping its logical size when it
//!    lands on a monitor with another scale, a toolkit's own start-up
//!    sizing) is therefore undone by the next check, whether or not the
//!    reservation itself changed. Requests are bounded across checks
//!    ([`MAX_MOVE_REQUESTS`], then one every [`RETRY_EVERY`] checks), so a
//!    window that never arrives is reported and retried slowly, never
//!    spun on. Once the window is confirmed after a move we send
//!    `ABM_WINDOWPOSCHANGED`.
//! 5. The shell sends our callback message with `wParam` set to one of the
//!    `ABN_*` codes ([`decode_notification`]) when the taskbar moves, a
//!    fullscreen app opens or closes, and so on.
//! 6. `ABM_REMOVE`, with the same window that registered, gives the space
//!    back.

use std::fmt;

use raw_window_handle::RawWindowHandle;

use super::{
    win32_hwnd, DockError, DockEvent, DockOutcome, EdgeDock, ReservationGuarantee,
    UnsupportedReason,
};
use crate::geometry::{Edge, MonitorInfo, PhysicalRect};

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

/// How many checks in a row may ask for the window to be moved before the
/// dock reports that it is not arriving. Two are needed when the window
/// lands on a monitor with another scale (the first move changes its DPI
/// and the toolkit resizes it; the second puts it back); the third is
/// slack.
pub const MAX_MOVE_REQUESTS: u32 = 3;

/// After [`MAX_MOVE_REQUESTS`] unanswered requests, ask (and report)
/// again only every this many checks (about half a minute at the
/// controller's interval).
pub const RETRY_EVERY: u32 = 10;

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

/// A rectangle as the four edges of a Win32 `RECT` (`right`/`bottom`
/// exclusive). Kept as edges because the shell can hand back an inverted
/// rect, whose edges still mean something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Edges {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge (exclusive).
    pub right: i32,
    /// Bottom edge (exclusive).
    pub bottom: i32,
}

impl Edges {
    /// The edges of `rect`.
    pub fn of(rect: &PhysicalRect) -> Self {
        Self {
            left: rect.x,
            top: rect.y,
            right: rect.right(),
            bottom: rect.bottom(),
        }
    }

    /// The rect between these edges (empty when inverted).
    pub fn rect(&self) -> PhysicalRect {
        PhysicalRect::from_edges(self.left, self.top, self.right, self.bottom)
    }
}

impl fmt::Display for Edges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}, {}, {}, {}]",
            self.left, self.top, self.right, self.bottom
        )
    }
}

/// The rect to propose with `ABM_QUERYPOS`: the whole monitor (its
/// *bounds*), so the shell sees every bar on that monitor intersect it and
/// trims it against all of them (a proposal only one strip tall misses a
/// second bar stacked above the taskbar). The shell also uses this rect to
/// decide which monitor we mean.
///
/// Monitor bounds are used deliberately, not the work area: the work area
/// already excludes our own reservation, so proposing from it would move
/// the strip inwards by its own height every time we re-query.
pub fn query_proposal(monitor: &MonitorInfo) -> Edges {
    Edges::of(&monitor.bounds)
}

/// After `ABM_QUERYPOS` the shell has moved our `edge` inwards past the
/// taskbar and other appbars, and trimmed the ends against side bars.
/// Keep that edge and those ends, and put the other edge `height` physical
/// pixels away, as the AppBar documentation prescribes.
pub fn after_query(queried: &Edges, edge: Edge, height: u32) -> PhysicalRect {
    let height = i32::try_from(height).unwrap_or(i32::MAX);
    match edge {
        Edge::Bottom => PhysicalRect::from_edges(
            queried.left,
            queried.bottom.saturating_sub(height),
            queried.right,
            queried.bottom,
        ),
        Edge::Top => PhysicalRect::from_edges(
            queried.left,
            queried.top,
            queried.right,
            queried.top.saturating_add(height),
        ),
    }
}

/// The strip's height in physical pixels on `monitor` (never taller than
/// the monitor).
pub fn strip_height(monitor: &MonitorInfo, strip_height_logical: u32) -> u32 {
    monitor
        .to_physical(strip_height_logical)
        .min(monitor.bounds.height)
}

/// The `ABM_*` messages, sent to the shell for one window (an `HWND` as an
/// integer).
pub trait AppBarShell: Send {
    /// `ABM_NEW`: registers `window` as an appbar.
    fn register(&mut self, window: isize) -> Result<(), DockError>;
    /// `ABM_QUERYPOS`: the shell's adjustment of `proposed` on `edge`.
    fn query_pos(&mut self, window: isize, edge: Edge, proposed: Edges) -> Edges;
    /// `ABM_SETPOS`: reserves `rect` on `edge`; returns what the shell
    /// actually reserved.
    fn set_pos(&mut self, window: isize, edge: Edge, rect: Edges) -> Edges;
    /// `ABM_REMOVE`: gives the space back.
    fn remove(&mut self, window: isize);
    /// `ABM_ACTIVATE`: our appbar is active (keeps the shell's z-order of
    /// auto-hide bars right).
    fn activate(&mut self, window: isize);
    /// `ABM_WINDOWPOSCHANGED`: our window moved.
    fn window_pos_changed(&mut self, window: isize);
    /// The id of the private message the shell notifies us with (`0` when
    /// none could be registered).
    fn callback_message(&self) -> u32;
}

/// The strip's native window, as the AppBar sees it.
pub trait AppBarWindow: Send {
    /// Where the window really is now (outer rect, physical pixels), or
    /// `None` when it cannot be read (the window is gone).
    fn rect(&self, window: isize) -> Option<PhysicalRect>;
    /// Asks for the window to be moved and sized to `rect`, topmost,
    /// without activating it. The move happens on the window's own thread,
    /// after this returns: read the rect at the next check, not now.
    fn place(&mut self, window: isize, rect: &PhysicalRect) -> Result<(), DockError>;
    /// Changes only the z-order: topmost, or behind everything (while a
    /// fullscreen app is open).
    fn set_topmost(&mut self, window: isize, topmost: bool) -> Result<(), DockError>;
}

/// What one check decided about the window (see [`WindowFollow`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FollowStep {
    /// Ask for the window to be moved to the reserved rect.
    pub request_move: bool,
    /// The window was just seen on the reserved rect for the first time
    /// since the reservation changed or the window was moved: say so once.
    pub confirmed: bool,
    /// The window still has not arrived after [`MAX_MOVE_REQUESTS`]
    /// requests: report it (rate-limited).
    pub not_arriving: bool,
}

/// Keeps the window on the reserved rect across checks, with moves that
/// land asynchronously. Pure: it is told where the window is and decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowFollow {
    /// The window has been seen on the current reserved rect.
    confirmed: bool,
    /// Moves asked for since the window was last confirmed.
    requests: u32,
    /// Checks since the window was last confirmed.
    misses: u32,
}

impl WindowFollow {
    /// A window not yet seen anywhere.
    pub fn new() -> Self {
        Self::default()
    }

    /// The reservation moved: the window must be confirmed afresh.
    pub fn reservation_changed(&mut self) {
        *self = Self::default();
    }

    /// `true` when the window has been seen on the current rect.
    pub fn is_confirmed(&self) -> bool {
        self.confirmed
    }

    /// Moves asked for since the window was last confirmed.
    pub fn requests(&self) -> u32 {
        self.requests
    }

    /// One check: the window is `on_rect` or not.
    pub fn check(&mut self, on_rect: bool) -> FollowStep {
        if on_rect {
            let confirmed = !self.confirmed;
            *self = Self {
                confirmed: true,
                requests: 0,
                misses: 0,
            };
            return FollowStep {
                confirmed,
                ..FollowStep::default()
            };
        }
        self.confirmed = false;
        self.misses += 1;
        let request_move = if self.requests < MAX_MOVE_REQUESTS {
            true
        } else {
            (self.misses - MAX_MOVE_REQUESTS).is_multiple_of(RETRY_EVERY)
        };
        // Reported when the bound is first passed, then with each slow
        // retry.
        let not_arriving = (self.requests >= MAX_MOVE_REQUESTS && request_move)
            || self.misses == MAX_MOVE_REQUESTS + 1;
        if request_move {
            self.requests += 1;
        }
        FollowStep {
            request_move,
            confirmed: false,
            not_arriving,
        }
    }
}

fn show(rect: Option<PhysicalRect>) -> String {
    rect.map_or_else(|| "unknown".to_owned(), |r| r.to_string())
}

/// What the dock was last asked for.
#[derive(Debug, Clone, PartialEq)]
struct Request {
    monitor: MonitorInfo,
    edge: Edge,
    strip_height_logical: u32,
}

/// The AppBar dock over its two ports. See the module docs.
#[derive(Debug)]
pub struct AppBar<S: AppBarShell, W: AppBarWindow> {
    shell: S,
    window: W,
    hwnd: Option<isize>,
    request: Option<Request>,
    /// The edge and rect the shell reserved for us.
    reserved: Option<(Edge, PhysicalRect)>,
    /// Whether the window is on the reserved rect, across checks.
    follow: WindowFollow,
    /// A move was asked for since the window was last confirmed (so the
    /// shell hears `ABM_WINDOWPOSCHANGED` when it arrives).
    moved: bool,
}

impl<S: AppBarShell, W: AppBarWindow> AppBar<S, W> {
    /// An unregistered appbar talking to `shell` and moving windows with
    /// `window`.
    pub fn with_ports(shell: S, window: W) -> Self {
        Self {
            shell,
            window,
            hwnd: None,
            request: None,
            reserved: None,
            follow: WindowFollow::new(),
            moved: false,
        }
    }

    /// The registered window, if any.
    pub fn window(&self) -> Option<isize> {
        self.hwnd
    }

    /// The edge and rect currently reserved, if any.
    pub fn reserved(&self) -> Option<(Edge, PhysicalRect)> {
        self.reserved
    }

    /// The shell port (tests inspect their model through it).
    pub fn shell(&self) -> &S {
        &self.shell
    }

    /// The window port.
    pub fn window_port(&self) -> &W {
        &self.window
    }

    /// The window port, mutably (tests move the fake window through it).
    pub fn window_port_mut(&mut self) -> &mut W {
        &mut self.window
    }

    fn position(&mut self, hwnd: isize, force: bool) -> Result<DockOutcome, DockError> {
        let request = self.request.clone().ok_or(DockError::NotReserved)?;
        let Request {
            monitor,
            edge,
            strip_height_logical,
        } = &request;
        let edge = *edge;
        let proposed = query_proposal(monitor);
        let queried = self.shell.query_pos(hwnd, edge, proposed);
        let wanted = after_query(&queried, edge, strip_height(monitor, *strip_height_logical));
        let reservation_changed = force || self.reserved != Some((edge, wanted));
        let rect = if reservation_changed {
            let set = self.shell.set_pos(hwnd, edge, Edges::of(&wanted));
            let rect = set.rect();
            self.reserved = Some((edge, rect));
            rect
        } else {
            wanted
        };
        if reservation_changed {
            self.follow.reservation_changed();
        }
        let window_now = self.window.rect(hwnd);
        let step = self.follow.check(window_now == Some(rect));
        if step.request_move {
            self.window.place(hwnd, &rect)?;
            self.moved = true;
        }
        let move_state = if step.request_move {
            "requested"
        } else if window_now == Some(rect) {
            "not needed"
        } else {
            "held back"
        };
        let operation = if force {
            Some("reserved the screen edge")
        } else if reservation_changed {
            Some("moved the reserved screen edge")
        } else if step.request_move && self.follow.requests() == 1 {
            Some("asked for the strip to go back to its reserved edge")
        } else {
            None
        };
        if let Some(operation) = operation {
            tracing::info!(
                edge = %edge,
                monitor = %monitor.id,
                bounds = %monitor.bounds,
                scale = monitor.scale_factor,
                proposed = %proposed,
                queried = %queried,
                reserved = %rect,
                window_before = %show(window_now),
                target = %rect,
                window_move = move_state,
                "{operation}"
            );
        }
        if step.confirmed {
            if std::mem::take(&mut self.moved) {
                self.shell.window_pos_changed(hwnd);
            }
            tracing::info!(
                window = %rect,
                edge = %edge,
                monitor = %monitor.id,
                "the strip is on its reserved edge"
            );
        } else if step.not_arriving {
            tracing::warn!(
                window = %show(window_now),
                reserved = %rect,
                requests = self.follow.requests(),
                window_move = move_state,
                "the strip is not on its reserved edge yet; will try again"
            );
        } else if operation.is_none() {
            tracing::debug!(
                edge = %edge,
                reserved = %rect,
                window = %show(window_now),
                on_rect = self.follow.is_confirmed(),
                window_move = move_state,
                "dock checked"
            );
        }
        Ok(DockOutcome::Reserved {
            rect,
            guarantee: ReservationGuarantee::Enforced,
        })
    }
}

impl<S: AppBarShell, W: AppBarWindow> EdgeDock for AppBar<S, W> {
    fn reserve(
        &mut self,
        window: RawWindowHandle,
        monitor: &MonitorInfo,
        edge: Edge,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError> {
        let Some(hwnd) = win32_hwnd(&window) else {
            return Ok(DockOutcome::Unsupported {
                reason: UnsupportedReason::UnsupportedWindowHandle,
            });
        };
        if self.hwnd.is_some_and(|h| h != hwnd) {
            self.release()?;
        }
        if self.hwnd.is_none() {
            self.shell.register(hwnd)?;
            self.hwnd = Some(hwnd);
            self.shell.activate(hwnd);
            tracing::info!(window = hwnd, "registered the strip as an appbar");
        }
        self.request = Some(Request {
            monitor: monitor.clone(),
            edge,
            strip_height_logical,
        });
        self.position(hwnd, true)
    }

    fn update(
        &mut self,
        monitor: &MonitorInfo,
        edge: Edge,
        strip_height_logical: u32,
    ) -> Result<DockOutcome, DockError> {
        let hwnd = self.hwnd.ok_or(DockError::NotReserved)?;
        self.request = Some(Request {
            monitor: monitor.clone(),
            edge,
            strip_height_logical,
        });
        self.position(hwnd, false)
    }

    fn release(&mut self) -> Result<(), DockError> {
        if let Some(hwnd) = self.hwnd.take() {
            self.shell.remove(hwnd);
            let (edge, rect) = self
                .reserved
                .map_or(("none".to_owned(), "none".to_owned()), |(e, r)| {
                    (e.to_string(), r.to_string())
                });
            tracing::info!(window = hwnd, edge, rect, "released the screen edge");
        }
        self.reserved = None;
        self.request = None;
        self.follow = WindowFollow::new();
        self.moved = false;
        Ok(())
    }

    fn handle_native_message(
        &mut self,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> Option<DockEvent> {
        let hwnd = self.hwnd?;
        let callback = self.shell.callback_message();
        if message != callback || callback == 0 {
            return None;
        }
        match decode_notification(wparam, lparam)? {
            AppBarNotification::PositionChanged | AppBarNotification::StateChanged => {
                match self.position(hwnd, false) {
                    Ok(DockOutcome::Reserved { rect, .. }) => {
                        Some(DockEvent::Repositioned { rect })
                    }
                    Ok(DockOutcome::Unsupported { .. }) => None,
                    Err(err) => {
                        tracing::warn!("AppBar reposition failed: {err}");
                        None
                    }
                }
            }
            AppBarNotification::FullscreenApp(entered) => {
                // As the AppBar docs prescribe: drop below the fullscreen
                // app, and come back on top when it closes.
                if let Err(err) = self.window.set_topmost(hwnd, !entered) {
                    tracing::debug!("AppBar z-order change failed: {err}");
                }
                Some(if entered {
                    DockEvent::FullscreenEntered
                } else {
                    DockEvent::FullscreenLeft
                })
            }
            AppBarNotification::WindowArrange(_) => None,
        }
    }
}

impl<S: AppBarShell, W: AppBarWindow> Drop for AppBar<S, W> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::num::NonZeroIsize;

    use raw_window_handle::{Win32WindowHandle, XlibWindowHandle};

    use super::*;
    use crate::geometry::{dock_rect, MonitorId};

    fn monitor(id: &str, bounds: PhysicalRect, scale: f64) -> MonitorInfo {
        MonitorInfo::without_work_area(MonitorId::new(id), bounds, scale, id == "A")
    }

    /// Primary 1920x1080 at 100 % with a 40 px bottom taskbar.
    fn a() -> MonitorInfo {
        monitor("A", PhysicalRect::new(0, 0, 1920, 1080), 1.0)
    }

    /// 3840x2160 at 200 % to the right of A, with its own 80 px taskbar.
    fn b() -> MonitorInfo {
        monitor("B", PhysicalRect::new(1920, 0, 3840, 2160), 2.0)
    }

    /// 2560x1440 at 150 % to the left of and above A (negative origin).
    fn c() -> MonitorInfo {
        monitor("C", PhysicalRect::new(-2560, -360, 2560, 1440), 1.5)
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
    fn proposal_is_the_whole_monitor_whatever_the_work_area() {
        let mut m = c();
        m.work_area = PhysicalRect::new(-2560, -360, 2560, 1380);
        assert_eq!(
            query_proposal(&m),
            Edges {
                left: -2560,
                top: -360,
                right: 0,
                bottom: 1080
            }
        );
        assert_eq!(query_proposal(&m).rect(), m.bounds);
        assert_eq!(query_proposal(&m).to_string(), "[-2560, -360, 0, 1080]");
    }

    #[test]
    fn after_query_keeps_the_edge_the_shell_left_and_restores_the_height() {
        // Bottom: the shell moved the bottom up to the taskbar (1040).
        let queried = Edges {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert_eq!(
            after_query(&queried, Edge::Bottom, 30),
            PhysicalRect::new(0, 1010, 1920, 30)
        );
        // Top: the shell moved the top down below a top taskbar (48).
        let queried = Edges {
            left: 62,
            top: 48,
            right: 1920,
            bottom: 1080,
        };
        assert_eq!(
            after_query(&queried, Edge::Top, 24),
            PhysicalRect::new(62, 48, 1858, 24)
        );
        // An inverted answer still keeps its meaningful edge.
        let squashed = Edges {
            left: 0,
            top: 1056,
            right: 1920,
            bottom: 1040,
        };
        assert_eq!(
            after_query(&squashed, Edge::Bottom, 24),
            PhysicalRect::new(0, 1016, 1920, 24)
        );
    }

    #[test]
    fn strip_height_is_scaled_and_capped() {
        assert_eq!(strip_height(&c(), 24), 36);
        assert_eq!(strip_height(&b(), 24), 48);
        let tiny = monitor("t", PhysicalRect::new(0, 0, 100, 10), 1.0);
        assert_eq!(strip_height(&tiny, 24), 10);
    }

    // ---- A model of Explorer -------------------------------------------

    /// The edge a bar is docked to (appbars of other programs may use the
    /// sides).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Side {
        Top,
        Bottom,
        Left,
        Right,
    }

    impl From<Edge> for Side {
        fn from(edge: Edge) -> Self {
            match edge {
                Edge::Top => Self::Top,
                Edge::Bottom => Self::Bottom,
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Bar {
        window: isize,
        side: Side,
        rect: PhysicalRect,
    }

    /// Takes `bar` out of `r` along the bar's side.
    fn subtract(r: &mut Edges, bar: &Bar) {
        match bar.side {
            Side::Bottom => r.bottom = r.bottom.min(bar.rect.y),
            Side::Top => r.top = r.top.max(bar.rect.bottom()),
            Side::Left => r.left = r.left.max(bar.rect.right()),
            Side::Right => r.right = r.right.min(bar.rect.x),
        }
    }

    /// Enough of Explorer's appbar bookkeeping to reproduce what it does to
    /// our rect: taskbars that never move, other appbars (Task Rail) in the
    /// order they registered, and each monitor's work area.
    #[derive(Debug, Default)]
    struct Explorer {
        monitors: Vec<MonitorInfo>,
        /// Taskbars (window 0), other programs' bars and ours.
        bars: Vec<Bar>,
        registered: Vec<isize>,
        set_pos_calls: u32,
        activated: u32,
        pos_changed: u32,
        removed: Vec<isize>,
    }

    impl Explorer {
        fn with_taskbars(monitors: &[MonitorInfo]) -> Self {
            let bars = monitors
                .iter()
                .map(|m| {
                    let h = m.to_physical(40);
                    Bar {
                        window: 0,
                        side: Side::Bottom,
                        rect: PhysicalRect::new(
                            m.bounds.x,
                            m.bounds.bottom() - h as i32,
                            m.bounds.width,
                            h,
                        ),
                    }
                })
                .collect();
            Self {
                monitors: monitors.to_vec(),
                bars,
                ..Self::default()
            }
        }

        fn monitor_of(&self, rect: &PhysicalRect) -> Option<&MonitorInfo> {
            self.monitors
                .iter()
                .max_by_key(|m| m.bounds.overlap_area(rect))
                .filter(|m| m.bounds.overlap_area(rect) > 0)
        }

        /// What Explorer does in ABM_QUERYPOS: clip to the monitor, then
        /// subtract every other bar that intersects the rect, by that bar's
        /// side.
        fn adjust(&self, window: isize, proposed: Edges) -> Edges {
            let mut r = proposed;
            let Some(m) = self.monitor_of(&proposed.rect()) else {
                return r;
            };
            r.left = r.left.max(m.bounds.x);
            r.right = r.right.min(m.bounds.right());
            r.top = r.top.max(m.bounds.y);
            r.bottom = r.bottom.min(m.bounds.bottom());
            for bar in self.bars.iter().filter(|b| b.window != window) {
                if bar.rect.intersection(&r.rect()).is_some() {
                    subtract(&mut r, bar);
                }
            }
            r
        }

        fn work_area(&self, m: &MonitorInfo) -> PhysicalRect {
            let mut r = Edges::of(&m.bounds);
            for bar in &self.bars {
                if bar.rect.intersection(&m.bounds).is_some() {
                    subtract(&mut r, bar);
                }
            }
            r.rect()
        }

        fn ours(&self) -> Vec<Bar> {
            self.bars
                .iter()
                .filter(|b| self.registered.contains(&b.window))
                .copied()
                .collect()
        }
    }

    impl AppBarShell for Explorer {
        fn register(&mut self, window: isize) -> Result<(), DockError> {
            if self.registered.contains(&window) {
                return Err(DockError::Win32 {
                    operation: "SHAppBarMessage(ABM_NEW)",
                    code: 0,
                });
            }
            self.registered.push(window);
            Ok(())
        }
        fn query_pos(&mut self, window: isize, _: Edge, proposed: Edges) -> Edges {
            self.adjust(window, proposed)
        }
        fn set_pos(&mut self, window: isize, edge: Edge, rect: Edges) -> Edges {
            self.set_pos_calls += 1;
            let rect = rect.rect();
            self.bars.retain(|b| b.window != window);
            self.bars.push(Bar {
                window,
                side: edge.into(),
                rect,
            });
            Edges::of(&rect)
        }
        fn remove(&mut self, window: isize) {
            self.removed.push(window);
            self.registered.retain(|w| *w != window);
            self.bars.retain(|b| b.window != window);
        }
        fn activate(&mut self, _: isize) {
            self.activated += 1;
        }
        fn window_pos_changed(&mut self, _: isize) {
            self.pos_changed += 1;
        }
        fn callback_message(&self) -> u32 {
            0xC123
        }
    }

    // ---- A model of a winit window -------------------------------------

    /// A per-monitor-DPI-aware window as winit runs it on Windows: `place`
    /// is posted to the window's thread (`SetWindowPos` with
    /// `SWP_ASYNCWINDOWPOS`) and only happens when that thread runs, which
    /// is after the dock's call has returned ([`pump`], unless `busy`).
    /// When a move lands on a monitor with another scale, winit answers
    /// `WM_DPICHANGED` by resizing the window to keep its *logical* size.
    #[derive(Debug)]
    struct Winit {
        monitors: Vec<MonitorInfo>,
        rect: PhysicalRect,
        pending: VecDeque<PhysicalRect>,
        busy: bool,
        topmost: bool,
        places: u32,
    }

    impl Winit {
        fn new(monitors: &[MonitorInfo], rect: PhysicalRect) -> Self {
            Self {
                monitors: monitors.to_vec(),
                rect,
                pending: VecDeque::new(),
                busy: false,
                topmost: true,
                places: 0,
            }
        }

        fn scale_at(&self, rect: &PhysicalRect) -> f64 {
            self.monitors
                .iter()
                .max_by_key(|m| m.bounds.overlap_area(rect))
                .map_or(1.0, |m| m.scale_factor)
        }

        /// The window's thread handles the moves posted to it.
        fn run_thread(&mut self) {
            if self.busy {
                return;
            }
            while let Some(target) = self.pending.pop_front() {
                let old = self.scale_at(&self.rect);
                let new = self.scale_at(&target);
                self.rect = target;
                if (old - new).abs() > f64::EPSILON {
                    let keep = |len: u32| (f64::from(len) / old * new).round() as u32;
                    self.rect.width = keep(target.width);
                    self.rect.height = keep(target.height);
                }
            }
        }
    }

    impl AppBarWindow for Winit {
        fn rect(&self, _: isize) -> Option<PhysicalRect> {
            Some(self.rect)
        }
        fn place(&mut self, _: isize, rect: &PhysicalRect) -> Result<(), DockError> {
            self.places += 1;
            self.pending.push_back(*rect);
            Ok(())
        }
        fn set_topmost(&mut self, _: isize, topmost: bool) -> Result<(), DockError> {
            self.topmost = topmost;
            Ok(())
        }
    }

    const HWND: isize = 0x1234;

    type Dock = AppBar<Explorer, Winit>;

    fn handle(hwnd: isize) -> RawWindowHandle {
        RawWindowHandle::Win32(Win32WindowHandle::new(NonZeroIsize::new(hwnd).unwrap()))
    }

    fn appbar(monitors: &[MonitorInfo]) -> Dock {
        // winit created the window on the primary, at its own default size.
        AppBar::with_ports(
            Explorer::with_taskbars(monitors),
            Winit::new(monitors, PhysicalRect::new(200, 200, 760, 24)),
        )
    }

    /// Time passes between the dock's calls: the window's thread runs.
    fn pump(bar: &mut Dock) {
        bar.window_port_mut().run_thread();
    }

    fn reserved_rect(outcome: Result<DockOutcome, DockError>) -> PhysicalRect {
        match outcome {
            Ok(DockOutcome::Reserved { rect, guarantee }) => {
                assert_eq!(guarantee, ReservationGuarantee::Enforced);
                rect
            }
            other => panic!("{other:?}"),
        }
    }

    /// A periodic (or UI-triggered) check, then the window's thread runs.
    fn check(bar: &mut Dock, monitor: &MonitorInfo, edge: Edge) -> PhysicalRect {
        let rect = reserved_rect(bar.update(monitor, edge, 24));
        pump(bar);
        rect
    }

    /// Reserves, lets the window's thread run, and checks until the window
    /// is confirmed on the rect, which must take at most `checks` checks.
    fn reserve_and_settle(
        bar: &mut Dock,
        monitor: &MonitorInfo,
        edge: Edge,
        checks: u32,
    ) -> PhysicalRect {
        let rect = reserved_rect(bar.reserve(handle(HWND), monitor, edge, 24));
        pump(bar);
        for _ in 0..checks {
            check(bar, monitor, edge);
            if bar.follow.is_confirmed() {
                return rect;
            }
        }
        panic!(
            "not confirmed within {checks} checks: window {} reserved {rect}",
            bar.window_port().rect
        );
    }

    #[test]
    fn reserves_above_the_taskbar_and_confirms_the_window_at_the_next_check() {
        let mut bar = appbar(&[a()]);
        let rect = reserved_rect(bar.reserve(handle(HWND), &a(), Edge::Bottom, 24));
        assert_eq!(rect, PhysicalRect::new(0, 1016, 1920, 24));
        // The move was only requested: it lands after the call returns.
        assert_eq!(bar.window_port().rect, PhysicalRect::new(200, 200, 760, 24));
        assert_eq!(bar.window_port().places, 1);
        assert!(!bar.follow.is_confirmed());
        assert_eq!(bar.shell().pos_changed, 0, "not moved yet");
        pump(&mut bar);
        assert_eq!(bar.window_port().rect, rect);
        check(&mut bar, &a(), Edge::Bottom);
        assert!(bar.follow.is_confirmed());
        assert_eq!(bar.window_port().places, 1, "asked once, never repeated");
        assert_eq!(bar.shell().pos_changed, 1, "told once it arrived");
        assert_eq!(
            bar.shell().work_area(&a()),
            PhysicalRect::new(0, 0, 1920, 1016)
        );
        assert_eq!(bar.shell().activated, 1);
        assert_eq!(bar.window(), Some(HWND));
        assert_eq!(bar.reserved(), Some((Edge::Bottom, rect)));
        // The same answer as the platform-neutral maths gives for the work
        // area the shell reports without our strip.
        let mut m = a();
        m.work_area = PhysicalRect::new(0, 0, 1920, 1040);
        assert_eq!(dock_rect(&m, Edge::Bottom, 24, None), rect);
    }

    /// The owner's first report: moved to another screen with another
    /// scale, the strip sat away from the taskbar. The window followed our
    /// SetWindowPos, then winit resized it for the new DPI, and nothing ever
    /// put it back because the reservation itself had not changed. Now the
    /// next check sees the resized window and asks again; the one after
    /// confirms it, and nothing moves after that.
    #[test]
    fn moving_to_a_monitor_with_another_scale_converges_within_two_checks_and_stays() {
        let monitors = [a(), b()];
        let mut bar = appbar(&monitors);
        reserve_and_settle(&mut bar, &a(), Edge::Bottom, 1);
        let rect = reserved_rect(bar.reserve(handle(HWND), &b(), Edge::Bottom, 24));
        // B: 2160 tall, 80 px taskbar, 48 px strip.
        assert_eq!(rect, PhysicalRect::new(1920, 2032, 3840, 48));
        pump(&mut bar);
        assert_eq!(
            bar.window_port().rect,
            PhysicalRect::new(1920, 2032, 7680, 96),
            "winit kept the logical size: twice as big"
        );
        check(&mut bar, &b(), Edge::Bottom);
        assert_eq!(bar.window_port().rect, rect, "corrected by the first check");
        check(&mut bar, &b(), Edge::Bottom);
        assert!(bar.follow.is_confirmed(), "confirmed by the second");
        let places = bar.window_port().places;
        for _ in 0..5 {
            check(&mut bar, &b(), Edge::Bottom);
        }
        assert_eq!(bar.window_port().rect, rect);
        assert_eq!(bar.window_port().places, places, "and stays");
        // One appbar, moved; A's work area is whole again.
        assert_eq!(bar.shell().registered, vec![HWND]);
        assert_eq!(bar.shell().ours().len(), 1);
        assert_eq!(
            bar.shell().work_area(&a()),
            PhysicalRect::new(0, 0, 1920, 1040)
        );
        assert_eq!(
            bar.shell().work_area(&b()),
            PhysicalRect::new(1920, 0, 3840, 2032)
        );
        // And back to A, from 200 % to 100 %.
        let rect = reserve_and_settle(&mut bar, &a(), Edge::Bottom, 2);
        assert_eq!(bar.window_port().rect, rect);
        assert_eq!(rect, PhysicalRect::new(0, 1016, 1920, 24));
    }

    #[test]
    fn negative_origin_monitor_at_150_percent() {
        let monitors = [a(), c()];
        let mut bar = appbar(&monitors);
        let rect = reserve_and_settle(&mut bar, &c(), Edge::Bottom, 2);
        // C's taskbar is 60 px (40 at 150 %); the strip 36.
        assert_eq!(rect, PhysicalRect::new(-2560, 1080 - 60 - 36, 2560, 36));
        assert_eq!(bar.window_port().rect, rect);
    }

    /// Whatever moves the window after we placed it (winit's start-up
    /// sizing, a DPI change, a viewport command), the next check asks for
    /// it to go back even though the reservation is unchanged.
    #[test]
    fn a_window_that_drifted_is_put_back_by_the_next_checks() {
        let mut bar = appbar(&[a()]);
        let rect = reserve_and_settle(&mut bar, &a(), Edge::Bottom, 1);
        let set_pos = bar.shell().set_pos_calls;
        bar.window_port_mut().rect = PhysicalRect::new(400, 300, 760, 24);
        assert_eq!(
            reserved_rect(bar.update(&a(), Edge::Bottom, 24)),
            rect,
            "same reservation"
        );
        assert!(!bar.follow.is_confirmed());
        assert_eq!(
            bar.window_port().rect,
            PhysicalRect::new(400, 300, 760, 24),
            "not read back in the same call"
        );
        pump(&mut bar);
        assert_eq!(bar.window_port().rect, rect, "window back on it");
        check(&mut bar, &a(), Edge::Bottom);
        assert!(bar.follow.is_confirmed());
        assert_eq!(bar.shell().set_pos_calls, set_pos, "no new ABM_SETPOS");
        // Grown by a stray resize (one strip taller): shrunk back.
        bar.window_port_mut().rect = PhysicalRect::new(0, 1016, 1920, 48);
        check(&mut bar, &a(), Edge::Bottom);
        assert_eq!(bar.window_port().rect, rect);
    }

    #[test]
    fn repeated_checks_neither_creep_nor_move_anything() {
        let mut bar = appbar(&[a()]);
        let rect = reserve_and_settle(&mut bar, &a(), Edge::Bottom, 1);
        let places = bar.window_port().places;
        for _ in 0..10 {
            assert_eq!(check(&mut bar, &a(), Edge::Bottom), rect);
        }
        assert_eq!(bar.shell().set_pos_calls, 1);
        assert_eq!(bar.window_port().places, places, "no oscillation");
    }

    /// A window that never arrives (its thread busy, or something fighting
    /// it) is asked a bounded number of times, then only every
    /// RETRY_EVERY checks; it is never spun on. When the thread runs again,
    /// the window lands and is confirmed.
    #[test]
    fn a_window_that_does_not_arrive_is_retried_slowly_and_converges_later() {
        let monitors = [a(), b()];
        let mut bar = appbar(&monitors);
        bar.window_port_mut().busy = true;
        let rect = reserved_rect(bar.reserve(handle(HWND), &b(), Edge::Bottom, 24));
        for _ in 0..(MAX_MOVE_REQUESTS + RETRY_EVERY - 1) {
            check(&mut bar, &b(), Edge::Bottom);
        }
        assert_eq!(bar.window_port().places, MAX_MOVE_REQUESTS + 1);
        assert_eq!(
            bar.window_port().pending.len() as u32,
            MAX_MOVE_REQUESTS + 1
        );
        bar.window_port_mut().busy = false;
        pump(&mut bar);
        for _ in 0..2 {
            check(&mut bar, &b(), Edge::Bottom);
        }
        assert!(bar.follow.is_confirmed());
        assert_eq!(bar.window_port().rect, rect);
    }

    #[test]
    fn follow_bounds_requests_and_rate_limits_reports() {
        let mut follow = WindowFollow::new();
        let mut steps = Vec::new();
        for _ in 0..(MAX_MOVE_REQUESTS + 2 * RETRY_EVERY) {
            steps.push(follow.check(false));
        }
        let requested: Vec<usize> = steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.request_move)
            .map(|(i, _)| i + 1)
            .collect();
        assert_eq!(requested, vec![1, 2, 3, 13, 23]);
        let reported: Vec<usize> = steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.not_arriving)
            .map(|(i, _)| i + 1)
            .collect();
        assert_eq!(reported, vec![4, 13, 23]);
        assert!(steps.iter().all(|s| !s.confirmed));
        assert_eq!(
            follow.check(true),
            FollowStep {
                confirmed: true,
                ..FollowStep::default()
            }
        );
        assert_eq!(follow.check(true), FollowStep::default(), "confirmed once");
        assert_eq!(follow.requests(), 0);
        follow.reservation_changed();
        assert!(
            follow.check(true).confirmed,
            "confirmed afresh after a change"
        );
    }

    #[test]
    fn a_second_appbar_on_a_side_edge_trims_our_end() {
        let mut explorer = Explorer::with_taskbars(&[a()]);
        // Task Rail docked on the left, 200 px wide, registered first.
        explorer.bars.push(Bar {
            window: 77,
            side: Side::Left,
            rect: PhysicalRect::new(0, 0, 200, 1040),
        });
        let mut bar = AppBar::with_ports(
            explorer,
            Winit::new(&[a()], PhysicalRect::new(0, 0, 10, 10)),
        );
        let rect = reserve_and_settle(&mut bar, &a(), Edge::Bottom, 1);
        assert_eq!(rect, PhysicalRect::new(200, 1016, 1720, 24));
        assert_eq!(bar.window_port().rect, rect);

        // Docked on the right instead, at the top edge.
        bar.shell.bars.retain(|b| b.window != 77);
        bar.shell.bars.push(Bar {
            window: 77,
            side: Side::Right,
            rect: PhysicalRect::new(1720, 0, 200, 1040),
        });
        let rect = check(&mut bar, &a(), Edge::Top);
        assert_eq!(rect, PhysicalRect::new(0, 0, 1720, 24));
        assert_eq!(bar.window_port().rect, rect);
    }

    #[test]
    fn a_second_bottom_appbar_above_the_taskbar_stacks_us_above_it() {
        let mut explorer = Explorer::with_taskbars(&[a()]);
        explorer.bars.push(Bar {
            window: 77,
            side: Side::Bottom,
            rect: PhysicalRect::new(0, 1000, 1920, 40),
        });
        let mut bar = AppBar::with_ports(
            explorer,
            Winit::new(&[a()], PhysicalRect::new(0, 0, 10, 10)),
        );
        let rect = reserve_and_settle(&mut bar, &a(), Edge::Bottom, 1);
        assert_eq!(rect, PhysicalRect::new(0, 976, 1920, 24));
    }

    #[test]
    fn top_edge_and_switching_edges_moves_the_one_reservation() {
        let mut bar = appbar(&[a()]);
        reserve_and_settle(&mut bar, &a(), Edge::Bottom, 1);
        let rect = check(&mut bar, &a(), Edge::Top);
        assert_eq!(rect, PhysicalRect::new(0, 0, 1920, 24));
        assert!(!bar.follow.is_confirmed(), "a new rect is confirmed afresh");
        assert_eq!(bar.window_port().rect, rect);
        check(&mut bar, &a(), Edge::Top);
        assert!(bar.follow.is_confirmed());
        assert_eq!(bar.shell().ours().len(), 1);
        assert_eq!(
            bar.shell().work_area(&a()),
            PhysicalRect::new(0, 24, 1920, 1016)
        );
        assert_eq!(bar.reserved(), Some((Edge::Top, rect)));
    }

    #[test]
    fn top_edge_below_a_top_taskbar() {
        let mut explorer = Explorer::with_taskbars(&[]);
        explorer.monitors = vec![a()];
        explorer.bars.push(Bar {
            window: 0,
            side: Side::Top,
            rect: PhysicalRect::new(0, 0, 1920, 48),
        });
        let mut bar = AppBar::with_ports(
            explorer,
            Winit::new(&[a()], PhysicalRect::new(0, 0, 10, 10)),
        );
        let rect = reserve_and_settle(&mut bar, &a(), Edge::Top, 1);
        assert_eq!(rect, PhysicalRect::new(0, 48, 1920, 24));
    }

    #[test]
    fn release_and_drop_give_the_space_back_with_the_registering_window() {
        let mut bar = appbar(&[a()]);
        reserve_and_settle(&mut bar, &a(), Edge::Bottom, 1);
        bar.release().unwrap();
        assert_eq!(bar.shell().removed, vec![HWND]);
        assert!(bar.shell().ours().is_empty());
        assert_eq!(
            bar.shell().work_area(&a()),
            PhysicalRect::new(0, 0, 1920, 1040)
        );
        assert_eq!(bar.reserved(), None);
        assert!(!bar.follow.is_confirmed());
        bar.release().unwrap();
        assert_eq!(bar.shell().removed.len(), 1, "only once");
        assert_eq!(
            bar.update(&a(), Edge::Bottom, 24),
            Err(DockError::NotReserved)
        );

        // A new window replaces the old registration, which is removed first.
        bar.reserve(handle(HWND), &a(), Edge::Bottom, 24).unwrap();
        bar.reserve(handle(HWND + 1), &a(), Edge::Bottom, 24)
            .unwrap();
        assert_eq!(bar.shell().removed, vec![HWND, HWND]);
        assert_eq!(bar.shell().registered, vec![HWND + 1]);
    }

    #[test]
    fn foreign_handles_and_refused_registrations() {
        let mut bar = appbar(&[a()]);
        assert_eq!(
            bar.reserve(
                RawWindowHandle::Xlib(XlibWindowHandle::new(5)),
                &a(),
                Edge::Bottom,
                24
            ),
            Ok(DockOutcome::Unsupported {
                reason: UnsupportedReason::UnsupportedWindowHandle
            })
        );
        bar.shell.registered.push(HWND);
        assert!(matches!(
            bar.reserve(handle(HWND), &a(), Edge::Bottom, 24),
            Err(DockError::Win32 { .. })
        ));
        assert_eq!(bar.window(), None);
        assert_eq!(show(None), "unknown");
    }

    #[test]
    fn notifications_reposition_and_follow_fullscreen_apps() {
        let mut bar = appbar(&[a()]);
        assert_eq!(bar.handle_native_message(0xC123, ABN_POSCHANGED, 0), None);
        let rect = reserve_and_settle(&mut bar, &a(), Edge::Bottom, 1);
        assert_eq!(bar.handle_native_message(0x0001, ABN_POSCHANGED, 0), None);
        // The taskbar grew: the strip moves up with it.
        bar.shell.bars[0].rect = PhysicalRect::new(0, 1000, 1920, 80);
        assert_eq!(
            bar.handle_native_message(0xC123, ABN_POSCHANGED, 0),
            Some(DockEvent::Repositioned {
                rect: PhysicalRect::new(0, 976, 1920, 24)
            })
        );
        assert_eq!(bar.window_port().rect, rect, "requested, not yet moved");
        pump(&mut bar);
        assert_eq!(bar.window_port().rect, PhysicalRect::new(0, 976, 1920, 24));
        assert_eq!(
            bar.handle_native_message(0xC123, ABN_FULLSCREENAPP, 1),
            Some(DockEvent::FullscreenEntered)
        );
        assert!(!bar.window_port().topmost);
        assert_eq!(
            bar.handle_native_message(0xC123, ABN_FULLSCREENAPP, 0),
            Some(DockEvent::FullscreenLeft)
        );
        assert!(bar.window_port().topmost);
        assert_eq!(
            bar.handle_native_message(0xC123, ABN_WINDOWARRANGE, 1),
            None
        );
        assert_eq!(bar.handle_native_message(0xC123, 42, 0), None);
    }
}
