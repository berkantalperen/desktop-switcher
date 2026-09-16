//! Parsers for `ddcutil` output.
//!
//! Validated against **ddcutil 2.2.5** on the HP Z4 (Ubuntu 26.04, NVIDIA
//! Quadro M2000). The fixtures in `tests/fixtures/ddcutil/` are verbatim
//! capture from that machine, with stdout, stderr and exit status recorded
//! separately.
//!
//! Where ddcutil offers a `--terse` form, the terse form is parsed in
//! preference: it is the machine-oriented output and changes far less often
//! than the human-readable layout.

use switcher_core::types::InputCode;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("`ddcutil detect` produced no display blocks: {0:?}")]
    NoDisplays(String),
    #[error("could not read a VCP value from {0:?}")]
    BadVcpValue(String),
    #[error("`getvcp 60` output did not mention feature 0x60: {0:?}")]
    NotInputSource(String),
    /// ddcutil answered, and the monitor reported the feature as in error.
    /// Distinct from a parse failure: the tool worked, the feature did not.
    #[error("the monitor reported feature 0x{0:02X} as unsupported")]
    FeatureError(u8),
    #[error("could not parse capabilities output: {0}")]
    Capabilities(String),
}

/// One display block from `ddcutil detect`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DetectedDisplay {
    /// The `Display N` number. A discovery-time address, never identity.
    pub display_number: Option<u32>,
    /// e.g. `/dev/i2c-5`.
    pub i2c_bus: Option<String>,
    /// e.g. `card1-DP-1`. The closest ddcutil analogue to a stable port id.
    pub drm_connector: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    /// True for blocks ddcutil reported as `Invalid display`, which are
    /// present on the bus but cannot carry DDC.
    pub invalid: bool,
    pub raw: String,
}

impl DetectedDisplay {
    /// The most stable handle available for this display.
    pub fn stable_id(&self) -> String {
        self.drm_connector
            .clone()
            .or_else(|| self.i2c_bus.clone())
            .unwrap_or_else(|| {
                format!(
                    "display-{}",
                    self.display_number
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "unknown".into())
                )
            })
    }
}

/// Parse `ddcutil detect` (with or without `--verbose`).
pub fn parse_detect(stdout: &str) -> Result<Vec<DetectedDisplay>, ParseError> {
    let mut displays: Vec<DetectedDisplay> = Vec::new();
    let mut current: Option<DetectedDisplay> = None;

    for line in stdout.lines() {
        let trimmed = line.trim();

        // A new block starts at column zero.
        let starts_block = !line.starts_with(char::is_whitespace) && !trimmed.is_empty();
        if starts_block {
            if let Some(display) = current.take() {
                displays.push(display);
            }
            if let Some(rest) = trimmed.strip_prefix("Display ") {
                current = Some(DetectedDisplay {
                    display_number: rest.trim().parse().ok(),
                    raw: format!("{line}\n"),
                    ..Default::default()
                });
            } else if trimmed.starts_with("Invalid display") {
                current = Some(DetectedDisplay {
                    invalid: true,
                    raw: format!("{line}\n"),
                    ..Default::default()
                });
            }
            // Any other column-zero line (banners, warnings) closes the block.
            continue;
        }

        let Some(display) = current.as_mut() else {
            continue;
        };
        display.raw.push_str(line);
        display.raw.push('\n');

        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "I2C bus" => display.i2c_bus = Some(value.to_string()),
            // 2.2.5 prints `DRM_connector`; older releases and the docs use a
            // space. Accept both rather than depending on which is installed.
            "DRM connector" | "DRM_connector" => display.drm_connector = Some(value.to_string()),
            // `AOC - (Unknown)` or just `AOC`: keep only the PNP id.
            "Mfg id" => {
                let id = value.split_whitespace().next().unwrap_or(value);
                display.manufacturer = Some(id.trim_end_matches('-').trim().to_string());
            }
            "Model" => display.model = Some(value.to_string()),
            "Serial number" => display.serial = Some(value.to_string()),
            _ => {}
        }

        if trimmed.contains("does not support DDC") {
            display.invalid = true;
        }
    }
    if let Some(display) = current.take() {
        displays.push(display);
    }

    if displays.is_empty() {
        return Err(ParseError::NoDisplays(stdout.to_string()));
    }
    Ok(displays)
}

