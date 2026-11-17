//! The "Show animations in Windows" setting. See [`crate::motion`].

use windows_sys::core::BOOL;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION,
};

/// `Some(true)` when the user turned animations off.
pub(crate) fn prefers_reduced_motion() -> Option<bool> {
    let mut enabled: BOOL = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL through the pointer,
    // which points at `enabled`, alive for the call.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut enabled as *mut BOOL).cast(),
            0,
        )
    };
    (ok != 0).then_some(enabled == 0)
}
