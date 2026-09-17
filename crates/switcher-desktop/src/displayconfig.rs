//! Topology changes through `SetDisplayConfig`.
//!
//! The older `ChangeDisplaySettingsEx` detach — hand back a DEVMODE whose size
//! fields are zero — is rejected outright by the driver this was developed
//! against, with `DISP_CHANGE_BADMODE` and no further explanation. Two
//! variants of that recipe were tried and both failed, so attaching and
//! detaching are done here instead, with the API Windows actually maintains.
//!
//! The other reason to prefer it: a path carries the monitor's device
//! interface path, the same `\\?\DISPLAY#...` string the monitor backend uses
//! as its stable id. Matching is therefore exact rather than inferred.

use std::mem::size_of;

use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig, SetDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    QDC_ALL_PATHS, QDC_ONLY_ACTIVE_PATHS, QUERY_DISPLAY_CONFIG_FLAGS, SDC_ALLOW_CHANGES, SDC_APPLY,
    SDC_SAVE_TO_DATABASE, SDC_USE_SUPPLIED_DISPLAY_CONFIG,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};

use crate::DesktopError;

/// Path flag meaning "this monitor is part of the desktop".
const PATH_ACTIVE: u32 = 0x0000_0001;
/// Sentinel for "no mode supplied"; Windows then picks one itself.
const MODE_IDX_INVALID: u32 = 0xffff_ffff;

pub struct DisplayTopology {
    pub paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    pub modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}

fn check(code: WIN32_ERROR, operation: &str) -> Result<(), DesktopError> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(DesktopError::OsCall {
            operation: operation.to_string(),
            detail: format!("Win32 error {}", code.0),
        })
    }
}

/// Read the current topology.
pub fn query(flags: QUERY_DISPLAY_CONFIG_FLAGS) -> Result<DisplayTopology, DesktopError> {
    // The buffers can change size between sizing and reading if a display is
    // hot-plugged in between, so retry a couple of times before giving up.
    for _ in 0..3 {
        let mut path_count = 0u32;
        let mut mode_count = 0u32;
        check(
            unsafe { GetDisplayConfigBufferSizes(flags, &mut path_count, &mut mode_count) },
            "GetDisplayConfigBufferSizes",
        )?;

        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];

        let result = unsafe {
            QueryDisplayConfig(
                flags,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };
        if result == ERROR_SUCCESS {
            paths.truncate(path_count as usize);
            modes.truncate(mode_count as usize);
            return Ok(DisplayTopology { paths, modes });
        }
        // ERROR_INSUFFICIENT_BUFFER (122) means the layout changed underneath.
        if result.0 != 122 {
            check(result, "QueryDisplayConfig")?;
        }
    }
    Err(DesktopError::OsCall {
        operation: "QueryDisplayConfig".into(),
        detail: "the display layout kept changing while it was being read".into(),
    })
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// The monitor device interface path for a path, e.g. `\\?\DISPLAY#AOC2702#...`.
pub fn target_device_path(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut request = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        ..Default::default()
    };
    let ok = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
    (ok == ERROR_SUCCESS.0 as i32).then(|| wide_to_string(&request.monitorDevicePath))
}

/// The GDI name for a path's source, e.g. `\\.\DISPLAY5`.
///
/// Not used by the operations here, which address displays by device path,
/// but kept because it is the only way to correlate a DisplayConfig path back
/// to the GDI world when diagnosing a mismatch.
#[allow(dead_code)]
pub fn source_gdi_name(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut request = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        },
        ..Default::default()
    };
    let ok = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
    (ok == ERROR_SUCCESS.0 as i32).then(|| wide_to_string(&request.viewGdiDeviceName))
}

/// Enumerate every monitor Windows knows about, attached or not.
///
/// This replaced an `EnumDisplayDevices` walk, which reported the *same*
/// device interface path for four different GDI adapters after a topology
/// change — and a detach was consequently aimed at the wrong monitor. Identity
/// has to come from the same API that performs the change, or the two can
/// disagree exactly when it matters.
pub fn enumerate() -> Result<Vec<RawDisplay>, DesktopError> {
    // Two queries, because the PATH_ACTIVE flag in an ALL_PATHS result is not
    // a reliable answer to "is this monitor on the desktop right now". A
    // monitor appears in many candidate paths there, and one of them carrying
    // the flag does not mean the monitor is in use -- which is exactly how a
    // detached display came back reported as attached, and `claim` then
    // refused to reattach it. ONLY_ACTIVE_PATHS is Windows' own answer.
    let active = query(QDC_ONLY_ACTIVE_PATHS)?;
    let all = query(QDC_ALL_PATHS)?;

    let mut out: Vec<RawDisplay> = Vec::new();

    for path in &active.paths {
        let Some(device_path) = target_device_path(path) else {
            continue;
        };
        if device_path.is_empty() || out.iter().any(|d| d.device_path == device_path) {
            continue;
        }
        out.push(raw_from(path, &active, device_path, true));
    }

    // Anything the hardware knows about but is not driving is detached.
    for path in &all.paths {
        let Some(device_path) = target_device_path(path) else {
            continue;
        };
        if device_path.is_empty() || out.iter().any(|d| d.device_path == device_path) {
            continue;
        }
        out.push(raw_from(path, &all, device_path, false));
    }

    Ok(out)
}

