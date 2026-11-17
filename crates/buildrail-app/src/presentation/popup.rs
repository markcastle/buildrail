//! Where the strip's pop-up cards go (details, overflow, update), in
//! physical pixels, decided without a window.
//!
//! The strip's own window is the reference: its real rect in physical
//! pixels, and the monitor it is on (bounds, work area and scale from the
//! platform's monitor list). A card opens away from the screen edge the
//! strip is docked to (upwards from a bottom strip, downwards from a top
//! one, and towards whichever side has room when floating), and is kept
//! inside *that* monitor's work area horizontally. Nothing assumes the
//! monitor starts at `(0, 0)` or shares the primary monitor's scale.
//!
//! The egui layer measures the card in points, converts it with the
//! strip's pixels-per-point (a pop-up on the same monitor has the same
//! scale), calls [`popup_rect`], and then moves the pop-up window until it
//! is there ([`PopupPlacement`]), keeping it hidden until it is.

use buildrail_platform::geometry::{MonitorInfo, PhysicalRect};

/// How the strip sits, as far as its pop-ups care.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StripSide {
    /// Docked at the top of its monitor: pop-ups open downwards.
    Top,
    /// Docked at the bottom: pop-ups open upwards.
    #[default]
    Bottom,
    /// A floating window: pop-ups open upwards when there is room above,
    /// else downwards.
    Floating,
}

/// Which end of the card lines up with the anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupAlign {
    /// The card's left edge is at the anchor (an entry's details).
    Start,
    /// The card's right edge is at the anchor (the update card, the
    /// overflow list at the strip's right end).
    End,
}

/// Everything [`popup_rect`] needs, in physical pixels.
#[derive(Debug, Clone, Copy)]
pub struct PopupRequest<'a> {
    /// The strip window's real rect.
    pub strip: PhysicalRect,
    /// Horizontal anchor: the card's left edge ([`PopupAlign::Start`]) or
    /// right edge ([`PopupAlign::End`]).
    pub anchor_x: i32,
    /// Which edge of the card `anchor_x` is.
    pub align: PopupAlign,
    /// The card window's size.
    pub width: u32,
    /// The card window's height.
    pub height: u32,
    /// How the strip sits.
    pub side: StripSide,
    /// The connected monitors (the strip's is picked from these).
    pub monitors: &'a [MonitorInfo],
}

/// The monitor `rect` is (mostly) on, if it touches any.
pub fn monitor_under<'a>(
    rect: &PhysicalRect,
    monitors: &'a [MonitorInfo],
) -> Option<&'a MonitorInfo> {
    monitors
        .iter()
        .filter(|m| m.bounds.overlap_area(rect) > 0)
        .max_by_key(|m| m.bounds.overlap_area(rect))
}

/// The pop-up window's rect. See the module docs.
pub fn popup_rect(request: &PopupRequest<'_>) -> PhysicalRect {
    let PopupRequest {
        strip,
        anchor_x,
        align,
        width,
        height,
        side,
        monitors,
    } = *request;
    let len = |v: u32| i64::from(v);
    let x = match align {
        PopupAlign::Start => i64::from(anchor_x),
        PopupAlign::End => i64::from(anchor_x) - len(width),
    };
    let above = i64::from(strip.y) - len(height);
    let below = i64::from(strip.bottom());
    let monitor = monitor_under(&strip, monitors);
    let y = match (side, monitor) {
        (StripSide::Top, _) => below,
        (StripSide::Bottom, _) | (StripSide::Floating, None) => above,
        (StripSide::Floating, Some(m)) => {
            let work = usable(m);
            let room_above = i64::from(strip.y) - i64::from(work.y);
            let room_below = i64::from(work.bottom()) - below;
            if room_above >= len(height) || room_above >= room_below {
                above
            } else {
                below
            }
        }
    };
    let rect = PhysicalRect::new(saturate(x), saturate(y), width, height);
    let Some(monitor) = monitor else {
        return rect;
    };
    // Sideways: inside the work area (clear of a side taskbar or appbar).
    // Up and down: inside the monitor, so a card never lands on the
    // neighbouring screen.
    let work = usable(monitor);
    let horizontal = PhysicalRect::new(work.x, rect.y, work.width, rect.height);
    let vertical = PhysicalRect::new(rect.x, monitor.bounds.y, rect.width, monitor.bounds.height);
    PhysicalRect::new(
        rect.clamped_into(&horizontal).x,
        rect.clamped_into(&vertical).y,
        width,
        height,
    )
}

