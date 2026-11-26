//! Single instance through a named mutex. See [`crate::single_instance`].

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;

use super::last_error;
use crate::single_instance::{InstanceError, InstanceGuard, SingleInstance};
use crate::wide::to_wide;

/// A named mutex in the session's `Local\` namespace: one per signed-in
/// user and session, released by Windows when the process ends (even if it
/// is killed).
#[derive(Debug, Clone, Copy, Default)]
pub struct NamedMutexInstance;

/// The mutex handle, as an integer so the guard can be `Send`.
struct OwnedMutex(isize);

impl Drop for OwnedMutex {
    fn drop(&mut self) {
        close(self.0 as HANDLE);
    }
}

fn close(handle: HANDLE) {
    // SAFETY: `handle` came from `CreateMutexW` and is closed exactly once.
    unsafe {
        CloseHandle(handle);
    }
}

impl SingleInstance for NamedMutexInstance {
    fn acquire(&self, name: &str) -> Result<InstanceGuard, InstanceError> {
        let full = format!(
            "Local\\BuildRail.{}",
            crate::single_instance::lock_name(name)
        );
        let wide = to_wide(&full);
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the
        // call; null security attributes mean the default ones.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
        let error = last_error();
        if handle.is_null() {
            return Err(InstanceError::Io(format!("CreateMutexW failed ({error})")));
        }
        if error == i64::from(ERROR_ALREADY_EXISTS) {
            close(handle);
            return Err(InstanceError::AlreadyRunning);
        }
        Ok(InstanceGuard::new(OwnedMutex(handle as isize)))
    }
}
