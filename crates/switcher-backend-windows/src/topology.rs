//! Which physical monitor a display is, decided before any DDC is attempted.
//!
//! Everything here is pure. It works on the topology Windows reported and
//! decides whether, and how, a display can be addressed. The unsafe calls live
//! in `ddc`, so the rules that keep a write off the wrong monitor are tested on
//! every platform.

use switcher_core::backend::BackendError;
use switcher_core::edid::EdidInfo;
use switcher_core::mccs;
use switcher_core::types::{
    InputCapabilities, InputCode, InputReading, InputSourceOption, MonitorIdentity, Transport,
};

/// VCP feature 0x60, Input Source.
pub const INPUT_SOURCE: u8 = 0x60;

/// What Windows reports when a monitor answers but has no such VCP feature.
/// `ERROR_GRAPHICS_DDCCI_VCP_NOT_SUPPORTED`.
pub const VCP_NOT_SUPPORTED: u32 = 0xC026_2584;

/// One display as Windows' display topology reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyEntry {
    /// Device interface path, e.g. `\\?\DISPLAY#AOC2702#4&34b9e9a7&0&UID4165#{e6f0...}`.
    pub device_path: String,
    /// GDI source name, e.g. `\\.\DISPLAY2`. The only way to reach a
    /// display's physical monitor through the Monitor Configuration API.
    pub gdi_name: String,
    /// The built-in laptop panel, which has no inputs to switch.
    pub internal: bool,
    /// Part of the desktop. A detached display has no GDI source, so there is
    /// nothing to address it through.
    pub attached: bool,
}

/// The id this backend gives a display: its device path without the
/// interface GUID.
///
/// It is the format the PowerToys backend used, deliberately, so a
/// configuration written against that backend keeps binding unchanged.
pub fn backend_id(device_path: &str) -> String {
    let trimmed = device_path.trim();
    match trimmed.rfind("#{") {
        Some(i) => trimmed[..i].to_string(),
        None => trimmed.to_string(),
    }
}

fn same_display(device_path: &str, id: &str) -> bool {
    let a = backend_id(device_path);
    let b = backend_id(id);
    // Exact, not a prefix match: `...UID4165` must never match `...UID41650`.
    !a.is_empty() && a.eq_ignore_ascii_case(&b)
}

/// Why a display cannot be written to right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing Windows reports has this id.
    Absent,
    /// More than one display has this id, and they cannot be told apart.
    Ambiguous(usize),
    /// The built-in laptop panel.
    Internal,
    /// Connected, but not part of the desktop.
    Detached,
    /// The desktop mirrors this display with others, so they share one GDI
    /// source. Windows returns their physical monitors in an order nothing
    /// ties to a display, and picking one would be a guess.
    Mirrored(usize),
}

impl Refusal {
    pub fn into_error(self, id: &str) -> BackendError {
        match self {
            Refusal::Absent => BackendError::MonitorNotFound {
                detail: format!("nothing Windows reports right now has the id `{id}`"),
            },
            Refusal::Ambiguous(n) => BackendError::MonitorNotFound {
                detail: format!(
                    "{n} displays share the id `{id}`, and there is no way to tell which is \
                     which; refusing to guess"
                ),
            },
            Refusal::Internal => BackendError::Unsupported {
                detail: format!("`{id}` is the built-in panel, which has no inputs to switch"),
            },
            Refusal::Detached => BackendError::MonitorNotFound {
                detail: format!(
                    "`{id}` is connected but not part of the desktop, so Windows offers no way \
                     to talk to it. Turn it back on in Settings > System > Display."
                ),
            },
            Refusal::Mirrored(n) => BackendError::Unsupported {
                detail: format!(
                    "`{id}` is mirrored with {} other display(s). Windows returns mirrored \
                     monitors in an order nothing ties to a particular display, so a write \
                     could land on the wrong one. Set the displays to Extend in \
                     Settings > System > Display.",
                    n - 1
                ),
            },
        }
    }
}

impl Refusal {
    /// Whether waiting could change the answer.
    ///
    /// Right after a monitor changes input, Windows spends a second or two
    /// re-detecting its displays, and in that window the topology is briefly
    /// wrong: a panel missing, detached, or two panels shown as mirrored. The
    /// same answers from a settled desktop are real. An id shared by two
    /// displays, or the built-in panel, is structural and will not change.
    pub fn may_be_transient(&self) -> bool {
        matches!(
            self,
            Refusal::Absent | Refusal::Detached | Refusal::Mirrored(_)
        )
    }
}

