//! Reads a display's EDID out of the Windows device registry.
//!
//! Windows names a display by a DevicePath such as
//! `\\?\DISPLAY#AOC2702#5&23e00778&0&UID4613`. The same components address the
//! device's registry key, where Windows caches the EDID block the monitor sent
//! during handshake. Reading it needs no elevation.

use switcher_core::edid::{self, EdidInfo};
use winreg::enums::HKEY_LOCAL_MACHINE;
use winreg::RegKey;

/// Translate a DevicePath into the `Device Parameters` key that holds its EDID.
///
/// Returns `None` for anything that is not a `DISPLAY` device path, rather
/// than building a registry path out of unvalidated input.
fn registry_subpath(device_path: &str) -> Option<String> {
    let trimmed = device_path
        .trim_start_matches("\\\\?\\")
        .trim_start_matches("\\\\.\\");
    let mut parts = trimmed.split('#');

    let enumerator = parts.next()?;
    if !enumerator.eq_ignore_ascii_case("DISPLAY") {
        return None;
    }
    let hardware_id = parts.next().filter(|s| !s.is_empty())?;
    let instance_id = parts.next().filter(|s| !s.is_empty())?;

    // Path components come from the OS, but validate anyway: a `\` or `..`
    // here would escape the key we mean to read.
    for part in [hardware_id, instance_id] {
        if part.contains('\\') || part.contains('/') || part.contains("..") {
            return None;
        }
    }

    Some(format!(
        "SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{hardware_id}\\{instance_id}\\Device Parameters"
    ))
}

/// Best-effort EDID lookup. A failure here degrades identification to the
/// connection id; it is never fatal, and never fabricates a serial.
pub fn read_for_device_path(device_path: &str) -> Option<EdidInfo> {
    let subpath = registry_subpath(device_path)?;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm.open_subkey(subpath).ok()?;
    let raw = key.get_raw_value("EDID").ok()?;
    edid::parse(&raw.bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_the_registry_path_from_a_real_device_path() {
        let path = registry_subpath("\\\\?\\DISPLAY#AOC2702#5&23e00778&0&UID4613").unwrap();
        assert_eq!(
            path,
            "SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\AOC2702\\5&23e00778&0&UID4613\\Device Parameters"
        );
    }

    #[test]
    fn ignores_a_trailing_interface_guid() {
        let path = registry_subpath(
            "\\\\?\\DISPLAY#AOC2702#5&23e00778&0&UID4613#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}",
        )
        .unwrap();
        assert!(path.ends_with("5&23e00778&0&UID4613\\Device Parameters"));
    }

    #[test]
    fn refuses_non_display_and_malformed_paths() {
        assert!(registry_subpath("\\\\?\\USB#VID_046D#abc").is_none());
        assert!(registry_subpath("DISPLAY#AOC2702").is_none());
        assert!(registry_subpath("").is_none());
    }

    #[test]
    fn refuses_path_components_that_would_escape_the_key() {
        assert!(registry_subpath("\\\\?\\DISPLAY#..#..\\..\\Something").is_none());
        assert!(registry_subpath("\\\\?\\DISPLAY#AOC2702#..").is_none());
    }

    /// Runs against the live registry. Asserts only on shape, so it stays
    /// meaningful on a machine with different monitors attached.
    #[test]
    fn reads_something_sane_from_the_live_registry_if_a_display_is_attached() {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let Ok(display_root) = hklm.open_subkey("SYSTEM\\CurrentControlSet\\Enum\\DISPLAY") else {
            return;
        };
        for hardware_id in display_root.enum_keys().flatten() {
            let Ok(hw_key) = display_root.open_subkey(&hardware_id) else {
                continue;
            };
            for instance in hw_key.enum_keys().flatten() {
                let device_path = format!("\\\\?\\DISPLAY#{hardware_id}#{instance}");
                if let Some(info) = read_for_device_path(&device_path) {
                    assert_eq!(
                        info.manufacturer.as_ref().map(|m| m.len()),
                        Some(3),
                        "PNP id should be three letters: {info:?}"
                    );
                    return;
                }
            }
        }
    }
}