/// Parse `getvcp 60`, in either the terse or the verbose form.
///
/// Terse:   `VCP 60 SNC x0f`
/// Verbose: `VCP code 0x60 (Input Source                  ): DisplayPort-1 (sl=0x0f)`
pub fn parse_getvcp_input(stdout: &str) -> Result<InputCode, ParseError> {
    for line in stdout.lines() {
        let trimmed = line.trim();

        // Terse form.
        if let Some(rest) = trimmed.strip_prefix("VCP 60 ") {
            // `VCP 60 ERR` means the monitor rejected the feature. ddcutil
            // exits non-zero, but the distinction still has to survive, or an
            // unsupported feature reads as a mangled value.
            if rest.trim() == "ERR" {
                return Err(ParseError::FeatureError(0x60));
            }
            if let Some(token) = rest.split_whitespace().last() {
                if let Some(hex) = token.strip_prefix('x') {
                    return hex_to_code(hex, trimmed);
                }
            }
            return Err(ParseError::BadVcpValue(trimmed.to_string()));
        }

        // Verbose form: pull the value out of `sl=0xNN`.
        if trimmed.starts_with("VCP code 0x60") {
            let Some(at) = trimmed.find("sl=0x") else {
                return Err(ParseError::BadVcpValue(trimmed.to_string()));
            };
            let hex: String = trimmed[at + 5..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect();
            return hex_to_code(&hex, trimmed);
        }
    }
    Err(ParseError::NotInputSource(stdout.to_string()))
}

fn hex_to_code(hex: &str, context: &str) -> Result<InputCode, ParseError> {
    u8::from_str_radix(hex, 16)
        .map(InputCode)
        .map_err(|_| ParseError::BadVcpValue(context.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedCapabilities {
    pub feature_present: bool,
    pub options: Vec<(InputCode, String)>,
    pub model: Option<String>,
    pub mccs_version: Option<String>,
}

/// Parse the human-readable `ddcutil capabilities` output.
///
/// The `--terse` form returns the raw MCCS string instead, which should be
/// parsed with [`switcher_core::mccs`]; that parser is the more trustworthy of
/// the two, and the backend cross-checks them.
pub fn parse_capabilities(stdout: &str) -> Result<ParsedCapabilities, ParseError> {
    let mut parsed = ParsedCapabilities::default();
    let mut in_feature_60 = false;
    let mut in_values = false;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("Model:") {
            parsed.model = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("MCCS version:") {
            parsed.mccs_version = Some(rest.trim().to_string());
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("Feature:") {
            // Any new feature ends the 0x60 block.
            let code = rest.split_whitespace().next().unwrap_or("");
            in_feature_60 = code.eq_ignore_ascii_case("60");
            in_values = false;
            if in_feature_60 {
                parsed.feature_present = true;
            }
            continue;
        }

        if in_feature_60 {
            if trimmed.starts_with("Values:") {
                in_values = true;
                continue;
            }
            if in_values {
                // `01: VGA-1`
                let Some((code_text, label)) = trimmed.split_once(':') else {
                    continue;
                };
                let code_text = code_text.trim();
                if code_text.len() > 2 || !code_text.chars().all(|c| c.is_ascii_hexdigit()) {
                    continue;
                }
                let code = hex_to_code(code_text, trimmed)?;
                parsed.options.push((code, label.trim().to_string()));
            }
        }
    }

    Ok(parsed)
}

/// Pull the raw MCCS capability string out of `capabilities --terse`.
///
/// ddcutil 2.2.5 prefixes it with `Unparsed capabilities string: `, so the
/// line cannot simply be trimmed and handed to the MCCS parser. Anything that
/// is not a `(...)` blob is rejected rather than passed along.
pub fn extract_terse_capability_string(stdout: &str) -> Option<&str> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        let candidate = match trimmed.strip_prefix("Unparsed capabilities string:") {
            Some(rest) => rest.trim(),
            None if trimmed.starts_with('(') => trimmed,
            None => continue,
        };
        if candidate.starts_with('(') && candidate.ends_with(')') {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim capture from ddcutil 2.2.5 on the HP Z4, 2026-09-16.
    const DETECT: &str = include_str!("../../../tests/fixtures/ddcutil/detect.stdout.txt");
    const CAPS: &str = include_str!("../../../tests/fixtures/ddcutil/capabilities.stdout.txt");
    const CAPS_TERSE: &str =
        include_str!("../../../tests/fixtures/ddcutil/capabilities-terse.stdout.txt");
    const GETVCP: &str = include_str!("../../../tests/fixtures/ddcutil/getvcp60.stdout.txt");
    const GETVCP_TERSE: &str =
        include_str!("../../../tests/fixtures/ddcutil/getvcp60-terse.stdout.txt");
    const ERR_UNSUPPORTED: &str =
        include_str!("../../../tests/fixtures/ddcutil/error-unsupported-feature.stdout.txt");

    #[test]
    fn parses_the_real_detect_output() {
        let displays = parse_detect(DETECT).unwrap();
        assert_eq!(displays.len(), 1, "only one panel is cabled to the Z4");
        let d = &displays[0];
        assert!(!d.invalid);
        assert_eq!(d.display_number, Some(1));
        assert_eq!(d.i2c_bus.as_deref(), Some("/dev/i2c-5"));
        assert_eq!(d.drm_connector.as_deref(), Some("card1-DP-4"));
        assert_eq!(d.model.as_deref(), Some("27P2DG5"));
        assert_eq!(d.manufacturer.as_deref(), Some("AOC"));
    }

    /// The whole cross-host design rests on this: the serial ddcutil reports
    /// must be the one Windows reports, or a monitor cannot be matched
    /// between the two computers.
    #[test]
    fn the_serial_matches_the_one_windows_reports() {
        let displays = parse_detect(DETECT).unwrap();
        assert_eq!(displays[0].serial.as_deref(), Some("ASFPA9A001108"));
    }

    #[test]
    fn handles_the_2_2_5_underscore_spelling_of_drm_connector() {
        // 2.2.5 prints `DRM_connector`; the docs use a space. Both must work,
        // because which one appears depends on the installed release.
        assert!(DETECT.contains("DRM_connector:"));
        let spaced = DETECT.replace("DRM_connector:", "DRM connector:");
        assert_eq!(
            parse_detect(&spaced).unwrap()[0].drm_connector.as_deref(),
            Some("card1-DP-4")
        );
    }

    #[test]
    fn prefers_the_drm_connector_as_the_stable_id() {
        let displays = parse_detect(DETECT).unwrap();
        assert_eq!(displays[0].stable_id(), "card1-DP-4");
    }

    #[test]
    fn falls_back_through_bus_then_number_when_no_connector_is_reported() {
        let no_connector = DETECT.replace("DRM_connector:           card1-DP-4", "");
        assert_eq!(
            parse_detect(&no_connector).unwrap()[0].stable_id(),
            "/dev/i2c-5"
        );

        let bare = "Display 4\n   VCP version:  2.2\n";
        assert_eq!(parse_detect(bare).unwrap()[0].stable_id(), "display-4");
    }

    /// Constructed, not captured: only one panel is currently cabled to the
    /// Z4, so a second block cannot be observed on this hardware yet.
    #[test]
    fn separates_multiple_displays_and_flags_invalid_ones() {
        let two = format!(
            "{}\n{}\nInvalid display\n   I2C bus:  /dev/i2c-3\n   Monitor does not support DDC\n",
            DETECT.trim_end(),
            DETECT
                .trim_end()
                .replace("Display 1", "Display 2")
                .replace("/dev/i2c-5", "/dev/i2c-6")
                .replace("card1-DP-4", "card1-DP-1")
                .replace("ASFPA9A001108", "ASFPA9A001109")
        );
        let displays = parse_detect(&two).unwrap();
        assert_eq!(displays.len(), 3);
        assert_eq!(displays[0].serial.as_deref(), Some("ASFPA9A001108"));
        assert_eq!(displays[1].serial.as_deref(), Some("ASFPA9A001109"));
        assert!(displays[2].invalid, "a non-DDC display must be flagged");
    }

    #[test]
    fn empty_detect_output_is_an_error() {
        assert!(matches!(
            parse_detect("").unwrap_err(),
            ParseError::NoDisplays(_)
        ));
    }

    #[test]
    fn parses_the_real_terse_getvcp() {
        assert_eq!(parse_getvcp_input(GETVCP_TERSE).unwrap(), InputCode(0x11));
    }

    #[test]
    fn parses_the_real_verbose_getvcp() {
        assert_eq!(parse_getvcp_input(GETVCP).unwrap(), InputCode(0x11));
    }

    /// The Z4 reads 0x11 — the *Windows* input — while Windows is driving
    /// that monitor over HDMI. DDC stays reachable from the computer that is
    /// not currently displayed, which is what makes switching work at all.
    #[test]
    fn both_getvcp_forms_agree_with_each_other() {
        assert_eq!(
            parse_getvcp_input(GETVCP).unwrap(),
            parse_getvcp_input(GETVCP_TERSE).unwrap()
        );
    }

    #[test]
    fn a_feature_error_is_distinct_from_a_parse_failure() {
        // `VCP AA ERR`: ddcutil worked, the monitor rejected the feature.
        assert!(matches!(
            parse_getvcp_input(&ERR_UNSUPPORTED.replace("AA", "60")).unwrap_err(),
            ParseError::FeatureError(0x60)
        ));
    }

    #[test]
    fn a_reading_for_another_feature_is_not_mistaken_for_the_input() {
        let text = "VCP code 0x10 (Brightness                    ): current value =    75\n";
        assert!(matches!(
            parse_getvcp_input(text).unwrap_err(),
            ParseError::NotInputSource(_)
        ));
    }

    #[test]
    fn a_malformed_value_is_an_error_not_a_default() {
        assert!(parse_getvcp_input("VCP 60 SNC xZZ\n").is_err());
        assert!(parse_getvcp_input("VCP code 0x60 (Input Source): unknown\n").is_err());
    }

    #[test]
    fn parses_the_real_capabilities_output() {
        let caps = parse_capabilities(CAPS).unwrap();
        assert!(caps.feature_present);
        let codes: Vec<u8> = caps.options.iter().map(|(c, _)| c.get()).collect();
        // Note the order the monitor actually reports: not sorted.
        assert_eq!(codes, vec![0x01, 0x03, 0x11, 0x0F]);
        assert_eq!(caps.model.as_deref(), Some("27P2Q"));
        assert_eq!(caps.mccs_version.as_deref(), Some("2.2"));
    }

    #[test]
    fn values_of_other_features_are_not_read_as_input_sources() {
        // The real output carries value lists on 0x14, 0x86, 0xCC, 0xDC and
        // 0xD6, several of which contain bytes that are also valid input
        // codes. Only the four under feature 0x60 may come back.
        let caps = parse_capabilities(CAPS).unwrap();
        assert_eq!(caps.options.len(), 4);
        assert!(caps.options.iter().all(|(_, label)| !label.is_empty()));
    }

    #[test]
    fn extracts_the_raw_capability_string_despite_the_2_2_5_prefix() {
        let raw = extract_terse_capability_string(CAPS_TERSE).unwrap();
        assert!(raw.starts_with("(vcp("), "{raw}");
        assert!(raw.ends_with(')'));
        assert!(!raw.contains("Unparsed"));
    }

    /// Two independent readings of the same claim, from two different ddcutil
    /// output modes. A disagreement means one of the parsers is wrong.
    #[test]
    fn the_human_readable_and_raw_capability_readings_agree() {
        let raw = extract_terse_capability_string(CAPS_TERSE).unwrap();
        let from_raw = switcher_core::mccs::input_source_values(raw)
            .unwrap()
            .unwrap();
        let from_table: Vec<u8> = parse_capabilities(CAPS)
            .unwrap()
            .options
            .iter()
            .map(|(c, _)| c.get())
            .collect();
        assert_eq!(from_raw, from_table);
    }

    /// And the same panel, read from Windows through an entirely different
    /// tool, reports a byte-identical capability string.
    #[test]
    fn the_capability_string_matches_the_one_powertoys_reports() {
        let from_linux = extract_terse_capability_string(CAPS_TERSE).unwrap();
        let from_windows = "(vcp(02 04 05 08 10 12 14(01 05 06 08 0B) 16 18 1A 52 60(01 03 11 0F ) 62 86(02 05) C8 C9 CC(01 02 03 04 05 06 07 09 0A 0B 0C 0D 0E 12 14 16 1E) B6 DF C6 DC(00 0B 0C 0D 0E 0F 10) D6(01 04) ED F8)prot(monitor)type(LCD)cmds(01 02 03 07 0C F3)mccs_ver(2.2)asset_eep(64)mpu_ver(005)model(27P2Q)mswhql(1))";
        assert_eq!(from_linux, from_windows);
    }

    #[test]
    fn rejects_terse_output_that_is_not_a_capability_string() {
        assert!(extract_terse_capability_string("Display not found\n").is_none());
        assert!(extract_terse_capability_string("Unparsed capabilities string: broken").is_none());
        assert!(extract_terse_capability_string("").is_none());
    }

    #[test]
    fn a_monitor_without_feature_60_reports_absence_rather_than_an_empty_success() {
        let text = "Model: X\nVCP Features:\n   Feature: 10 (Brightness)\n";
        let caps = parse_capabilities(text).unwrap();
        assert!(!caps.feature_present);
        assert!(caps.options.is_empty());
    }
}
