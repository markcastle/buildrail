//! EWMH strut values for an X11 dock window (pure maths).
//!
//! `_NET_WM_STRUT_PARTIAL` holds twelve `CARDINAL`s: the left, right, top and
//! bottom reservation **measured from the edges of the whole X screen (root
//! window)**, followed by the start/end coordinate of each reservation along
//! its edge (inclusive). `_NET_WM_STRUT` is the older four-value form that
//! reserves the whole edge; we set both because some window managers only
//! read the old one.
//!
//! Multi-monitor caveat (an EWMH limitation): because a bottom strut is
//! measured from the bottom of the *root window*, it only describes our strip
//! correctly when the strip's monitor reaches the bottom of the X screen.
//! On a monitor that sits higher than its neighbour, the strut also covers
//! the empty root area below that monitor; Mutter and KWin discard struts
//! that do not touch a monitor edge, others may reserve too much. A top
//! strut has the same caveat mirrored: it is measured from the top of the
//! root window.

use crate::geometry::{Edge, PhysicalRect};

/// The twelve values of `_NET_WM_STRUT_PARTIAL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StrutPartial {
    /// Pixels reserved at the left of the screen.
    pub left: u32,
    /// Pixels reserved at the right of the screen.
    pub right: u32,
    /// Pixels reserved at the top of the screen.
    pub top: u32,
    /// Pixels reserved at the bottom of the screen.
    pub bottom: u32,
    /// First row of the left reservation.
    pub left_start_y: u32,
    /// Last row (inclusive) of the left reservation.
    pub left_end_y: u32,
    /// First row of the right reservation.
    pub right_start_y: u32,
    /// Last row (inclusive) of the right reservation.
    pub right_end_y: u32,
    /// First column of the top reservation.
    pub top_start_x: u32,
    /// Last column (inclusive) of the top reservation.
    pub top_end_x: u32,
    /// First column of the bottom reservation.
    pub bottom_start_x: u32,
    /// Last column (inclusive) of the bottom reservation.
    pub bottom_end_x: u32,
}

impl StrutPartial {
    /// The property value, in EWMH order.
    pub fn to_cardinals(&self) -> [u32; 12] {
        [
            self.left,
            self.right,
            self.top,
            self.bottom,
            self.left_start_y,
            self.left_end_y,
            self.right_start_y,
            self.right_end_y,
            self.top_start_x,
            self.top_end_x,
            self.bottom_start_x,
            self.bottom_end_x,
        ]
    }

    /// The legacy `_NET_WM_STRUT` value (left, right, top, bottom).
    pub fn to_legacy_cardinals(&self) -> [u32; 4] {
        [self.left, self.right, self.top, self.bottom]
    }
}

/// The strut reserving `strip` at the bottom of an X screen
/// `screen_height` pixels tall.
///
/// Returns `None` when the strip is empty or not inside the screen (X11 root
/// coordinates are never negative).
pub fn bottom_strut(
    screen_width: u32,
    screen_height: u32,
    strip: &PhysicalRect,
) -> Option<StrutPartial> {
    let screen = PhysicalRect::new(0, 0, screen_width, screen_height);
    if strip.is_empty() || !screen.contains_rect(strip) {
        return None;
    }
    let top = u32::try_from(strip.y).ok()?;
    let start_x = u32::try_from(strip.x).ok()?;
    Some(StrutPartial {
        bottom: screen_height - top,
        bottom_start_x: start_x,
        bottom_end_x: start_x + strip.width - 1,
        ..StrutPartial::default()
    })
}

/// The strut reserving `strip` at the top of an X screen `screen_width`
/// by `screen_height` pixels: everything from the top of the root window
/// down to the strip's bottom edge, over the strip's columns.
///
/// Returns `None` when the strip is empty or not inside the screen.
pub fn top_strut(
    screen_width: u32,
    screen_height: u32,
    strip: &PhysicalRect,
) -> Option<StrutPartial> {
    let screen = PhysicalRect::new(0, 0, screen_width, screen_height);
    if strip.is_empty() || !screen.contains_rect(strip) {
        return None;
    }
    let bottom = u32::try_from(strip.bottom()).ok()?;
    let start_x = u32::try_from(strip.x).ok()?;
    Some(StrutPartial {
        top: bottom,
        top_start_x: start_x,
        top_end_x: start_x + strip.width - 1,
        ..StrutPartial::default()
    })
}

