//! `AttachConsole(ATTACH_PARENT_PROCESS)`. See [`crate::console`].

use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

/// Attaches to the parent's console; `true` on success.
pub(crate) fn attach_to_parent() -> bool {
    // SAFETY: no pointers are passed. The call fails harmlessly (returns 0)
    // when the parent has no console or this process already has one.
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 }
}
