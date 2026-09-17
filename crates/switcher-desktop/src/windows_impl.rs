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
    ChangeDisplaySettingsExW, EnumDisplaySettingsW, CDS_NORESET, CDS_TYPE, CDS_UPDATEREGISTRY,
    DEVMODEW, DISP_CHANGE_SUCCESSFUL, DM_BITSPERPEL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT,
    DM_PELSWIDTH, DM_POSITION, ENUM_CURRENT_SETTINGS,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, GetWindowTextW,
    IsWindowVisible, SetWindowPos, ShowWindow, GWL_EXSTYLE, SWP_NOACTIVATE, SWP_NOSIZE,
    SWP_NOZORDER, SW_MAXIMIZE, SW_RESTORE, WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};

use crate::displayconfig;
use crate::{
    detach_is_safe, sweep_target, DesktopDisplay, DesktopError, DesktopManager, MovedWindow, Rect,
    RestoreReport, SavedDisplayMode, SweepReport,
};

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

/// Opt into per-monitor DPI awareness, once per process.
///
/// Without this, a scaled display reports different geometry to GDI than to
/// the window functions — the laptop panel here is 2560x1600 physically but
/// 1707x1067 virtualised at 150%. Mixing the two coordinate spaces is how a
/// window gets "moved" to a position that is still on the display it started
/// on. Failure is ignored: the value cannot be changed once a process has
/// drawn, and we are no worse off than before.
fn ensure_dpi_aware() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    });
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

/// Turn a DISP_CHANGE_* return code into something a human can act on.
fn disp_change_reason(code: i32) -> &'static str {
    match code {
        0 => "successful",
        1 => "the change needs a restart to take effect",
        -1 => "the display driver rejected the request (DISP_CHANGE_FAILED)",
        -2 => {
            "the mode was rejected (DISP_CHANGE_BADMODE); for a detach this usually means \
               the display is the primary one, which Windows will not detach"
        }
        -3 => "the settings could not be written to the registry (DISP_CHANGE_NOTUPDATED)",
        -4 => "invalid flags (DISP_CHANGE_BADFLAGS)",
        -5 => "invalid parameters (DISP_CHANGE_BADPARAM)",
        -6 => "the change conflicts with a dual-view setup (DISP_CHANGE_BADDUALVIEW)",
        _ => "an unrecognised status was returned",
    }
}

/// Stage a change for one display without applying it yet.
///
/// Promotion has to move every display in the same commit, because the
/// primary defines the origin and Windows will reject an arrangement that is
/// inconsistent halfway through.
fn stage(
    gdi_name: &str,
    dm: &DEVMODEW,
    extra_flags: CDS_TYPE,
    operation: &str,
) -> Result<(), DesktopError> {
    let name = wide_nul(gdi_name);
    let staged = unsafe {
        ChangeDisplaySettingsExW(
            PCWSTR(name.as_ptr()),
            Some(dm as *const DEVMODEW),
            None,
            CDS_UPDATEREGISTRY | CDS_NORESET | extra_flags,
            None,
        )
    };
    if staged != DISP_CHANGE_SUCCESSFUL {
        return Err(DesktopError::OsCall {
            operation: format!("{operation} (staging {gdi_name})"),
            detail: format!(
                "ChangeDisplaySettingsEx returned {}: {}",
                staged.0,
                disp_change_reason(staged.0)
            ),
        });
    }
    Ok(())
}

/// Apply everything staged so far.
fn commit(operation: &str) -> Result<(), DesktopError> {
    let committed =
        unsafe { ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None) };
    if committed != DISP_CHANGE_SUCCESSFUL {
        return Err(DesktopError::OsCall {
            operation: format!("{operation} (committing)"),
            detail: format!(
                "ChangeDisplaySettingsEx returned {}: {}",
                committed.0,
                disp_change_reason(committed.0)
            ),
        });
    }
    Ok(())
}

fn apply(gdi_name: &str, dm: &DEVMODEW, operation: &str) -> Result<(), DesktopError> {
    stage(gdi_name, dm, CDS_TYPE(0), operation)?;
    commit(operation)
}

impl DesktopManager for WindowsDesktop {
    fn name(&self) -> &str {
        "windows"
    }

    fn displays(&self) -> Result<Vec<DesktopDisplay>, DesktopError> {
        ensure_dpi_aware();
        Ok(displayconfig::enumerate()?
            .into_iter()
            .map(|raw| {
                let mut display = DesktopDisplay {
                    rect: Rect {
                        left: raw.position.0,
                        top: raw.position.1,
                        right: raw.position.0 + raw.size.0 as i32,
                        bottom: raw.position.1 + raw.size.1 as i32,
                    },
                    is_primary: raw.is_primary(),
                    is_attached: raw.is_attached,
                    gdi_name: raw.gdi_name,
                    device_path: raw.device_path,
                    friendly_name: raw.friendly_name,
                    is_internal: false,
                };
                display.is_internal = self.is_protected(&display);
                display
            })
            .collect())
    }

