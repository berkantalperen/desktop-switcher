//! Parsers for `ddcutil` output.
//!
//! **Status: written against ddcutil's documented output shapes, not yet
//! validated against this hardware.** The fixtures in
//! `tests/fixtures/ddcutil/` are synthetic until `scripts/ubuntu-preflight.sh`
//! has run on the HP Z4 and its real output replaces them. Until then, treat
//! every claim this module makes about the AOC panels as unverified.
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
            "DRM connector" => display.drm_connector = Some(value.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    // SYNTHETIC fixtures, shaped from ddcutil's documented output. Replace
    // with real capture from the HP Z4 before trusting anything here.
    const DETECT: &str = include_str!("../../../tests/fixtures/ddcutil/detect.synthetic.txt");
    const CAPS: &str = include_str!("../../../tests/fixtures/ddcutil/capabilities.synthetic.txt");

    #[test]
    fn parses_two_panels_with_distinct_serials() {
        let displays = parse_detect(DETECT).unwrap();
        let valid: Vec<&DetectedDisplay> = displays.iter().filter(|d| !d.invalid).collect();
        assert_eq!(valid.len(), 2);
        assert_eq!(valid[0].serial.as_deref(), Some("ASFPA9A001108"));
        assert_eq!(valid[1].serial.as_deref(), Some("ASFPA9A001109"));
        assert_eq!(valid[0].manufacturer.as_deref(), Some("AOC"));
        assert_eq!(valid[0].model.as_deref(), Some("27P2Q"));
    }

    #[test]
    fn prefers_the_drm_connector_as_the_stable_id() {
        let displays = parse_detect(DETECT).unwrap();
        assert_eq!(displays[0].stable_id(), "card1-DP-1");
    }

    #[test]
    fn an_invalid_display_is_flagged_not_silently_dropped() {
        let displays = parse_detect(DETECT).unwrap();
        assert!(displays.iter().any(|d| d.invalid));
    }

    #[test]
    fn empty_detect_output_is_an_error() {
        assert!(matches!(
            parse_detect("").unwrap_err(),
            ParseError::NoDisplays(_)
        ));
    }

    #[test]
    fn parses_the_terse_getvcp_form() {
        assert_eq!(
            parse_getvcp_input("VCP 60 SNC x0f\n").unwrap(),
            InputCode(0x0F)
        );
        assert_eq!(
            parse_getvcp_input("VCP 60 SNC x11\n").unwrap(),
            InputCode(0x11)
        );
    }

    #[test]
    fn parses_the_verbose_getvcp_form() {
        let text = "VCP code 0x60 (Input Source                  ): DisplayPort-1 (sl=0x0f)\n";
        assert_eq!(parse_getvcp_input(text).unwrap(), InputCode(0x0F));
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
    fn parses_capabilities_values() {
        let caps = parse_capabilities(CAPS).unwrap();
        assert!(caps.feature_present);
        let codes: Vec<u8> = caps.options.iter().map(|(c, _)| c.get()).collect();
        assert_eq!(codes, vec![0x01, 0x03, 0x0F, 0x11]);
        assert_eq!(caps.model.as_deref(), Some("27P2Q"));
        assert_eq!(caps.mccs_version.as_deref(), Some("2.2"));
    }

    #[test]
    fn values_of_other_features_are_not_read_as_input_sources() {
        let text = "\
VCP Features:
   Feature: 14 (Select color preset)
      Values:
         01: sRGB
         05: 6500K
   Feature: 60 (Input Source)
      Values:
         0f: DisplayPort-1
   Feature: 86 (Display Scaling)
      Values:
         02: Scale to fit
";
        let caps = parse_capabilities(text).unwrap();
        let codes: Vec<u8> = caps.options.iter().map(|(c, _)| c.get()).collect();
        assert_eq!(codes, vec![0x0F]);
    }

    #[test]
    fn a_monitor_without_feature_60_reports_absence_rather_than_an_empty_success() {
        let text = "Model: X\nVCP Features:\n   Feature: 10 (Brightness)\n";
        let caps = parse_capabilities(text).unwrap();
        assert!(!caps.feature_present);
        assert!(caps.options.is_empty());
    }
}
