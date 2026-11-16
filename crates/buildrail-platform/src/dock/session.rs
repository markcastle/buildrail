//! Which display server a Linux/BSD desktop session runs.
//!
//! Detection mirrors winit's own backend choice so the answer matches the
//! window the UI actually gets: a non-empty `WAYLAND_DISPLAY` (or
//! `WAYLAND_SOCKET`) means winit will open a Wayland window; otherwise a
//! non-empty `DISPLAY` means X11. `XDG_SESSION_TYPE` is only consulted when
//! neither socket variable is set.
//!
//! Prefer the window handle variant (`RawWindowHandle::Wayland` vs
//! `Xlib`/`Xcb`) when you have a window: it is authoritative. This function
//! answers the questions a handle cannot, such as "is this X11 window really
//! running under XWayland?".

/// The display server of the current session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinuxSession {
    /// A Wayland compositor. `xwayland` is `true` when `DISPLAY` is also set,
    /// so X11 clients can run through XWayland.
    Wayland {
        /// X11 clients are possible through XWayland.
        xwayland: bool,
    },
    /// A native X11 server.
    X11,
    /// No graphical session (SSH, a TTY, a CI container without Xvfb).
    Headless,
}

impl LinuxSession {
    /// `true` for a Wayland session (X11 windows there run under XWayland).
    pub fn is_wayland(self) -> bool {
        matches!(self, Self::Wayland { .. })
    }
}

/// Detects the session from environment variables. `env` returns a
/// variable's value, or `None` when unset (inject `|k| std::env::var(k).ok()`
/// in production, a map in tests).
pub fn detect_session(env: impl Fn(&str) -> Option<String>) -> LinuxSession {
    let set = |key: &str| env(key).is_some_and(|v| !v.trim().is_empty());
    let has_x = set("DISPLAY");
    if set("WAYLAND_DISPLAY") || set("WAYLAND_SOCKET") {
        return LinuxSession::Wayland { xwayland: has_x };
    }
    if has_x {
        return LinuxSession::X11;
    }
    match env("XDG_SESSION_TYPE").map(|v| v.trim().to_ascii_lowercase()) {
        Some(kind) if kind == "wayland" => LinuxSession::Wayland { xwayland: false },
        Some(kind) if kind == "x11" => LinuxSession::X11,
        _ => LinuxSession::Headless,
    }
}

/// [`detect_session`] over the real process environment.
pub fn current_session() -> LinuxSession {
    detect_session(|key| std::env::var(key).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn detect(pairs: &[(&str, &str)]) -> LinuxSession {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        detect_session(|k| map.get(k).cloned())
    }

    #[test]
    fn wayland_with_and_without_xwayland() {
        assert_eq!(
            detect(&[
                ("WAYLAND_DISPLAY", "wayland-0"),
                ("DISPLAY", ":0"),
                ("XDG_SESSION_TYPE", "wayland")
            ]),
            LinuxSession::Wayland { xwayland: true }
        );
        assert_eq!(
            detect(&[("WAYLAND_DISPLAY", "wayland-0")]),
            LinuxSession::Wayland { xwayland: false }
        );
        assert_eq!(
            detect(&[("WAYLAND_SOCKET", "3")]),
            LinuxSession::Wayland { xwayland: false }
        );
    }

    #[test]
    fn socket_variables_beat_session_type_like_winit() {
        assert_eq!(
            detect(&[
                ("WAYLAND_DISPLAY", "wayland-1"),
                ("XDG_SESSION_TYPE", "x11")
            ]),
            LinuxSession::Wayland { xwayland: false }
        );
        assert_eq!(
            detect(&[("DISPLAY", ":1"), ("XDG_SESSION_TYPE", "wayland")]),
            LinuxSession::X11,
            "WAYLAND_DISPLAY unset: winit falls back to X11"
        );
    }

    #[test]
    fn plain_x11() {
        assert_eq!(detect(&[("DISPLAY", ":0")]), LinuxSession::X11);
        assert_eq!(detect(&[("XDG_SESSION_TYPE", "X11")]), LinuxSession::X11);
    }

    #[test]
    fn blank_values_count_as_unset() {
        assert_eq!(
            detect(&[("WAYLAND_DISPLAY", " "), ("DISPLAY", "")]),
            LinuxSession::Headless
        );
        assert_eq!(
            detect(&[("XDG_SESSION_TYPE", " wayland ")]),
            LinuxSession::Wayland { xwayland: false }
        );
    }

    #[test]
    fn headless() {
        assert_eq!(detect(&[]), LinuxSession::Headless);
        assert_eq!(
            detect(&[("XDG_SESSION_TYPE", "tty")]),
            LinuxSession::Headless
        );
        assert!(!LinuxSession::Headless.is_wayland());
        assert!(LinuxSession::Wayland { xwayland: true }.is_wayland());
    }

    #[test]
    fn current_session_does_not_panic() {
        let _ = current_session();
    }
}