/// One monitor as `SetDisplayConfig` sees it.
pub struct RawDisplay {
    pub device_path: String,
    pub gdi_name: String,
    pub friendly_name: Option<String>,
    pub is_attached: bool,
    pub position: (i32, i32),
    pub size: (u32, u32),
}

impl RawDisplay {
    /// The primary display is whichever attached source sits at the origin.
    pub fn is_primary(&self) -> bool {
        self.is_attached && self.position == (0, 0)
    }
}

fn raw_from(
    path: &DISPLAYCONFIG_PATH_INFO,
    topology: &DisplayTopology,
    device_path: String,
    active: bool,
) -> RawDisplay {
    let source = topology.modes.iter().find(|m| {
        m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
            && m.id == path.sourceInfo.id
            && m.adapterId.LowPart == path.sourceInfo.adapterId.LowPart
            && m.adapterId.HighPart == path.sourceInfo.adapterId.HighPart
    });
    let (position, size) = source
        .map(|m| {
            let mode = unsafe { m.Anonymous.sourceMode };
            (
                (mode.position.x, mode.position.y),
                (mode.width, mode.height),
            )
        })
        .unwrap_or(((0, 0), (0, 0)));

    RawDisplay {
        device_path,
        gdi_name: source_gdi_name(path).unwrap_or_default(),
        friendly_name: friendly_name(path).filter(|s| !s.is_empty()),
        is_attached: active,
        position: if active { position } else { (0, 0) },
        size: if active { size } else { (0, 0) },
    }
}

fn friendly_name(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut request = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        ..Default::default()
    };
    let ok = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
    (ok == ERROR_SUCCESS.0 as i32).then(|| wide_to_string(&request.monitorFriendlyDeviceName))
}

fn same_display(candidate: &str, wanted: &str) -> bool {
    let normalise = |s: &str| s.trim().to_ascii_uppercase();
    let a = normalise(candidate);
    let b = normalise(wanted);
    !a.is_empty() && !b.is_empty() && (a.starts_with(&b) || b.starts_with(&a))
}

fn apply(topology: &mut DisplayTopology, operation: &str) -> Result<(), DesktopError> {
    let code = unsafe {
        SetDisplayConfig(
            Some(&topology.paths),
            Some(&topology.modes),
            SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES | SDC_SAVE_TO_DATABASE,
        )
    };
    check(WIN32_ERROR(code as u32), operation)
}

/// Attach or detach one monitor, addressed by its device interface path.
pub fn set_active(device_path: &str, active: bool) -> Result<(), DesktopError> {
    // ALL_PATHS, not ONLY_ACTIVE_PATHS: reattaching means finding a path that
    // is currently inactive, which the active-only query would not return.
    let mut topology = query(QDC_ALL_PATHS)?;

    let index = topology
        .paths
        .iter()
        .position(|p| {
            let is_active = p.flags & PATH_ACTIVE != 0;
            is_active == active_before(active)
                && target_device_path(p)
                    .map(|dp| same_display(&dp, device_path))
                    .unwrap_or(false)
        })
        .ok_or_else(|| DesktopError::DisplayNotFound(device_path.to_string()))?;

    if active {
        topology.paths[index].flags |= PATH_ACTIVE;
        // No mode is supplied for a monitor being brought back, so let
        // Windows choose one rather than inventing a resolution.
        topology.paths[index].sourceInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
        topology.paths[index].targetInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
    } else {
        topology.paths[index].flags &= !PATH_ACTIVE;
        topology.paths[index].sourceInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
        topology.paths[index].targetInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
    }

    let verb = if active { "attaching" } else { "detaching" };
    apply(&mut topology, &format!("{verb} {device_path}"))
}

/// Which activity state the path we are looking for should currently be in.
fn active_before(making_active: bool) -> bool {
    // To activate we need a currently inactive path, and vice versa.
    !making_active
}

/// Make one monitor the primary display.
///
/// The primary is whichever source sits at the origin, so this shifts every
/// source by the same delta rather than setting a flag.
pub fn make_primary(device_path: &str) -> Result<(), DesktopError> {
    let mut topology = query(QDC_ONLY_ACTIVE_PATHS)?;

    let path = topology
        .paths
        .iter()
        .find(|p| {
            target_device_path(p)
                .map(|dp| same_display(&dp, device_path))
                .unwrap_or(false)
        })
        .ok_or_else(|| DesktopError::DisplayNotFound(device_path.to_string()))?;

    let source_id = path.sourceInfo.id;
    let adapter = path.sourceInfo.adapterId;

    // Find that source's current position; everything moves relative to it.
    let (dx, dy) = topology
        .modes
        .iter()
        .find(|m| {
            m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
                && m.id == source_id
                && m.adapterId.LowPart == adapter.LowPart
                && m.adapterId.HighPart == adapter.HighPart
        })
        .map(|m| {
            let pos = unsafe { m.Anonymous.sourceMode.position };
            (-pos.x, -pos.y)
        })
        .ok_or_else(|| DesktopError::OsCall {
            operation: format!("locating the source mode for {device_path}"),
            detail: "the display is active but has no source mode".into(),
        })?;

    if (dx, dy) == (0, 0) {
        return Ok(()); // already at the origin, so already primary
    }

    for mode in topology.modes.iter_mut() {
        if mode.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
            continue;
        }
        unsafe {
            mode.Anonymous.sourceMode.position.x += dx;
            mode.Anonymous.sourceMode.position.y += dy;
        }
    }

    apply(&mut topology, &format!("making {device_path} primary"))
}