fn usable(monitor: &MonitorInfo) -> PhysicalRect {
    if monitor.work_area.is_empty() {
        monitor.bounds
    } else {
        monitor.work_area
    }
}

fn saturate(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
}

/// How many times a pop-up window is moved before it is shown wherever it
/// ended up (a window manager that will not let it go there).
pub const MAX_POPUP_MOVES: u32 = 3;

/// What to do with a pop-up window this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupStep {
    /// Move it to the wanted rect (and paint again to check).
    Move,
    /// It is in place (or will not get closer): show it.
    Show,
    /// Nothing to do.
    Stay,
}

/// Places one pop-up window: it is created hidden, moved until its real
/// position is the wanted one (the first move can land it on a monitor
/// with another scale, which the toolkit answers by resizing and nudging
/// it), and only then shown. Reset it when the pop-up closes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PopupPlacement {
    target: Option<PhysicalRect>,
    moves: u32,
    shown: bool,
}

impl PopupPlacement {
    /// A pop-up that has not been placed or shown.
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` once the pop-up has been shown.
    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// The next step for a pop-up window that is really at `actual`
    /// (`None` where the platform does not say, e.g. Wayland) and should
    /// be at `wanted`. Positions within a pixel count as in place.
    pub fn step(&mut self, actual: Option<PhysicalRect>, wanted: &PhysicalRect) -> PopupStep {
        if self.target != Some(*wanted) {
            self.target = Some(*wanted);
            self.moves = 0;
        }
        let in_place = actual.is_some_and(|a| {
            (i64::from(a.x) - i64::from(wanted.x)).abs() <= 1
                && (i64::from(a.y) - i64::from(wanted.y)).abs() <= 1
        });
        if !in_place && actual.is_some() && self.moves < MAX_POPUP_MOVES {
            self.moves += 1;
            return PopupStep::Move;
        }
        if self.shown {
            PopupStep::Stay
        } else {
            self.shown = true;
            PopupStep::Show
        }
    }
}

#[cfg(test)]
mod tests {
    use buildrail_platform::geometry::MonitorId;

    use super::*;

    fn monitor(id: &str, bounds: PhysicalRect, work: PhysicalRect, scale: f64) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(id),
            bounds,
            work_area: work,
            scale_factor: scale,
            is_primary: id == "primary",
        }
    }

    /// 1920x1080 at 100 %, 40 px taskbar at the bottom, our 24 px strip
    /// reserved above it.
    fn primary() -> MonitorInfo {
        monitor(
            "primary",
            PhysicalRect::new(0, 0, 1920, 1080),
            PhysicalRect::new(0, 0, 1920, 1016),
            1.0,
        )
    }

    /// 3840x2160 at 200 % to the right.
    fn right_200() -> MonitorInfo {
        monitor(
            "right",
            PhysicalRect::new(1920, 0, 3840, 2160),
            PhysicalRect::new(1920, 0, 3840, 2032),
            2.0,
        )
    }

    /// 2560x1440 at 150 % to the left and above (negative origin).
    fn left_150() -> MonitorInfo {
        monitor(
            "left",
            PhysicalRect::new(-2560, -360, 2560, 1440),
            PhysicalRect::new(-2560, -360, 2560, 1344),
            1.5,
        )
    }

    fn all() -> Vec<MonitorInfo> {
        vec![primary(), right_200(), left_150()]
    }

    fn request(
        strip: PhysicalRect,
        anchor_x: i32,
        align: PopupAlign,
        size: (u32, u32),
        side: StripSide,
        monitors: &[MonitorInfo],
    ) -> PhysicalRect {
        popup_rect(&PopupRequest {
            strip,
            anchor_x,
            align,
            width: size.0,
            height: size.1,
            side,
            monitors,
        })
    }

    #[test]
    fn bottom_strip_on_the_primary_opens_upwards_at_the_entry() {
        let monitors = all();
        let strip = PhysicalRect::new(0, 1016, 1920, 24);
        let rect = request(
            strip,
            300,
            PopupAlign::Start,
            (320, 200),
            StripSide::Bottom,
            &monitors,
        );
        assert_eq!(rect, PhysicalRect::new(300, 816, 320, 200));
        assert_eq!(rect.bottom(), strip.y, "sits on the strip");
    }

    /// The owner's second report: on the second screen the pop-ups opened
    /// on the first one. The old code clamped x into `[0, monitor width]`,
    /// as if every monitor started at (0, 0), and mixed the two monitors'
    /// scales.
    #[test]
    fn a_strip_on_a_monitor_to_the_right_keeps_its_popups_there() {
        let monitors = all();
        // Docked on the 200 % monitor: 48 px tall.
        let strip = PhysicalRect::new(1920, 2032, 3840, 48);
        // A card of 320x200 points is 640x400 physical there.
        let rect = request(
            strip,
            1920 + 2000,
            PopupAlign::Start,
            (640, 400),
            StripSide::Bottom,
            &monitors,
        );
        assert_eq!(rect, PhysicalRect::new(3920, 1632, 640, 400));
        assert!(right_200().bounds.contains_rect(&rect));
        // Near its right end, the card is pulled back inside that monitor,
        // not inside the primary.
        let rect = request(
            strip,
            1920 + 3700,
            PopupAlign::Start,
            (640, 400),
            StripSide::Bottom,
            &monitors,
        );
        assert_eq!(rect.right(), right_200().bounds.right());
    }

    #[test]
    fn negative_origin_monitor_to_the_left_and_above() {
        let monitors = all();
        let strip = PhysicalRect::new(-2560, 984, 2560, 36);
        let rect = request(
            strip,
            -2600,
            PopupAlign::Start,
            (480, 300),
            StripSide::Bottom,
            &monitors,
        );
        assert_eq!(rect, PhysicalRect::new(-2560, 684, 480, 300));
        // Docked at its top edge (y = -360): opens downwards, still there.
        let strip = PhysicalRect::new(-2560, -360, 2560, 36);
        let rect = request(
            strip,
            -100,
            PopupAlign::End,
            (480, 300),
            StripSide::Top,
            &monitors,
        );
        assert_eq!(rect, PhysicalRect::new(-580, -324, 480, 300));
        assert!(left_150().bounds.contains_rect(&rect));
    }

    #[test]
    fn top_strip_opens_downwards() {
        let monitors = all();
        let strip = PhysicalRect::new(0, 0, 1920, 24);
        let rect = request(
            strip,
            1920,
            PopupAlign::End,
            (340, 120),
            StripSide::Top,
            &monitors,
        );
        assert_eq!(rect, PhysicalRect::new(1580, 24, 340, 120));
    }

    #[test]
    fn floating_strip_opens_where_there_is_room() {
        let monitors = all();
        let near_top = PhysicalRect::new(500, 40, 760, 24);
        let rect = request(
            near_top,
            600,
            PopupAlign::Start,
            (320, 200),
            StripSide::Floating,
            &monitors,
        );
        assert_eq!(rect.y, 64, "no room above: below");
        let near_bottom = PhysicalRect::new(500, 980, 760, 24);
        let rect = request(
            near_bottom,
            600,
            PopupAlign::Start,
            (320, 200),
            StripSide::Floating,
            &monitors,
        );
        assert_eq!(rect.bottom(), 980, "room above: above");
        // Room on neither side: the side with more, clamped on screen.
        let tiny = monitor(
            "primary",
            PhysicalRect::new(0, 0, 800, 300),
            PhysicalRect::new(0, 0, 800, 300),
            1.0,
        );
        let middle = PhysicalRect::new(0, 100, 800, 24);
        let rect = request(
            middle,
            0,
            PopupAlign::Start,
            (320, 250),
            StripSide::Floating,
            &[tiny],
        );
        assert_eq!(rect.y, 124 - 74, "below, pulled up to stay on the monitor");
    }

    #[test]
    fn a_card_wider_than_the_space_left_starts_at_the_work_area_edge() {
        let monitors = vec![monitor(
            "primary",
            PhysicalRect::new(0, 0, 1024, 768),
            PhysicalRect::new(62, 0, 962, 744),
            1.0,
        )];
        let strip = PhysicalRect::new(62, 720, 962, 24);
        let rect = request(
            strip,
            40,
            PopupAlign::Start,
            (2000, 100),
            StripSide::Bottom,
            &monitors,
        );
        assert_eq!(
            rect.x, 62,
            "left edge of the work area, clear of a side taskbar"
        );
        let rect = request(
            strip,
            70,
            PopupAlign::End,
            (300, 100),
            StripSide::Bottom,
            &monitors,
        );
        assert_eq!(
            rect.x, 62,
            "an end-aligned card past the left edge comes back"
        );
    }

    #[test]
    fn mixed_scales_use_only_the_strips_monitor() {
        // 100 % primary, 150 % on the left, 200 % on the right: the same
        // strip-relative card lands on each strip's own monitor.
        for m in all() {
            let height = m.to_physical(24);
            let strip = PhysicalRect::new(m.bounds.x, m.work_area.bottom(), m.bounds.width, height);
            let size = (m.to_physical(320), m.to_physical(140));
            let rect = request(
                strip,
                m.bounds.right() - 10,
                PopupAlign::Start,
                size,
                StripSide::Bottom,
                &all(),
            );
            assert!(m.bounds.contains_rect(&rect), "{} {rect}", m.id);
            assert_eq!(rect.bottom(), strip.y);
        }
    }

    #[test]
    fn without_a_known_monitor_nothing_is_clamped() {
        let strip = PhysicalRect::new(5000, 5000, 100, 24);
        let rect = request(
            strip,
            5000,
            PopupAlign::Start,
            (100, 50),
            StripSide::Floating,
            &all(),
        );
        assert_eq!(rect, PhysicalRect::new(5000, 4950, 100, 50));
        assert!(monitor_under(&strip, &all()).is_none());
        assert_eq!(saturate(i64::MAX), i32::MAX);
        assert_eq!(saturate(i64::MIN), i32::MIN);
    }

    #[test]
    fn placement_moves_until_in_place_then_shows_once() {
        let wanted = PhysicalRect::new(3920, 1632, 640, 400);
        let mut placement = PopupPlacement::new();
        // Created on the primary, at the wrong place.
        let created = PhysicalRect::new(1960, 816, 320, 200);
        assert_eq!(placement.step(Some(created), &wanted), PopupStep::Move);
        // The move landed it, and the toolkit nudged it for the new scale.
        let nudged = PhysicalRect::new(3900, 1632, 640, 400);
        assert_eq!(placement.step(Some(nudged), &wanted), PopupStep::Move);
        assert!(!placement.is_shown());
        assert_eq!(placement.step(Some(wanted), &wanted), PopupStep::Show);
        assert!(placement.is_shown());
        assert_eq!(placement.step(Some(wanted), &wanted), PopupStep::Stay);
        // A pixel of rounding is in place.
        let off_by_one = PhysicalRect::new(3921, 1631, 640, 400);
        assert_eq!(placement.step(Some(off_by_one), &wanted), PopupStep::Stay);
        // Another entry: move the visible pop-up, nothing else.
        let next = PhysicalRect::new(4200, 1632, 640, 400);
        assert_eq!(placement.step(Some(wanted), &next), PopupStep::Move);
        assert_eq!(placement.step(Some(next), &next), PopupStep::Stay);
    }

    #[test]
    fn placement_gives_up_moving_and_shows_where_positions_are_unknown_or_refused() {
        let wanted = PhysicalRect::new(100, 100, 10, 10);
        let mut placement = PopupPlacement::new();
        assert_eq!(placement.step(None, &wanted), PopupStep::Show);
        let mut stubborn = PopupPlacement::new();
        let elsewhere = Some(PhysicalRect::new(0, 0, 10, 10));
        for _ in 0..MAX_POPUP_MOVES {
            assert_eq!(stubborn.step(elsewhere, &wanted), PopupStep::Move);
        }
        assert_eq!(stubborn.step(elsewhere, &wanted), PopupStep::Show);
        assert_eq!(stubborn.step(elsewhere, &wanted), PopupStep::Stay);
    }
}