    fn sweep_windows_off(&self, display: &DesktopDisplay) -> Result<SweepReport, DesktopError> {
        ensure_dpi_aware();
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

    fn restore_windows(&self, windows: &[MovedWindow]) -> Result<RestoreReport, DesktopError> {
        ensure_dpi_aware();
        if windows.is_empty() {
            return Ok(RestoreReport::default());
        }
        let mut ctx = RestoreCtx {
            wanted: windows.to_vec(),
            done: Vec::new(),
        };
        unsafe {
            let _ = EnumWindows(
                Some(restore_callback),
                LPARAM(&mut ctx as *mut RestoreCtx as isize),
            );
        }
        let missing = windows
            .iter()
            .filter(|w| !ctx.done.contains(&w.title))
            .map(|w| w.title.clone())
            .collect();
        Ok(RestoreReport {
            restored: ctx.done,
            missing,
        })
    }

    fn set_primary(&self, display: &DesktopDisplay) -> Result<(), DesktopError> {
        ensure_dpi_aware();
        if display.is_primary {
            return Ok(());
        }
        if !display.is_attached {
            return Err(DesktopError::RefusedUnsafe {
                display: display.gdi_name.clone(),
                reason: "it is not attached, so it cannot be made primary".into(),
            });
        }
        if display.device_path.is_empty() {
            return Err(DesktopError::DisplayNotFound(display.gdi_name.clone()));
        }
        displayconfig::make_primary(&display.device_path)
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

        // Detaching goes through SetDisplayConfig. The older
        // ChangeDisplaySettingsEx recipe -- hand back a DEVMODE whose size
        // fields are zero -- is rejected by this driver with
        // DISP_CHANGE_BADMODE, in both the fully-zeroed and
        // derived-from-live-mode variants.
        if display.device_path.is_empty() {
            return Err(DesktopError::DisplayNotFound(display.gdi_name.clone()));
        }
        displayconfig::set_active(&display.device_path, false)?;

        Ok(saved)
    }

    fn attach(
        &self,
        display: &DesktopDisplay,
        saved: SavedDisplayMode,
    ) -> Result<(), DesktopError> {
        if display.device_path.is_empty() {
            return Err(DesktopError::DisplayNotFound(display.gdi_name.clone()));
        }

        // Bring it back into the desktop first; Windows picks a mode for it.
        displayconfig::set_active(&display.device_path, true)?;

        // Then put it back where it was. This is a best effort: the display
        // is usable either way, and refusing to report success because it
        // landed a few hundred pixels off would be worse than saying so.
        if saved.width == 0 || saved.height == 0 {
            return Ok(());
        }
        let mut dm = current_mode(&display.gdi_name).unwrap_or_else(blank_devmode);
        dm.dmFields =
            DM_PELSWIDTH | DM_PELSHEIGHT | DM_POSITION | DM_BITSPERPEL | DM_DISPLAYFREQUENCY;
        dm.dmPelsWidth = saved.width;
        dm.dmPelsHeight = saved.height;
        dm.dmDisplayFrequency = saved.refresh_hz;
        dm.dmBitsPerPel = saved.bits_per_pixel;
        dm.Anonymous1.Anonymous2.dmPosition.x = saved.pos_x;
        dm.Anonymous1.Anonymous2.dmPosition.y = saved.pos_y;

        if let Err(e) = apply(&display.gdi_name, &dm, "restoring the previous layout") {
            eprintln!(
                "warning: `{}` is attached again but could not be put back at {}x{} ({}, {}): {e}",
                display.gdi_name, saved.width, saved.height, saved.pos_x, saved.pos_y
            );
        }
        Ok(())
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
    // Remembered before anything is changed, so a restore can undo exactly
    // this move rather than an approximation of it.
    let original = bounds;

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

    // Synchronous, not SWP_ASYNCWINDOWPOS: the move has to have happened
    // before the rect is re-read below, and before a maximized window is
    // re-maximized — otherwise it maximizes on the display it started on.
    let requested = unsafe {
        SetWindowPos(
            hwnd,
            None,
            new_x,
            new_y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };

    // A successful SetWindowPos does not mean the window moved. Windows
    // belonging to an elevated process, and some that manage their own
    // placement, silently stay put. Re-read the position and believe that
    // instead of the return value.
    let mut after = RECT::default();
    let landed = requested.is_ok()
        && unsafe { GetWindowRect(hwnd, &mut after) }.is_ok()
        && !ctx.source.contains(
            after.left + (after.right - after.left) / 2,
            after.top + (after.bottom - after.top) / 2,
        );

    if was_maximized {
        let _ = unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
    }
    if landed {
        ctx.report.moved.push(MovedWindow {
            title,
            from: original,
            was_maximized,
        });
    } else {
        ctx.report.skipped.push(title);
    }
    TRUE
}

/// Move one window back to a remembered position, matched by title.
unsafe extern "system" fn restore_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut RestoreCtx);

    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return TRUE;
    }
    let title = window_title(hwnd);
    if title.is_empty() {
        return TRUE;
    }
    let Some(index) = ctx
        .wanted
        .iter()
        .position(|w| w.title == title && !ctx.done.contains(&w.title))
    else {
        return TRUE;
    };
    let wanted = ctx.wanted[index].clone();

    // A maximized window has to be restored before it can be placed.
    let mut placement = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    let is_maximized = unsafe { GetWindowPlacement(hwnd, &mut placement) }.is_ok()
        && placement.showCmd == SW_SHOWMAXIMIZED_FLAG;
    if is_maximized {
        let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
    }

    let moved = unsafe {
        SetWindowPos(
            hwnd,
            None,
            wanted.from.left,
            wanted.from.top,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    if wanted.was_maximized {
        let _ = unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
    }

    if moved.is_ok() {
        ctx.done.push(wanted.title);
    }
    TRUE
}

struct RestoreCtx {
    wanted: Vec<MovedWindow>,
    done: Vec<String>,
}
