//! Pure placement maths for the strip: no OS calls, fully unit tested.
//!
//! All rectangles are in **physical pixels** in the desktop's global
//! coordinate space (the same space winit reports monitor positions in).
//! On Windows that space can have negative coordinates (a monitor to the
//! left of or above the primary one); on X11 it is the root window and starts
//! at `(0, 0)`.
//!
//! The strip's height is given in **logical pixels** (24) and converted with
//! each monitor's scale factor using the same rounding winit uses
//! (round-half-away-from-zero), so a 24-logical-pixel egui panel exactly
//! fills the window at 125 %, 150 %, 175 % and so on.

use std::fmt;

/// The strip height the product targets, in logical pixels.
pub const STRIP_HEIGHT_LOGICAL: u32 = 24;

/// Gap, in logical pixels, between the floating window and the edge of the
/// work area it defaults to when no position has been remembered.
pub const FLOAT_BOTTOM_MARGIN_LOGICAL: u32 = 8;

/// The screen edge a docked strip sits on (and a floating one defaults
/// to).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Edge {
    /// Along the top of the monitor.
    Top,
    /// Along the bottom of the monitor (directly above a bottom taskbar).
    #[default]
    Bottom,
}

impl Edge {
    /// `"top"` or `"bottom"`, for logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

impl fmt::Display for Edge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An axis-aligned rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PhysicalRect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width (may be zero).
    pub width: u32,
    /// Height (may be zero).
    pub height: u32,
}

impl PhysicalRect {
    /// Builds a rectangle from its top-left corner and size.
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Builds a rectangle from its edges (`right`/`bottom` exclusive). An
    /// inverted rectangle becomes empty.
    pub fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            x: left,
            y: top,
            width: span(left, right),
            height: span(top, bottom),
        }
    }

    /// Exclusive right edge.
    pub fn right(&self) -> i32 {
        add_len(self.x, self.width)
    }

    /// Exclusive bottom edge.
    pub fn bottom(&self) -> i32 {
        add_len(self.y, self.height)
    }

    /// `true` when the rectangle covers no pixels.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The overlapping part of two rectangles, if any.
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        let rect = Self::from_edges(left, top, right, bottom);
        (!rect.is_empty()).then_some(rect)
    }

    /// Number of pixels in the rectangle.
    pub fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// Number of pixels shared with `other`.
    pub fn overlap_area(&self, other: &Self) -> u64 {
        self.intersection(other).map_or(0, |r| r.area())
    }

    /// `true` when `other` lies completely inside `self`.
    pub fn contains_rect(&self, other: &Self) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    /// Moves (never resizes) `self` so it lies inside `bounds` where
    /// possible. When `self` is larger than `bounds` along an axis it is
    /// aligned to the left/top edge of `bounds` on that axis.
    pub fn clamped_into(&self, bounds: &Self) -> Self {
        Self {
            x: clamp_axis(self.x, self.width, bounds.x, bounds.width),
            y: clamp_axis(self.y, self.height, bounds.y, bounds.height),
            ..*self
        }
    }
}

impl fmt::Display for PhysicalRect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}x{} at ({}, {})",
            self.width, self.height, self.x, self.y
        )
    }
}

fn span(start: i32, end: i32) -> u32 {
    u32::try_from(i64::from(end) - i64::from(start)).unwrap_or(0)
}

fn add_len(start: i32, len: u32) -> i32 {
    let end = i64::from(start) + i64::from(len);
    i32::try_from(end).unwrap_or(i32::MAX)
}

fn clamp_axis(pos: i32, len: u32, min: i32, avail: u32) -> i32 {
    if len >= avail {
        return min;
    }
    let max = i64::from(min) + i64::from(avail - len);
    let clamped = i64::from(pos).clamp(i64::from(min), max);
    // `clamped` lies between two values that came from i32s.
    i32::try_from(clamped).unwrap_or(min)
}

/// Stable name of a monitor as the windowing toolkit reports it
/// (winit's `MonitorHandle::name()`, e.g. `\\.\DISPLAY2` on Windows or
/// `HDMI-1` on X11). Stored in per-device preferences as "the chosen
/// display".
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MonitorId(String);