/// The strut reserving `strip` on `edge` ([`top_strut`] or
/// [`bottom_strut`]).
pub fn edge_strut(
    edge: Edge,
    screen_width: u32,
    screen_height: u32,
    strip: &PhysicalRect,
) -> Option<StrutPartial> {
    match edge {
        Edge::Top => top_strut(screen_width, screen_height, strip),
        Edge::Bottom => bottom_strut(screen_width, screen_height, strip),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_monitor_top_strut() {
        let strip = PhysicalRect::new(0, 0, 1920, 24);
        let strut = top_strut(1920, 1080, &strip).unwrap();
        assert_eq!(
            strut.to_cardinals(),
            [0, 0, 24, 0, 0, 0, 0, 0, 0, 1919, 0, 0]
        );
        assert_eq!(strut.to_legacy_cardinals(), [0, 0, 24, 0]);
        assert_eq!(edge_strut(Edge::Top, 1920, 1080, &strip), Some(strut));
    }

    #[test]
    fn top_strut_below_a_top_panel_and_on_the_right_monitor() {
        // A 32 px top panel: the strip sits under it, the strut covers both.
        let strip = PhysicalRect::new(1920, 32, 1920, 36);
        let strut = top_strut(3840, 1080, &strip).unwrap();
        assert_eq!(strut.top, 68);
        assert_eq!((strut.top_start_x, strut.top_end_x), (1920, 3839));
        assert_eq!(strut.bottom, 0);
    }

    #[test]
    fn top_strut_rejects_rects_outside_the_root() {
        assert_eq!(
            top_strut(1920, 1080, &PhysicalRect::new(0, -1, 10, 24)),
            None
        );
        assert_eq!(top_strut(1920, 1080, &PhysicalRect::new(0, 0, 0, 24)), None);
        assert_eq!(
            edge_strut(
                Edge::Bottom,
                1920,
                1080,
                &PhysicalRect::new(0, 1056, 1920, 24)
            ),
            bottom_strut(1920, 1080, &PhysicalRect::new(0, 1056, 1920, 24))
        );
    }

    #[test]
    fn single_monitor_bottom_strut() {
        let strip = PhysicalRect::new(0, 1008, 1920, 24);
        let strut = bottom_strut(1920, 1080, &strip).unwrap();
        assert_eq!(
            strut.to_cardinals(),
            [0, 0, 0, 72, 0, 0, 0, 0, 0, 0, 0, 1919]
        );
        assert_eq!(strut.to_legacy_cardinals(), [0, 0, 0, 72]);
    }

    #[test]
    fn right_hand_monitor_limits_the_columns() {
        // Two 1920x1080 monitors side by side; strip on the right one, flush
        // with the bottom.
        let strip = PhysicalRect::new(1920, 1056, 1920, 24);
        let strut = bottom_strut(3840, 1080, &strip).unwrap();
        assert_eq!(strut.bottom, 24);
        assert_eq!((strut.bottom_start_x, strut.bottom_end_x), (1920, 3839));
    }

    #[test]
    fn shorter_monitor_measures_from_the_root_bottom() {
        // 1920x1080 next to a 2560x1440: the root is 1440 tall.
        let strip = PhysicalRect::new(0, 1056, 1920, 24);
        assert_eq!(bottom_strut(4480, 1440, &strip).unwrap().bottom, 384);
    }

    #[test]
    fn rejects_rects_outside_the_root() {
        assert_eq!(
            bottom_strut(1920, 1080, &PhysicalRect::new(-1, 1000, 10, 24)),
            None
        );
        assert_eq!(
            bottom_strut(1920, 1080, &PhysicalRect::new(0, 1070, 10, 24)),
            None
        );
        assert_eq!(
            bottom_strut(1920, 1080, &PhysicalRect::new(0, 1000, 0, 24)),
            None
        );
    }
}
