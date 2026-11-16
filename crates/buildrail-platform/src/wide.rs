//! UTF-16 helpers for Win32 calls. Platform-independent so they are tested
//! everywhere; only the `win32` module uses them.
#![cfg_attr(not(windows), allow(dead_code))]

/// `text` as a NUL-terminated UTF-16 string.
pub(crate) fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A UTF-16 buffer up to its first NUL, lossily decoded.
pub(crate) fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Display scale from a Windows DPI value (96 DPI = 100 %). Zero means the
/// call failed: assume 100 %.
pub(crate) fn scale_from_dpi(dpi: u32) -> f64 {
    if dpi == 0 {
        1.0
    } else {
        f64::from(dpi) / 96.0
    }
}

/// Window classes of the Windows shell (desktop and taskbars). A
/// foreground window of one of these classes is never "a fullscreen app",
/// even though the desktop window covers the whole monitor.
pub(crate) fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "WorkerW" | "Progman" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_round_trip() {
        assert_eq!(to_wide("ab"), vec![97, 98, 0]);
        assert_eq!(from_wide(&to_wide("\\\\.\\DISPLAY1")), "\\\\.\\DISPLAY1");
        assert_eq!(from_wide(&[104, 105]), "hi", "no NUL");
        assert_eq!(from_wide(&[0, 104]), "");
    }

    #[test]
    fn dpi_to_scale() {
        assert_eq!(scale_from_dpi(96), 1.0);
        assert_eq!(scale_from_dpi(120), 1.25);
        assert_eq!(scale_from_dpi(144), 1.5);
        assert_eq!(scale_from_dpi(0), 1.0);
    }

    #[test]
    fn shell_classes() {
        assert!(is_shell_class("WorkerW"));
        assert!(is_shell_class("Shell_TrayWnd"));
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
    }
}