impl MonitorId {
    /// Wraps a toolkit monitor name.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MonitorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Everything placement needs to know about one display.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// Toolkit name of the monitor.
    pub id: MonitorId,
    /// Whole monitor, physical pixels, global coordinates.
    pub bounds: PhysicalRect,
    /// The part not covered by taskbars, docks and panels. Use `bounds`
    /// when the platform cannot tell (winit does not expose work areas).
    pub work_area: PhysicalRect,
    /// Display scaling (1.0 = 100 %, 1.5 = 150 %).
    pub scale_factor: f64,
    /// `true` for the OS's primary display.
    pub is_primary: bool,
}

impl MonitorInfo {
    /// A monitor whose work area is its full bounds (what you can build
    /// from winit's `MonitorHandle` alone).
    pub fn without_work_area(
        id: MonitorId,
        bounds: PhysicalRect,
        scale_factor: f64,
        is_primary: bool,
    ) -> Self {
        Self {
            id,
            bounds,
            work_area: bounds,
            scale_factor,
            is_primary,
        }
    }

    /// Converts a logical length to physical pixels on this monitor.
    pub fn to_physical(&self, logical: u32) -> u32 {
        to_physical(logical, self.scale_factor)
    }
}

/// Converts a logical length to physical pixels, rounding like winit
/// (`f64::round`). Non-finite or non-positive scale factors are treated as
/// 1.0, and a non-zero logical length never rounds to zero.
pub fn to_physical(logical: u32, scale_factor: f64) -> u32 {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let physical = (f64::from(logical) * scale).round();
    // `as` saturates for out-of-range floats, which is what we want here.
    let physical = physical as u32;
    if logical > 0 {
        physical.max(1)
    } else {
        physical
    }
}

/// Picks the monitor to place the strip on: the chosen one when it is still
/// connected, else the primary, else the first one. `None` only when the
/// list is empty.
pub fn resolve_monitor<'a>(
    monitors: &'a [MonitorInfo],
    chosen: Option<&MonitorId>,
) -> Option<&'a MonitorInfo> {
    chosen
        .and_then(|id| monitors.iter().find(|m| &m.id == id))
        .or_else(|| monitors.iter().find(|m| m.is_primary))
        .or_else(|| monitors.first())
}

/// The work area with our own previously reserved strip given back.
///
/// On X11 the window manager subtracts our strut from the work area it
/// reports, so recomputing the dock rect from that work area would move the
/// strip inwards by its own height each time. When `own` sits flush against
/// the `edge` side of `work_area` and overlaps it horizontally, the work
/// area is extended to cover it again.
pub fn work_area_without_own_strip(
    work_area: &PhysicalRect,
    edge: Edge,
    own: Option<&PhysicalRect>,
) -> PhysicalRect {
    let Some(own) = own else {
        return *work_area;
    };
    let overlaps = own.x < work_area.right() && own.right() > work_area.x;
    match edge {
        Edge::Bottom if overlaps && own.y == work_area.bottom() => {
            PhysicalRect::from_edges(work_area.x, work_area.y, work_area.right(), own.bottom())
        }
        Edge::Top if overlaps && own.bottom() == work_area.y => {
            PhysicalRect::from_edges(work_area.x, own.y, work_area.right(), work_area.bottom())
        }
        _ => *work_area,
    }
}

/// Where a docked strip goes: full work-area width, sitting directly on
/// the `edge` side of the work area (that is, immediately above a bottom
/// taskbar or panel, or below a top one).
///
/// `own_reservation` is the rect this strip currently reserves, if any (see
/// [`work_area_without_own_strip`]).
pub fn dock_rect(
    monitor: &MonitorInfo,
    edge: Edge,
    strip_height_logical: u32,
    own_reservation: Option<&PhysicalRect>,
) -> PhysicalRect {
    let work = work_area_without_own_strip(&monitor.work_area, edge, own_reservation);
    let height = monitor.to_physical(strip_height_logical).min(work.height);
    let y = match edge {
        Edge::Top => work.y,
        Edge::Bottom => work.bottom() - i32::try_from(height).unwrap_or(0),
    };
    PhysicalRect::new(work.x, y, work.width, height)
}

