//! Fitting entries into the strip's width.
//!
//! The strip never wraps or scrolls: entries are shown in pinned order
//! until the next one would not fit, and the rest collapse into one
//! overflow button ("+3 · 2 failed"). Failures hidden in the overflow are
//! counted so a red build is never silently off-screen.
//!
//! The function is pure (widths in, counts out) so the egui layer measures
//! text and this module decides.

use super::strip::StripEntry;

/// Result of [`fit`]: how many leading entries are visible, and what the
/// overflow group holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverflowLayout {
    /// Entries `0..visible` are drawn on the strip.
    pub visible: usize,
    /// The overflow group, `None` when everything fits.
    pub overflow: Option<OverflowGroup>,
}

/// The entries that did not fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverflowGroup {
    /// Number of hidden entries.
    pub hidden: usize,
    /// How many of them show a failed run.
    pub failures: usize,
}

impl OverflowGroup {
    /// Button text: `+3` or `+3 · 2 failed`.
    pub fn text(&self) -> String {
        if self.failures == 0 {
            format!("+{}", self.hidden)
        } else {
            format!("+{} · {} failed", self.hidden, self.failures)
        }
    }

    /// Accessible name: `3 more builds, 2 failed`.
    pub fn accessible_name(&self) -> String {
        let builds = if self.hidden == 1 { "build" } else { "builds" };
        if self.failures == 0 {
            format!("{} more {builds}", self.hidden)
        } else {
            format!("{} more {builds}, {} failed", self.hidden, self.failures)
        }
    }
}

/// Decides how many entries fit.
///
/// * `widths[i]` – measured width of entry `i` (same order as `entries`).
/// * `spacing` – gap drawn *before* every entry after the first, and before
///   the overflow button.
/// * `available` – width left for entries and the overflow button.
/// * `overflow_width` – width reserved for the overflow button when needed.
///
/// If everything fits, nothing overflows. Otherwise the longest prefix that
/// fits *together with* the overflow button is shown (possibly none).
pub fn fit(
    entries: &[StripEntry],
    widths: &[f32],
    spacing: f32,
    available: f32,
    overflow_width: f32,
) -> OverflowLayout {
    let n = entries.len().min(widths.len());
    let total = |count: usize| -> f32 {
        let gaps = count.saturating_sub(1) as f32 * spacing;
        widths[..count].iter().sum::<f32>() + gaps
    };
    if total(n) <= available {
        return OverflowLayout {
            visible: n,
            overflow: None,
        };
    }
    let mut visible = 0;
    for count in (0..n).rev() {
        let button = if count == 0 {
            overflow_width
        } else {
            spacing + overflow_width
        };
        if total(count) + button <= available {
            visible = count;
            break;
        }
    }
    let hidden = &entries[visible..n];
    OverflowLayout {
        visible,
        overflow: Some(OverflowGroup {
            hidden: hidden.len(),
            failures: hidden.iter().filter(|e| e.is_failure()).count(),
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use buildrail_core::domain::{ProviderKind, Tone, WatchId};

    use super::*;
    use crate::presentation::IconKind;

    fn e(tone: Tone) -> StripEntry {
        StripEntry {
            watch_id: WatchId::new("x"),
            provider: ProviderKind::GitHub,
            label: "x".into(),
            status_text: "x".into(),
            tone,
            icon: IconKind::Check,
            note: None,
            stale: false,
            needs_reconnect: false,
            details: vec![],
            hint: None,
            accessible_name: "x".into(),
            url: None,
            ages: vec![],
        }
    }

    #[test]
    fn everything_fits() {
        let entries = [e(Tone::Green), e(Tone::Red)];
        let layout = fit(&entries, &[100.0, 100.0], 10.0, 210.0, 50.0);
        assert_eq!(
            layout,
            OverflowLayout {
                visible: 2,
                overflow: None
            }
        );
        assert_eq!(fit(&[], &[], 10.0, 0.0, 50.0).visible, 0);
    }

    #[test]
    fn overflow_counts_hidden_failures() {
        let entries = [e(Tone::Green), e(Tone::Red), e(Tone::Red), e(Tone::Amber)];
        // 100 + 10 + 100 = 210 → +10+50 = 270 fits in 280; three would need 380.
        let layout = fit(&entries, &[100.0; 4], 10.0, 280.0, 50.0);
        assert_eq!(layout.visible, 2);
        let group = layout.overflow.unwrap();
        assert_eq!(group.hidden, 2);
        assert_eq!(group.failures, 1);
        assert_eq!(group.text(), "+2 · 1 failed");
        assert_eq!(group.accessible_name(), "2 more builds, 1 failed");
    }

    #[test]
    fn nothing_fits_but_the_button() {
        let entries = [e(Tone::Red)];
        let layout = fit(&entries, &[300.0], 10.0, 60.0, 50.0);
        assert_eq!(layout.visible, 0);
        let group = layout.overflow.unwrap();
        assert_eq!(group.text(), "+1 · 1 failed");
        assert_eq!(group.accessible_name(), "1 more build, 1 failed");
        // Even the button does not fit: still report the overflow.
        let layout = fit(&entries, &[300.0], 10.0, 10.0, 50.0);
        assert_eq!(layout.visible, 0);
        assert!(layout.overflow.is_some());
    }

    #[test]
    fn no_failures_text() {
        let group = OverflowGroup {
            hidden: 3,
            failures: 0,
        };
        assert_eq!(group.text(), "+3");
        assert_eq!(group.accessible_name(), "3 more builds");
    }
}
