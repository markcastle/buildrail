//! The per-user Run key. Logic: [`crate::autostart::RunKeyAutostart`].

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};

use crate::autostart::{AutostartError, RunKeyStore, RUN_KEY_PATH};
use crate::wide::{from_wide, to_wide};

fn registry_error(operation: &str, code: WIN32_ERROR) -> AutostartError {
    AutostartError::Registry(format!("{operation} failed (Win32 error {code})"))
}

/// Reads value `name` into `buffer`; `size` is in bytes, in and out.
fn get_value(key: &[u16], name: &[u16], buffer: Option<&mut [u16]>, size: &mut u32) -> WIN32_ERROR {
    let data = buffer.map_or(std::ptr::null_mut(), |b| b.as_mut_ptr().cast());
    // SAFETY: `key`/`name` are NUL-terminated; `data` is null (size query)
    // or a buffer of at least `*size` bytes; `size` is a valid u32.
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            data,
            size,
        )
    }
}

/// `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CurrentUserRunKey;

impl RunKeyStore for CurrentUserRunKey {
    fn get(&self, name: &str) -> Result<Option<String>, AutostartError> {
        let (key, name) = (to_wide(RUN_KEY_PATH), to_wide(name));
        let mut size = 0_u32;
        match get_value(&key, &name, None, &mut size) {
            ERROR_SUCCESS => {}
            ERROR_FILE_NOT_FOUND => return Ok(None),
            code => return Err(registry_error("RegGetValueW", code)),
        }
        let mut buffer = vec![0_u16; usize::try_from(size).unwrap_or(0) / 2 + 1];
        match get_value(&key, &name, Some(&mut buffer), &mut size) {
            ERROR_SUCCESS => Ok(Some(from_wide(&buffer))),
            ERROR_FILE_NOT_FOUND => Ok(None),
            code => Err(registry_error("RegGetValueW", code)),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), AutostartError> {
        let (key, name, value) = (to_wide(RUN_KEY_PATH), to_wide(name), to_wide(value));
        let bytes = u32::try_from(value.len() * 2)
            .map_err(|_| AutostartError::InvalidCommand("command line too long".into()))?;
        // SAFETY: all strings are NUL-terminated and alive for the call;
        // `bytes` is the exact byte length of `value` including the NUL.
        let code = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr().cast(),
                bytes,
            )
        };
        match code {
            ERROR_SUCCESS => Ok(()),
            code => Err(registry_error("RegSetKeyValueW", code)),
        }
    }

    fn delete(&self, name: &str) -> Result<(), AutostartError> {
        let (key, name) = (to_wide(RUN_KEY_PATH), to_wide(name));
        // SAFETY: both strings are NUL-terminated and alive for the call.
        let code = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        match code {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            code => Err(registry_error("RegDeleteKeyValueW", code)),
        }
    }
}
