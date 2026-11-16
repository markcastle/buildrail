//! Windows monitor listing. See [`crate::monitors`].

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

use super::{from_rect, last_error};
use crate::geometry::{MonitorId, MonitorInfo};
use crate::monitors::{MonitorError, MonitorSource};
use crate::wide::{from_wide, scale_from_dpi};

/// `EnumDisplayMonitors` callback: appends each monitor to the `Vec` whose
/// address is in `data`.
unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the `&mut Vec<HMONITOR>` passed by `enumerate`,
    // which outlives the synchronous enumeration and is not aliased.
    let list = unsafe { &mut *(data as *mut Vec<HMONITOR>) };
    list.push(monitor);
    1
}

fn enumerate() -> Result<Vec<HMONITOR>, MonitorError> {
    let mut list: Vec<HMONITOR> = Vec::new();
    let data = &mut list as *mut Vec<HMONITOR> as LPARAM;
    // SAFETY: null DC and clip rect mean "all monitors"; `collect` matches
    // MONITORENUMPROC and `data` points at `list`, alive until we return.
    let ok =
        unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect), data) };
    if ok == 0 {
        return Err(MonitorError(format!(
            "EnumDisplayMonitors failed (Win32 error {})",
            last_error()
        )));
    }
    Ok(list)
}

fn monitor_info(monitor: HMONITOR) -> Option<MONITORINFOEXW> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = u32::try_from(std::mem::size_of::<MONITORINFOEXW>()).unwrap_or(0);
    // SAFETY: `info` is a MONITORINFOEXW whose cbSize says so, which is how
    // the API distinguishes it from a plain MONITORINFO (its first field).
    let ok = unsafe {
        GetMonitorInfoW(
            monitor,
            (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>(),
        )
    };
    (ok != 0).then_some(info)
}

fn monitor_dpi(monitor: HMONITOR) -> u32 {
    let (mut x, mut y) = (0_u32, 0_u32);
    // SAFETY: both out-pointers are valid, writable u32s.
    let hr = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) };
    if hr >= 0 {
        x
    } else {
        0
    }
}

/// Lists monitors with `EnumDisplayMonitors`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Win32Monitors;

impl MonitorSource for Win32Monitors {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, MonitorError> {
        Ok(enumerate()?
            .into_iter()
            .filter_map(|handle| {
                let info = monitor_info(handle)?;
                Some(MonitorInfo {
                    id: MonitorId::new(from_wide(&info.szDevice)),
                    bounds: from_rect(&info.monitorInfo.rcMonitor),
                    work_area: from_rect(&info.monitorInfo.rcWork),
                    scale_factor: scale_from_dpi(monitor_dpi(handle)),
                    is_primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                })
            })
            .collect())
    }
}