/// Keep asking until the answer is not a transient refusal, or time runs out.
///
/// `sleep` is injected so the rule can be tested without waiting. Whatever is
/// finally returned was true at the moment it was returned; this never turns
/// a refusal into permission, it only declines to give up early.
pub fn settle<T>(
    attempts: usize,
    mut probe: impl FnMut() -> Result<T, Refusal>,
    mut sleep: impl FnMut(),
) -> Result<T, Refusal> {
    let mut last = probe();
    for _ in 1..attempts.max(1) {
        match &last {
            Err(r) if r.may_be_transient() => {
                sleep();
                last = probe();
            }
            _ => break,
        }
    }
    last
}

/// The display with this id, if it can be addressed unambiguously.
pub fn locate<'a>(entries: &'a [TopologyEntry], id: &str) -> Result<&'a TopologyEntry, Refusal> {
    let matches: Vec<&TopologyEntry> = entries
        .iter()
        .filter(|e| same_display(&e.device_path, id))
        .collect();
    let entry = match matches.as_slice() {
        [] => return Err(Refusal::Absent),
        [one] => *one,
        many => {
            // The same display listed twice, once attached and once not, is
            // one display. Anything else is a genuine ambiguity.
            let attached: Vec<&&TopologyEntry> = many.iter().filter(|e| e.attached).collect();
            match attached.as_slice() {
                [one] => **one,
                _ => return Err(Refusal::Ambiguous(many.len())),
            }
        }
    };

    if entry.internal {
        return Err(Refusal::Internal);
    }
    if !entry.attached || entry.gdi_name.is_empty() {
        return Err(Refusal::Detached);
    }
    let sharing = entries
        .iter()
        .filter(|e| e.attached && e.gdi_name.eq_ignore_ascii_case(&entry.gdi_name))
        .count();
    if sharing > 1 {
        return Err(Refusal::Mirrored(sharing));
    }
    Ok(entry)
}

/// Identity for a display, from the topology and the EDID Windows cached.
///
/// Nothing here talks to the monitor. Whether a monitor answers a particular
/// DDC request is reported on that request; it never decides whether the
/// monitor is listed. The backend this replaced hid any monitor whose
/// capabilities it could not read, and a cable that carries every short
/// command a switch needs could not carry that one long transfer.
pub fn identity_for(entry: &TopologyEntry, index: u32, edid: Option<&EdidInfo>) -> MonitorIdentity {
    MonitorIdentity {
        backend_id: backend_id(&entry.device_path),
        discovery_index: Some(index),
        manufacturer: edid.and_then(|e| e.manufacturer.clone()),
        model: edid.and_then(|e| e.model_name.clone()),
        serial: edid.and_then(EdidInfo::best_serial),
        transport: if entry.internal {
            Transport::Internal
        } else {
            Transport::DdcCi
        },
    }
}

/// Turn a VCP 0x60 reply into a reading.
///
/// The input code is the low byte. A reply of zero is not an input, and is
/// reported as a failed read rather than invented into one.
pub fn reading_from_reply(current: u32) -> InputReading {
    match (current & 0xFF) as u8 {
        0 => InputReading::ReadFailed {
            detail: format!(
                "the monitor answered 0x{current:04X} for its input, which is not an input code"
            ),
        },
        code => InputReading::Value(InputCode(code)),
    }
}

