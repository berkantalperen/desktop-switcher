//! The unsafe edge: Windows' Monitor Configuration API (`dxva2.dll`).
//!
//! Deliberately thin. Every decision about *which* monitor to talk to is made
//! in `topology` before anything here runs; this module only turns a GDI
//! source name into a physical-monitor handle and exchanges VCP values with it.

use std::mem::size_of;

use windows::core::{BOOL, HRESULT};
use windows::Win32::Devices::Display::{
    CapabilitiesRequestAndCapabilitiesReply, DestroyPhysicalMonitors, GetCapabilitiesStringLength,
    GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
    GetVCPFeatureAndVCPFeatureReply, SetVCPFeature, PHYSICAL_MONITOR,
};
use windows::Win32::Foundation::{GetLastError, HANDLE, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};

/// A failed exchange, with the Windows error code kept for the caller.
#[derive(Debug, Clone)]
pub struct DdcError {
    pub code: u32,
    pub detail: String,
}

impl std::fmt::Display for DdcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

fn last_error(what: &str) -> DdcError {
    let code = unsafe { GetLastError() }.0;
    let message = HRESULT(code as i32).message();
    let message = message.trim();
    DdcError {
        code,
        detail: if message.is_empty() {
            format!("{what} failed (Windows error 0x{code:08X})")
        } else {
            format!("{what} failed: {message} (0x{code:08X})")
        },
    }
}

/// The physical monitors behind one GDI source. Destroyed on drop, because
/// Windows leaks the handles otherwise.
pub struct PhysicalMonitors(Vec<PHYSICAL_MONITOR>);

impl PhysicalMonitors {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The only physical monitor, when there is exactly one.
    pub fn only(&self) -> Option<HANDLE> {
        match self.0.as_slice() {
            [one] => Some(one.hPhysicalMonitor),
            _ => None,
        }
    }
}

impl Drop for PhysicalMonitors {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            let _ = unsafe { DestroyPhysicalMonitors(&self.0) };
        }
    }
}

/// Open the physical monitors behind a GDI source such as `\\.\DISPLAY2`.
pub fn physical_monitors_for(gdi_name: &str) -> Result<PhysicalMonitors, DdcError> {
    let hmonitor = find_hmonitor(gdi_name).ok_or_else(|| DdcError {
        code: 0,
        detail: format!("Windows has no monitor handle for {gdi_name}"),
    })?;

    let mut count = 0u32;
    unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hmonitor, &mut count) }
        .map_err(|_| last_error("counting the physical monitors"))?;
    let mut monitors = vec![PHYSICAL_MONITOR::default(); count as usize];
    if count > 0 {
        unsafe { GetPhysicalMonitorsFromHMONITOR(hmonitor, &mut monitors) }
            .map_err(|_| last_error("opening the physical monitor"))?;
    }
    Ok(PhysicalMonitors(monitors))
}

struct Search {
    want: String,
    found: Option<HMONITOR>,
}

unsafe extern "system" fn search_callback(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let search = unsafe { &mut *(data.0 as *mut Search) };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(hmonitor, &mut info as *mut _ as *mut MONITORINFO) };
    if ok.as_bool() {
        let end = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        let name = String::from_utf16_lossy(&info.szDevice[..end]);
        if name.eq_ignore_ascii_case(&search.want) {
            search.found = Some(hmonitor);
            return BOOL(0);
        }
    }
    BOOL(1)
}

fn find_hmonitor(gdi_name: &str) -> Option<HMONITOR> {
    let mut search = Search {
        want: gdi_name.to_string(),
        found: None,
    };
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(search_callback),
            LPARAM(&mut search as *mut Search as isize),
        );
    }
    search.found
}

/// Current value of a VCP feature.
pub fn get_vcp(monitor: HANDLE, code: u8) -> Result<u32, DdcError> {
    let mut current = 0u32;
    let mut maximum = 0u32;
    let ok = unsafe {
        GetVCPFeatureAndVCPFeatureReply(monitor, code, None, &mut current, Some(&mut maximum))
    };
    if ok == 0 {
        return Err(last_error("reading from the monitor"));
    }
    Ok(current)
}

/// Write a VCP feature.
pub fn set_vcp(monitor: HANDLE, code: u8, value: u32) -> Result<(), DdcError> {
    let ok = unsafe { SetVCPFeature(monitor, code, value) };
    if ok == 0 {
        return Err(last_error("writing to the monitor"));
    }
    Ok(())
}

/// The monitor's MCCS capabilities string.
///
/// A long, multi-part transfer: the one exchange most likely to fail over a
/// marginal cable or adapter while short commands still work.
pub fn capabilities(monitor: HANDLE) -> Result<String, DdcError> {
    let mut length = 0u32;
    if unsafe { GetCapabilitiesStringLength(monitor, &mut length) } == 0 {
        return Err(last_error("asking the monitor for its capabilities"));
    }
    if length == 0 {
        return Err(DdcError {
            code: 0,
            detail: "the monitor returned an empty capabilities string".into(),
        });
    }
    let mut buffer = vec![0u8; length as usize];
    if unsafe { CapabilitiesRequestAndCapabilitiesReply(monitor, &mut buffer) } == 0 {
        return Err(last_error("reading the monitor's capabilities"));
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    Ok(String::from_utf8_lossy(&buffer[..end]).into_owned())
}