/// Size of the compact movable window, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicalSize {
    /// Width in logical pixels.
    pub width: u32,
    /// Height in logical pixels.
    pub height: u32,
}

/// A remembered top-left position of the floating window, physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PhysicalPosition {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
}

/// Where the floating (undocked) window goes.
///
/// * With a remembered position: the window keeps that position if it is
///   mostly visible; it is sized for, and clamped into the work area of, the
///   monitor it overlaps most. If it overlaps no monitor (the display it was
///   on has been unplugged), it is clamped onto the chosen/primary monitor.
/// * Without one: horizontally centred near the `edge` (bottom or top) of
///   the chosen (or primary) monitor's work area.
///
/// Returns `None` only when `monitors` is empty.
pub fn float_rect(
    monitors: &[MonitorInfo],
    chosen: Option<&MonitorId>,
    size: LogicalSize,
    remembered: Option<PhysicalPosition>,
    edge: Edge,
) -> Option<PhysicalRect> {
    let fallback = resolve_monitor(monitors, chosen)?;
    let Some(pos) = remembered else {
        return Some(default_float_rect(fallback, size, edge));
    };
    let probe_on = |m: &MonitorInfo| {
        PhysicalRect::new(
            pos.x,
            pos.y,
            m.to_physical(size.width),
            m.to_physical(size.height),
        )
    };
    let best = monitors
        .iter()
        .map(|m| (m, probe_on(m).overlap_area(&m.work_area)))
        .filter(|(_, overlap)| *overlap > 0)
        .max_by_key(|(_, overlap)| *overlap)
        .map(|(m, _)| m);
    let target = best.unwrap_or(fallback);
    Some(probe_on(target).clamped_into(&target.work_area))
}