/// What a monitor's capabilities string claims about feature 0x60.
pub fn capabilities_from(caps: &str) -> Result<InputCapabilities, String> {
    let features = mccs::parse_vcp_features(caps).map_err(|e| e.to_string())?;
    let options: Vec<InputSourceOption> = features
        .get(&INPUT_SOURCE)
        .map(|codes| {
            codes
                .iter()
                .map(|&c| InputSourceOption {
                    code: InputCode(c),
                    label: InputCode(c).standard_label().map(String::from),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(InputCapabilities {
        feature_present: features.contains_key(&INPUT_SOURCE),
        options,
        raw_capabilities: Some(caps.to_string()),
        raw_output: caps.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: &str = "\\\\?\\DISPLAY#AOC2702#4&34b9e9a7&0&UID4165";
    const CENTER: &str = "\\\\?\\DISPLAY#AOC2702#4&34b9e9a7&0&UID8517";
    const BUILT_IN: &str = "\\\\?\\DISPLAY#BOE0CBF#4&34b9e9a7&0&UID8388688";
    const GUID: &str = "#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";

    /// Captured from one of the AOC panels this was written against.
    const AOC_CAPS: &str = "(vcp(02 04 05 08 10 12 14(01 05 06 08 0B) 16 18 1A 52 60(01 03 11 0F ) 62 86(02 05) C8 C9 CC(01 02 03 04 05 06 07 09 0A 0B 0C 0D 0E 12 14 16 1E) B6 DF C6 DC(00 0B 0C 0D 0E 0F 10) D6(01 04) ED F8)prot(monitor)type(LCD)cmds(01 02 03 07 0C F3)mccs_ver(2.2)asset_eep(64)mpu_ver(005)model(27P2Q)mswhql(1))";

    fn entry(path: &str, gdi: &str) -> TopologyEntry {
        TopologyEntry {
            device_path: format!("{path}{GUID}"),
            gdi_name: gdi.into(),
            internal: false,
            attached: true,
        }
    }

    fn desk() -> Vec<TopologyEntry> {
        let mut built_in = entry(BUILT_IN, "\\\\.\\DISPLAY1");
        built_in.internal = true;
        vec![
            built_in,
            entry(LEFT, "\\\\.\\DISPLAY2"),
            entry(CENTER, "\\\\.\\DISPLAY3"),
        ]
    }

    #[test]
    fn the_id_is_the_device_path_without_its_interface_guid() {
        assert_eq!(backend_id(&format!("{LEFT}{GUID}")), LEFT);
        assert_eq!(backend_id(LEFT), LEFT);
    }

    #[test]
    fn a_display_is_found_by_the_id_an_existing_configuration_holds() {
        let desk = desk();
        let found = locate(&desk, LEFT).unwrap();
        assert_eq!(found.gdi_name, "\\\\.\\DISPLAY2");
        // Case and the trailing GUID do not matter; either form addresses it.
        assert!(locate(&desk, &LEFT.to_lowercase()).is_ok());
        assert!(locate(&desk, &format!("{CENTER}{GUID}")).is_ok());
    }

    #[test]
    fn a_longer_id_that_merely_starts_the_same_is_a_different_display() {
        let desk = desk();
        assert_eq!(
            locate(&desk, &format!("{LEFT}0")).unwrap_err(),
            Refusal::Absent
        );
    }

    #[test]
    fn the_built_in_panel_is_never_a_target() {
        assert_eq!(locate(&desk(), BUILT_IN).unwrap_err(), Refusal::Internal);
    }

    #[test]
    fn a_detached_display_has_nothing_to_address_it_through() {
        let mut desk = desk();
        desk[1].attached = false;
        assert_eq!(locate(&desk, LEFT).unwrap_err(), Refusal::Detached);
    }

    /// Mirroring puts two displays behind one GDI source, and nothing Windows
    /// returns says which physical monitor is which. This project's own
    /// display commands have left the panels mirrored before.
    #[test]
    fn mirrored_displays_are_refused_rather_than_guessed() {
        let mut desk = desk();
        desk[2].gdi_name = desk[1].gdi_name.clone();
        assert_eq!(locate(&desk, LEFT).unwrap_err(), Refusal::Mirrored(2));
        assert_eq!(locate(&desk, CENTER).unwrap_err(), Refusal::Mirrored(2));
    }

    #[test]
    fn the_same_display_listed_attached_and_detached_is_one_display() {
        let mut desk = desk();
        let mut stale = desk[1].clone();
        stale.attached = false;
        stale.gdi_name.clear();
        desk.push(stale);
        assert!(locate(&desk, LEFT).is_ok());
    }

    #[test]
    fn two_attached_displays_with_one_id_are_ambiguous() {
        let mut desk = desk();
        let mut twin = desk[1].clone();
        twin.gdi_name = "\\\\.\\DISPLAY9".into();
        desk.push(twin);
        assert_eq!(locate(&desk, LEFT).unwrap_err(), Refusal::Ambiguous(2));
    }

    #[test]
    fn identity_comes_from_the_topology_and_the_cached_edid() {
        let edid = EdidInfo {
            manufacturer: Some("AOC".into()),
            model_name: Some("27P2DG5".into()),
            serial_text: Some("ASFPA9A001109".into()),
            ..Default::default()
        };
        let id = identity_for(&desk()[1], 2, Some(&edid));
        assert_eq!(id.backend_id, LEFT);
        assert_eq!(id.serial.as_deref(), Some("ASFPA9A001109"));
        assert_eq!(id.model.as_deref(), Some("27P2DG5"));
        assert_eq!(id.transport, Transport::DdcCi);
    }

    /// The rule the whole backend exists for: being listed does not depend on
    /// answering. A display with no EDID at all is still a display.
    #[test]
    fn a_display_is_listed_without_anything_being_asked_of_it() {
        let id = identity_for(&desk()[1], 2, None);
        assert_eq!(id.transport, Transport::DdcCi);
        assert_eq!(id.serial, None);
    }

    #[test]
    fn the_built_in_panel_is_identified_as_such() {
        let id = identity_for(&desk()[0], 1, None);
        assert_eq!(id.transport, Transport::Internal);
        assert!(!id.transport.supports_input_switching());
    }

    #[test]
    fn a_reply_reads_as_its_low_byte() {
        assert_eq!(
            reading_from_reply(0x11),
            InputReading::Value(InputCode(0x11))
        );
        assert_eq!(
            reading_from_reply(0x010F),
            InputReading::Value(InputCode(0x0F))
        );
    }

    #[test]
    fn a_zero_reply_is_a_failed_read_not_an_input() {
        assert!(matches!(
            reading_from_reply(0),
            InputReading::ReadFailed { .. }
        ));
    }

    #[test]
    fn real_capabilities_yield_the_advertised_inputs() {
        let caps = capabilities_from(AOC_CAPS).unwrap();
        assert!(caps.feature_present);
        let codes: Vec<u8> = caps.options.iter().map(|o| o.code.0).collect();
        assert_eq!(codes, vec![0x01, 0x03, 0x11, 0x0F]);
        assert_eq!(caps.options[2].label.as_deref(), Some("HDMI-1"));
    }

    #[test]
    fn capabilities_without_feature_60_say_so() {
        let caps = capabilities_from("(vcp(02 10 12)type(LCD))").unwrap();
        assert!(!caps.feature_present);
        assert!(caps.options.is_empty());
    }

    #[test]
    fn truncated_capabilities_are_an_error_not_an_empty_list() {
        assert!(capabilities_from("(vcp(02 10 60(01 03").is_err());
    }

    /// Seen on hardware: the second step of an action ran while Windows was
    /// re-detecting after the first, found the two panels briefly mirrored,
    /// and gave up. A second later the desktop was extended again.
    #[test]
    fn a_reshuffle_is_waited_out_rather_than_refused() {
        let mut script = vec![
            Err(Refusal::Mirrored(2)),
            Err(Refusal::Absent),
            Ok("addressable"),
        ]
        .into_iter();
        let mut slept = 0;
        let got = settle(10, || script.next().unwrap(), || slept += 1);
        assert_eq!(got, Ok("addressable"));
        assert_eq!(slept, 2);
    }

    #[test]
    fn a_desktop_that_stays_mirrored_is_still_refused() {
        let mut slept = 0;
        let got: Result<(), _> = settle(5, || Err(Refusal::Mirrored(2)), || slept += 1);
        assert_eq!(got, Err(Refusal::Mirrored(2)));
        assert_eq!(slept, 4, "gives up after the attempts it was given");
    }

    /// Waiting cannot make the built-in panel switchable or two identical ids
    /// distinguishable, so those answer at once.
    #[test]
    fn a_structural_refusal_is_not_waited_on() {
        for refusal in [Refusal::Internal, Refusal::Ambiguous(2)] {
            let mut slept = 0;
            let got: Result<(), _> = settle(5, || Err(refusal.clone()), || slept += 1);
            assert_eq!(got, Err(refusal));
            assert_eq!(slept, 0);
        }
    }

    #[test]
    fn a_settled_desktop_costs_no_waiting() {
        let mut slept = 0;
        let got = settle(5, || Ok::<_, Refusal>(1), || slept += 1);
        assert_eq!(got, Ok(1));
        assert_eq!(slept, 0);
    }

    #[test]
    fn every_refusal_names_the_display() {
        for refusal in [
            Refusal::Absent,
            Refusal::Ambiguous(2),
            Refusal::Internal,
            Refusal::Detached,
            Refusal::Mirrored(2),
        ] {
            assert!(refusal
                .into_error("some-id")
                .to_string()
                .contains("some-id"));
        }
    }
}
