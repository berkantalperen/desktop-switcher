//! Windows implementation of layer 2: desktop attachment and window sweeping.
//!
//! Detaching a display is the only operation in this project that can leave
//! someone unable to see anything, and unlike a bad DDC write the monitor's
//! own buttons will not rescue them. So every detach goes through
//! [`crate::detach_is_safe`] first, the previous mode is captured before the
//! change so it can be put back exactly, and the internal laptop panel is
//! never a candidate.

use std::mem::size_of;

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, EnumDisplayDevicesW, EnumDisplaySettingsW, CDS_NORESET, CDS_TYPE,
    CDS_UPDATEREGISTRY, DEVMODEW, DISPLAY_DEVICEW, DISP_CHANGE_SUCCESSFUL, DM_BITSPERPEL,
    DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH, DM_POSITION, ENUM_CURRENT_SETTINGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, GetWindowTextW,
    IsWindowVisible, SetWindowPos, ShowWindow, GWL_EXSTYLE, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOZORDER, SW_MAXIMIZE, SW_RESTORE, WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};

use crate::{
    detach_is_safe, sweep_target, DesktopDisplay, DesktopError, DesktopManager, Rect,
    SavedDisplayMode, SweepReport,
};

// StateFlags bits of DISPLAY_DEVICEW, and the EnumDisplayDevices flag that
// asks for a device interface path rather than a friendly name.
const ATTACHED_TO_DESKTOP: u32 = 0x0000_0001;
const PRIMARY_DEVICE: u32 = 0x0000_0004;
const MIRRORING_DRIVER: u32 = 0x0000_0008;
const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 0x0000_0001;

const SW_SHOWMAXIMIZED_FLAG: u32 = 3;

pub struct WindowsDesktop {
    /// Device paths that must never be detached, supplied by the caller.
    ///
    /// The internal laptop panel is the recovery display, and this crate has
    /// no reliable way to recognise it on its own; the monitor backend does,
    /// because Windows reaches that panel over WMI rather than DDC/CI.
    protected: Vec<String>,
}

impl Default for WindowsDesktop {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsDesktop {
    pub fn new() -> Self {
        Self {
            protected: Vec::new(),
        }
    }

    /// Mark device paths that must never be detached.
    pub fn protecting(paths: Vec<String>) -> Self {
        Self { protected: paths }
    }

    fn is_protected(&self, display: &DesktopDisplay) -> bool {
        self.protected.iter().any(|p| display.matches_backend_id(p))
    }
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn wide_nul(s: &str) -> Vec<u16> {
    s.encoding_utf16_nul()
}

trait EncodeWide {
    fn encoding_utf16_nul(&self) -> Vec<u16>;
}

impl EncodeWide for str {
    fn encoding_utf16_nul(&self) -> Vec<u16> {
        self.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

fn blank_devmode() -> DEVMODEW {
    DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    }
}

/// Read a display's current mode straight from the OS.
fn current_mode(gdi_name: &str) -> Option<DEVMODEW> {
    let name = wide_nul(gdi_name);
    let mut dm = blank_devmode();
    let ok = unsafe {
        EnumDisplaySettingsW(
            PCWSTR(name.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut dm as *mut DEVMODEW,
        )
    };
    ok.as_bool().then_some(dm)
}

fn apply(gdi_name: &str, dm: &DEVMODEW, operation: &str) -> Result<(), DesktopError> {
    let name = wide_nul(gdi_name);
    // CDS_NORESET stages the change; the second call with a null device name
    // is what actually commits the new topology.
    let staged = unsafe {
        ChangeDisplaySettingsExW(
            PCWSTR(name.as_ptr()),
            Some(dm as *const DEVMODEW),
            None,
            CDS_UPDATEREGISTRY | CDS_NORESET,
            None,
        )
    };
    if staged != DISP_CHANGE_SUCCESSFUL {
        return Err(DesktopError::OsCall {
            operation: format!("{operation} (staging {gdi_name})"),
            detail: format!("ChangeDisplaySettingsEx returned {}", staged.0),
        });
    }

    let committed =
        unsafe { ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None) };
    if committed != DISP_CHANGE_SUCCESSFUL {
        return Err(DesktopError::OsCall {
            operation: format!("{operation} (committing)"),
            detail: format!("ChangeDisplaySettingsEx returned {}", committed.0),
        });
    }
    Ok(())
}

impl DesktopManager for WindowsDesktop {
    fn name(&self) -> &str {
        "windows"
    }

