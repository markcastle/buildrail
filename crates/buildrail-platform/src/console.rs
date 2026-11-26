//! Showing `--help`, `--version` and start-up errors in the terminal the
//! user started BuildRail from.
//!
//! A Windows release build is a GUI program (`windows_subsystem =
//! "windows"`), so it starts without a console and anything it prints goes
//! nowhere. [`attach_to_parent`] attaches the process to the console of
//! the program that started it, if there is one: run from PowerShell or
//! `cmd`, output appears there; double-clicked, there is no parent console
//! and nothing is opened. Elsewhere the standard streams already work and
//! this does nothing.

/// Attaches to the parent process's console (Windows); returns `true` when
/// it did. Call it first thing in `main`, before anything prints. A no-op
/// returning `false` on other systems, when there is no parent console, or
/// when the process already has one (a debug build).
pub fn attach_to_parent() -> bool {
    #[cfg(windows)]
    {
        crate::win32::console::attach_to_parent()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn attaching_never_fails_loudly() {
        // The test runner already has a console (or none): either way the
        // call returns without effect on the test's output.
        let _ = super::attach_to_parent();
    }
}