fn default_float_rect(monitor: &MonitorInfo, size: LogicalSize, edge: Edge) -> PhysicalRect {
    let work = monitor.work_area;
    let width = monitor.to_physical(size.width);
    let height = monitor.to_physical(size.height);
    let margin = monitor.to_physical(FLOAT_BOTTOM_MARGIN_LOGICAL);
    let x = i64::from(work.x) + (i64::from(work.width) - i64::from(width)) / 2;
    let y = match edge {
        Edge::Top => i64::from(work.y) + i64::from(margin),
        Edge::Bottom => i64::from(work.bottom()) - i64::from(height) - i64::from(margin),
    };
    let rect = PhysicalRect::new(
        i32::try_from(x).unwrap_or(work.x),
        i32::try_from(y).unwrap_or(work.y),
        width,
        height,
    );
    rect.clamped_into(&work)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(
        id: &str,
        bounds: PhysicalRect,
        work: PhysicalRect,
        scale: f64,
        primary: bool,
    ) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(id),
            bounds,
            work_area: work,
            scale_factor: scale,
            is_primary: primary,
        }
    }

    /// 1920x1080 primary with a 48 px bottom taskbar.
    fn primary() -> MonitorInfo {
        monitor(
            "\\\\.\\DISPLAY1",
            PhysicalRect::new(0, 0, 1920, 1080),
            PhysicalRect::new(0, 0, 1920, 1032),
            1.0,
            true,
        )
    }

    /// 2560x1440 at 150 % to the left of the primary (negative x).
    fn left_hidpi() -> MonitorInfo {
        monitor(
            "\\\\.\\DISPLAY2",
            PhysicalRect::new(-2560, -200, 2560, 1440),
            PhysicalRect::new(-2560, -200, 2560, 1380),
            1.5,
            false,
        )
    }

    #[test]
    fn rect_edges_and_empty() {
        let r = PhysicalRect::new(-10, -20, 30, 40);
        assert_eq!((r.right(), r.bottom()), (20, 20));
        assert!(!r.is_empty());
        assert!(PhysicalRect::from_edges(5, 5, 4, 10).is_empty());
        assert_eq!(
            PhysicalRect::from_edges(1, 2, 4, 6),
            PhysicalRect::new(1, 2, 3, 4)
        );
        assert_eq!(r.area(), 1200);
        assert_eq!(r.to_string(), "30x40 at (-10, -20)");
    }

    #[test]
    fn rect_edges_saturate_instead_of_overflowing() {
        let r = PhysicalRect::new(i32::MAX - 1, 0, u32::MAX, 1);
        assert_eq!(r.right(), i32::MAX);
    }

    #[test]
    fn intersection_and_overlap() {
        let a = PhysicalRect::new(0, 0, 10, 10);
        let b = PhysicalRect::new(5, 5, 10, 10);
        assert_eq!(a.intersection(&b), Some(PhysicalRect::new(5, 5, 5, 5)));
        assert_eq!(a.overlap_area(&b), 25);
        let c = PhysicalRect::new(10, 0, 5, 5);
        assert_eq!(a.intersection(&c), None, "touching edges do not overlap");
        assert_eq!(a.overlap_area(&c), 0);
        assert!(a.contains_rect(&PhysicalRect::new(2, 2, 8, 8)));
        assert!(!a.contains_rect(&b));
    }

    #[test]
    fn clamping_moves_but_never_resizes() {
        let bounds = PhysicalRect::new(-100, -100, 200, 200);
        let r = PhysicalRect::new(90, 90, 50, 50);
        assert_eq!(r.clamped_into(&bounds), PhysicalRect::new(50, 50, 50, 50));
        let r = PhysicalRect::new(-500, -500, 50, 50);
        assert_eq!(
            r.clamped_into(&bounds),
            PhysicalRect::new(-100, -100, 50, 50)
        );
        let too_big = PhysicalRect::new(30, 30, 400, 10);
        assert_eq!(
            too_big.clamped_into(&bounds),
            PhysicalRect::new(-100, 30, 400, 10)
        );
    }

    #[test]
    fn physical_height_rounds_like_winit() {
        assert_eq!(to_physical(24, 1.0), 24);
        assert_eq!(to_physical(24, 1.25), 30);
        assert_eq!(to_physical(24, 1.5), 36);
        assert_eq!(to_physical(24, 1.75), 42);
        assert_eq!(to_physical(24, 2.0), 48);
        assert_eq!(to_physical(24, 1.1), 26); // 26.4
        assert_eq!(to_physical(24, 1.0625), 26); // 25.5 rounds away from zero
        assert_eq!(to_physical(24, 0.5), 12);
    }

    #[test]
    fn physical_height_survives_bad_scale_factors() {
        assert_eq!(to_physical(24, 0.0), 24);
        assert_eq!(to_physical(24, -2.0), 24);
        assert_eq!(to_physical(24, f64::NAN), 24);
        assert_eq!(to_physical(24, f64::INFINITY), 24);
        assert_eq!(to_physical(1, 0.01), 1, "never rounds to nothing");
        assert_eq!(to_physical(0, 2.0), 0);
    }

    #[test]
    fn resolve_prefers_chosen_then_primary_then_first() {
        let monitors = [left_hidpi(), primary()];
        let chosen = MonitorId::new("\\\\.\\DISPLAY2");
        assert_eq!(
            resolve_monitor(&monitors, Some(&chosen)).map(|m| &m.id),
            Some(&chosen)
        );
        let gone = MonitorId::new("\\\\.\\DISPLAY9");
        assert_eq!(
            resolve_monitor(&monitors, Some(&gone)).map(|m| m.is_primary),
            Some(true),
            "a disconnected choice falls back to the primary"
        );
        assert!(resolve_monitor(&monitors, None).is_some_and(|m| m.is_primary));
        let mut no_primary = left_hidpi();
        no_primary.id = MonitorId::new("x");
        let list = [no_primary.clone(), left_hidpi()];
        assert_eq!(
            resolve_monitor(&list, None).map(|m| &m.id),
            Some(&no_primary.id)
        );
        assert!(resolve_monitor(&[], None).is_none());
    }

    #[test]
    fn dock_sits_directly_above_the_taskbar() {
        let rect = dock_rect(&primary(), Edge::Bottom, STRIP_HEIGHT_LOGICAL, None);
        assert_eq!(rect, PhysicalRect::new(0, 1008, 1920, 24));
        assert_eq!(rect.bottom(), 1032);
    }

    #[test]
    fn dock_handles_negative_origins_and_fractional_scale() {
        let rect = dock_rect(&left_hidpi(), Edge::Bottom, STRIP_HEIGHT_LOGICAL, None);
        assert_eq!(rect, PhysicalRect::new(-2560, 1180 - 36, 2560, 36));
        let mut m = left_hidpi();
        m.scale_factor = 1.25;
        assert_eq!(dock_rect(&m, Edge::Bottom, 24, None).height, 30);
    }

    #[test]
    fn dock_respects_a_side_taskbar() {
        let m = monitor(
            "side",
            PhysicalRect::new(0, 0, 1920, 1080),
            PhysicalRect::new(62, 0, 1858, 1080),
            1.0,
            true,
        );
        assert_eq!(
            dock_rect(&m, Edge::Bottom, 24, None),
            PhysicalRect::new(62, 1056, 1858, 24)
        );
    }

    #[test]
    fn dock_does_not_climb_when_the_wm_subtracts_our_own_strut() {
        let m = primary();
        let first = dock_rect(&m, Edge::Bottom, 24, None);
        let mut shrunk = m.clone();
        shrunk.work_area = PhysicalRect::from_edges(0, 0, 1920, first.y);
        assert_eq!(dock_rect(&shrunk, Edge::Bottom, 24, Some(&first)), first);
    }

    #[test]
    fn own_reservation_elsewhere_is_ignored() {
        let m = primary();
        let elsewhere = PhysicalRect::new(-2560, 500, 100, 24);
        assert_eq!(
            work_area_without_own_strip(&m.work_area, Edge::Bottom, Some(&elsewhere)),
            m.work_area
        );
        let below_but_not_flush = PhysicalRect::new(0, 1040, 1920, 24);
        assert_eq!(
            work_area_without_own_strip(&m.work_area, Edge::Bottom, Some(&below_but_not_flush)),
            m.work_area
        );
    }

    #[test]
    fn dock_never_exceeds_a_tiny_work_area() {
        let m = monitor(
            "tiny",
            PhysicalRect::new(0, 0, 100, 10),
            PhysicalRect::new(0, 0, 100, 10),
            2.0,
            true,
        );
        assert_eq!(
            dock_rect(&m, Edge::Bottom, 24, None),
            PhysicalRect::new(0, 0, 100, 10)
        );
    }

    #[test]
    fn top_dock_sits_directly_below_a_top_taskbar() {
        let m = monitor(
            "top-taskbar",
            PhysicalRect::new(0, 0, 1920, 1080),
            PhysicalRect::new(0, 40, 1920, 1040),
            1.0,
            true,
        );
        assert_eq!(
            dock_rect(&m, Edge::Top, 24, None),
            PhysicalRect::new(0, 40, 1920, 24)
        );
        // A monitor above and to the left (negative origin) at 150 %.
        assert_eq!(
            dock_rect(&left_hidpi(), Edge::Top, 24, None),
            PhysicalRect::new(-2560, -200, 2560, 36)
        );
    }

    #[test]
    fn top_dock_does_not_sink_when_the_wm_subtracts_our_own_strut() {
        let m = primary();
        let first = dock_rect(&m, Edge::Top, 24, None);
        assert_eq!(first, PhysicalRect::new(0, 0, 1920, 24));
        let mut shrunk = m.clone();
        shrunk.work_area = PhysicalRect::from_edges(0, first.bottom(), 1920, 1032);
        assert_eq!(dock_rect(&shrunk, Edge::Top, 24, Some(&first)), first);
        // A strip reserved at the bottom does not count for the top edge.
        let bottom = dock_rect(&m, Edge::Bottom, 24, None);
        assert_eq!(
            work_area_without_own_strip(&m.work_area, Edge::Top, Some(&bottom)),
            m.work_area
        );
    }

    #[test]
    fn edge_names() {
        assert_eq!(Edge::Top.to_string(), "top");
        assert_eq!(Edge::Bottom.as_str(), "bottom");
        assert_eq!(Edge::default(), Edge::Bottom);
    }

    #[test]
    fn float_defaults_near_the_top_when_asked() {
        let monitors = [primary(), left_hidpi()];
        assert_eq!(
            float_rect(&monitors, None, FLOAT, None, Edge::Top),
            Some(PhysicalRect::new(760, 8, 400, 24))
        );
    }

    const FLOAT: LogicalSize = LogicalSize {
        width: 400,
        height: 24,
    };

    #[test]
    fn float_defaults_to_bottom_centre_of_the_chosen_monitor() {
        let monitors = [primary(), left_hidpi()];
        let rect = float_rect(&monitors, None, FLOAT, None, Edge::Bottom);
        assert_eq!(rect, Some(PhysicalRect::new(760, 1032 - 24 - 8, 400, 24)));
        let chosen = MonitorId::new("\\\\.\\DISPLAY2");
        let rect = float_rect(&monitors, Some(&chosen), FLOAT, None, Edge::Bottom);
        // 600x36 physical, centred in x on [-2560, 0), 12 px margin.
        assert_eq!(
            rect,
            Some(PhysicalRect::new(-1580, 1180 - 36 - 12, 600, 36))
        );
    }

    #[test]
    fn float_keeps_a_visible_remembered_position() {
        let monitors = [primary(), left_hidpi()];
        let pos = PhysicalPosition { x: 100, y: 200 };
        assert_eq!(
            float_rect(&monitors, None, FLOAT, Some(pos), Edge::Bottom),
            Some(PhysicalRect::new(100, 200, 400, 24))
        );
        // On the high-DPI monitor it is sized with that monitor's scale.
        let pos = PhysicalPosition { x: -2000, y: 0 };
        assert_eq!(
            float_rect(&monitors, None, FLOAT, Some(pos), Edge::Bottom),
            Some(PhysicalRect::new(-2000, 0, 600, 36))
        );
    }

    #[test]
    fn float_is_clamped_back_onto_the_screen() {
        let monitors = [primary()];
        let pos = PhysicalPosition { x: 1800, y: 1070 };
        assert_eq!(
            float_rect(&monitors, None, FLOAT, Some(pos), Edge::Bottom),
            Some(PhysicalRect::new(1520, 1008, 400, 24)),
            "partly off the bottom-right corner and over the taskbar"
        );
    }

    #[test]
    fn float_from_a_disconnected_monitor_lands_on_the_fallback() {
        let monitors = [primary()];
        let pos = PhysicalPosition { x: -2000, y: 300 };
        assert_eq!(
            float_rect(&monitors, None, FLOAT, Some(pos), Edge::Bottom),
            Some(PhysicalRect::new(0, 300, 400, 24))
        );
        assert_eq!(float_rect(&[], None, FLOAT, Some(pos), Edge::Bottom), None);
        assert_eq!(float_rect(&[], None, FLOAT, None, Edge::Bottom), None);
    }

    #[test]
    fn float_straddling_two_monitors_goes_where_most_of_it_is() {
        let monitors = [primary(), left_hidpi()];
        // 600 px wide on the left monitor: x in [-100, 500) is mostly primary.
        let pos = PhysicalPosition { x: -100, y: 100 };
        assert_eq!(
            float_rect(&monitors, None, FLOAT, Some(pos), Edge::Bottom),
            Some(PhysicalRect::new(0, 100, 400, 24))
        );
    }

    #[test]
    fn monitor_helpers() {
        let m = MonitorInfo::without_work_area(
            MonitorId::new("a"),
            PhysicalRect::new(0, 0, 10, 10),
            2.0,
            false,
        );
        assert_eq!(m.work_area, m.bounds);
        assert_eq!(m.to_physical(3), 6);
        assert_eq!(m.id.as_str(), "a");
        assert_eq!(m.id.to_string(), "a");
    }
}