    fn displays(&self) -> Result<Vec<DesktopDisplay>, DesktopError> {
        let mut out = Vec::new();
        let mut index = 0u32;

        loop {
            let mut adapter = DISPLAY_DEVICEW {
                cb: size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            let more =
                unsafe { EnumDisplayDevicesW(PCWSTR::null(), index, &mut adapter as *mut _, 0) };
            if !more.as_bool() {
                break;
            }
            index += 1;

            // Mirroring pseudo-devices are not real screens.
            if adapter.StateFlags.0 & MIRRORING_DRIVER != 0 {
                continue;
            }

            let gdi_name = wide_to_string(&adapter.DeviceName);
            if gdi_name.is_empty() {
                continue;
            }

            // The monitor child carries the device interface path, which is
            // what the monitor backend uses as its stable id.
            let mut monitor = DISPLAY_DEVICEW {
                cb: size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            let has_monitor = unsafe {
                EnumDisplayDevicesW(
                    PCWSTR(adapter.DeviceName.as_ptr()),
                    0,
                    &mut monitor as *mut _,
                    EDD_GET_DEVICE_INTERFACE_NAME,
                )
            }
            .as_bool();

            let device_path = if has_monitor {
                wide_to_string(&monitor.DeviceID)
            } else {
                String::new()
            };

            // A GPU output with nothing plugged in still enumerates as an
            // adapter. It has no monitor child and is not on the desktop, so
            // it is not a display anyone can mean.
            if device_path.is_empty() && adapter.StateFlags.0 & ATTACHED_TO_DESKTOP == 0 {
                continue;
            }
            let friendly_name = has_monitor
                .then(|| wide_to_string(&monitor.DeviceString))
                .filter(|s| !s.is_empty());

            let rect = current_mode(&gdi_name)
                .map(|dm| {
                    let pos = unsafe { dm.Anonymous1.Anonymous2.dmPosition };
                    Rect {
                        left: pos.x,
                        top: pos.y,
                        right: pos.x + dm.dmPelsWidth as i32,
                        bottom: pos.y + dm.dmPelsHeight as i32,
                    }
                })
                .unwrap_or_default();

            let mut display = DesktopDisplay {
                gdi_name,
                device_path,
                friendly_name,
                rect,
                is_primary: adapter.StateFlags.0 & PRIMARY_DEVICE != 0,
                is_attached: adapter.StateFlags.0 & ATTACHED_TO_DESKTOP != 0,
                is_internal: false,
            };
            display.is_internal = self.is_protected(&display);
            out.push(display);
        }

        Ok(out)
    }

    fn sweep_windows_off(&self, display: &DesktopDisplay) -> Result<SweepReport, DesktopError> {
        let displays = self.displays()?;
        let Some(target) = sweep_target(&displays, display) else {
            return Err(DesktopError::RefusedUnsafe {
                display: display.gdi_name.clone(),
                reason: "there is no other attached display to move windows onto".into(),
            });
        };

        let mut ctx = SweepCtx {
            source: display.rect,
            target: target.rect,
            report: SweepReport::default(),
        };
        // EnumWindows returns an error when the callback stops early; we
        // never stop early, so a failure here is a real failure.
        unsafe {
            let _ = EnumWindows(
                Some(sweep_callback),
                LPARAM(&mut ctx as *mut SweepCtx as isize),
            );
        }
        Ok(ctx.report)
    }

    fn detach(&self, display: &DesktopDisplay) -> Result<SavedDisplayMode, DesktopError> {
        if self.is_protected(display) {
            return Err(DesktopError::RefusedUnsafe {
                display: display.gdi_name.clone(),
                reason: "it is marked as a protected recovery display".into(),
            });
        }
        let displays = self.displays()?;
        detach_is_safe(&displays, display).map_err(|reason| DesktopError::RefusedUnsafe {
            display: display.gdi_name.clone(),
            reason,
        })?;

        // Capture the exact mode first, so reattaching restores the same
        // resolution, refresh rate and position rather than a guess.
        let dm = current_mode(&display.gdi_name).ok_or_else(|| DesktopError::OsCall {
            operation: format!("reading the current mode of {}", display.gdi_name),
            detail: "EnumDisplaySettings failed".into(),
        })?;
        let pos = unsafe { dm.Anonymous1.Anonymous2.dmPosition };
        let saved = SavedDisplayMode {
            width: dm.dmPelsWidth,
            height: dm.dmPelsHeight,
            pos_x: pos.x,
            pos_y: pos.y,
            refresh_hz: dm.dmDisplayFrequency,
            bits_per_pixel: dm.dmBitsPerPel,
        };

        // A DEVMODE of all zeroes is how Windows is told to drop a display.
        let mut blank = blank_devmode();
        blank.dmFields =
            DM_PELSWIDTH | DM_PELSHEIGHT | DM_POSITION | DM_BITSPERPEL | DM_DISPLAYFREQUENCY;
        apply(&display.gdi_name, &blank, "detaching the display")?;

        Ok(saved)
    }

    fn attach(
        &self,
        display: &DesktopDisplay,
        saved: SavedDisplayMode,
    ) -> Result<(), DesktopError> {
        if saved.width == 0 || saved.height == 0 {
            return Err(DesktopError::NoSavedLayout(display.gdi_name.clone()));
        }
        let mut dm = blank_devmode();
        dm.dmFields =
            DM_PELSWIDTH | DM_PELSHEIGHT | DM_POSITION | DM_BITSPERPEL | DM_DISPLAYFREQUENCY;
        dm.dmPelsWidth = saved.width;
        dm.dmPelsHeight = saved.height;
        dm.dmDisplayFrequency = saved.refresh_hz;
        dm.dmBitsPerPel = saved.bits_per_pixel;
        dm.Anonymous1.Anonymous2.dmPosition.x = saved.pos_x;
        dm.Anonymous1.Anonymous2.dmPosition.y = saved.pos_y;

        apply(&display.gdi_name, &dm, "reattaching the display")
    }
}

struct SweepCtx {
    source: Rect,
    target: Rect,
    report: SweepReport,
}

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if len <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

unsafe extern "system" fn sweep_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut SweepCtx);

    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return TRUE;
    }
    // Tool windows are palettes and overlays, not things a user alt-tabs to.
    let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return TRUE;
    }
    let title = window_title(hwnd);
    if title.is_empty() {
        return TRUE;
    }

    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return TRUE;
    }
    let bounds = Rect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    };
    let (cx, cy) = bounds.center();
    if !ctx.source.contains(cx, cy) {
        return TRUE;
    }

    // A maximized window has to be restored before it can be moved, then
    // maximized again on the new display.
    let mut placement = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    let was_maximized = unsafe { GetWindowPlacement(hwnd, &mut placement) }.is_ok()
        && placement.showCmd == SW_SHOWMAXIMIZED_FLAG;
    if was_maximized {
        let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
        let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
    }

    // Keep the window's offset within the display where it fits, so a sweep
    // does not stack everything in one corner.
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let offset_x = (rect.left - ctx.source.left).max(0);
    let offset_y = (rect.top - ctx.source.top).max(0);
    let new_x = ctx
        .target
        .left
        .saturating_add(offset_x.min((ctx.target.width() - width).max(0)));
    let new_y = ctx
        .target
        .top
        .saturating_add(offset_y.min((ctx.target.height() - height).max(0)));

    let moved = unsafe {
        SetWindowPos(
            hwnd,
            None,
            new_x,
            new_y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        )
    };

    if moved.is_ok() {
        if was_maximized {
            let _ = unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
        }
        ctx.report.moved.push(title);
    } else {
        ctx.report.skipped.push(title);
    }
    TRUE
}
