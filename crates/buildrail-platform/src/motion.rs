//! Whether the user asked the desktop for less motion.
//!
//! Only answered where it is one cheap call: on Windows the "Show
//! animations in Windows" setting (`SPI_GETCLIENTAREAANIMATION`). Linux
//! desktops keep it in per-desktop settings stores and macOS behind
//! AppKit, so there the answer is `None` ("not known") and the app's own
//! Animation preference decides alone.

/// `Some(true)` when the user turned animations off, `Some(false)` when
/// they are on, `None` when this platform does not say cheaply.
pub fn prefers_reduced_motion() -> Option<bool> {
    #[cfg(windows)]
    {
        crate::win32::motion::prefers_reduced_motion()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn answers_without_failing() {
        let answer = super::prefers_reduced_motion();
        if cfg!(not(windows)) {
            assert_eq!(answer, None);
        }
    }
}
