//! Parsers for `PowerToys.PowerDisplay.Cli.exe` output.
//!
//! Every parser here is strict. If the text does not look like what the tested
//! version prints, it returns an error rather than a best guess, because the
//! consequence of a misparse is writing an input code to the wrong display.
//!
//! Tests run against fixtures captured verbatim from the installed CLI; see
//! `tests/fixtures/powertoys/`.

use switcher_core::types::InputCode;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("unrecognised `list` header: {0:?}. This build was tested against PowerToys {tested}; the CLI output format may have changed.", tested = crate::TESTED_VERSION)]
    ListHeader(String),
    #[error("could not parse `list` row {row:?}: {detail}")]
    ListRow { row: String, detail: String },
    #[error("`get` output did not start with a `Monitor N` header: {0:?}")]
    MissingMonitorHeader(String),
    #[error("could not read an input code from {0:?}")]
    BadInputCode(String),
    #[error("capabilities output contained no `VCP codes:` section")]
    MissingVcpSection,
}

/// One row of `PowerToys.PowerDisplay.Cli.exe list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    pub number: u32,
    /// Blank for the internal panel on the tested version.
    pub name: Option<String>,
    /// `DDC/CI` or `WMI`.
    pub method: String,
    /// DevicePath-derived stable id.
    pub id: String,
    pub raw: String,
}

const LIST_COLUMNS: [&str; 4] = ["#", "Name", "Method", "Monitor ID"];

pub fn parse_list(stdout: &str) -> Result<Vec<ListRow>, ParseError> {
    let mut lines = stdout.lines().filter(|l| !l.trim().is_empty());

    let header = lines
        .next()
        .ok_or_else(|| ParseError::ListHeader(String::new()))?;
    let header_cols: Vec<&str> = header.split('|').map(str::trim).collect();
    if header_cols != LIST_COLUMNS {
        return Err(ParseError::ListHeader(header.to_string()));
    }

    let mut rows = Vec::new();
    for line in lines {
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        if cols.len() != 4 {
            return Err(ParseError::ListRow {
                row: line.to_string(),
                detail: format!("expected 4 columns, found {}", cols.len()),
            });
        }
        let number = cols[0].parse::<u32>().map_err(|e| ParseError::ListRow {
            row: line.to_string(),
            detail: format!("monitor number: {e}"),
        })?;
        if cols[3].is_empty() {
            return Err(ParseError::ListRow {
                row: line.to_string(),
                detail: "monitor id is empty".into(),
            });
        }
        rows.push(ListRow {
            number,
            name: (!cols[1].is_empty()).then(|| cols[1].to_string()),
            method: cols[2].to_string(),
            id: cols[3].to_string(),
            raw: line.to_string(),
        });
    }
    Ok(rows)
}

/// Parsed `capabilities --setting input-source` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCapabilities {
    pub feature_present: bool,
    pub options: Vec<(InputCode, String)>,
    /// The `Raw:` MCCS string, when the tool printed one.
    pub raw_capabilities: Option<String>,
    pub model: Option<String>,
}

pub fn parse_capabilities(stdout: &str) -> Result<ParsedCapabilities, ParseError> {
    let mut options = Vec::new();
    let mut raw_capabilities = None;
    let mut model = None;
    let mut feature_present = false;
    let mut saw_vcp_section = false;

    for line in stdout.lines() {
        let t = line.trim();
        if t == "VCP codes:" {
            saw_vcp_section = true;
        } else if let Some(rest) = t.strip_prefix("Model:") {
            model = Some(rest.trim().to_string());
        } else if let Some(rest) = t.strip_prefix("Raw:") {
            raw_capabilities = Some(rest.trim().to_string());
        } else if let Some(rest) = t.strip_prefix("0x60 Input Source:") {
            feature_present = true;
            for entry in rest.split(',') {
                let entry = entry.trim();
                if entry.is_empty() {
                    continue;
                }
                let (label, code) = split_label_and_code(entry)
                    .ok_or_else(|| ParseError::BadInputCode(entry.to_string()))?;
                options.push((code, label));
            }
        }
    }

    if !saw_vcp_section {
        return Err(ParseError::MissingVcpSection);
    }
    Ok(ParsedCapabilities {
        feature_present,
        options,
        raw_capabilities,
        model,
    })
}

/// Split `HDMI-1 (0x11)` into its label and code.
fn split_label_and_code(entry: &str) -> Option<(String, InputCode)> {
    let open = entry.rfind('(')?;
    let close = entry.rfind(')')?;
    if close < open {
        return None;
    }
    let code: InputCode = entry[open + 1..close].trim().parse().ok()?;
    Some((entry[..open].trim().to_string(), code))
}

/// Parsed `get --setting input-source` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedGet {
    pub id: Option<String>,
    pub protocol: Option<String>,
    /// `None` means the tool printed no `input-source` line at all, i.e. the
    /// monitor does not expose the setting. That is not the same as a failure.
    pub input_source: Option<InputCode>,
}

pub fn parse_get(stdout: &str) -> Result<ParsedGet, ParseError> {
    let mut lines = stdout.lines().filter(|l| !l.trim().is_empty());
    let header = lines
        .next()
        .ok_or_else(|| ParseError::MissingMonitorHeader(String::new()))?;
    if !header.trim_start().starts_with("Monitor ") {
        return Err(ParseError::MissingMonitorHeader(header.to_string()));
    }

    let mut parsed = ParsedGet {
        id: None,
        protocol: None,
        input_source: None,
    };
    for line in lines {
        let Some((key, value)) = split_key_value(line) else {
            continue;
        };
        match key.as_str() {
            "id" => parsed.id = Some(value),
            "protocol" => parsed.protocol = Some(value),
            "input-source" => {
                let code = split_label_and_code(&value)
                    .map(|(_, c)| c)
                    // Tolerate a bare code with no label.
                    .or_else(|| value.trim().parse().ok())
                    .ok_or_else(|| ParseError::BadInputCode(value.clone()))?;
                parsed.input_source = Some(code);
            }
            _ => {}
        }
    }
    Ok(parsed)
}

/// Split an indented `key      value` line on its run of two or more spaces.
fn split_key_value(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let idx = t.find("  ")?;
    let key = t[..idx].trim().to_string();
    let value = t[idx..].trim().to_string();
    (!key.is_empty() && !value.is_empty()).then_some((key, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = include_str!("../../../tests/fixtures/powertoys/list.stdout.txt");
    const CAPS: &str =
        include_str!("../../../tests/fixtures/powertoys/capabilities-input-source-m2.stdout.txt");
    const GET_M2: &str =
        include_str!("../../../tests/fixtures/powertoys/get-input-source-m2.stdout.txt");
    const GET_M3: &str =
        include_str!("../../../tests/fixtures/powertoys/get-input-source-m3.stdout.txt");
    const GET_ALL: &str =
        include_str!("../../../tests/fixtures/powertoys/get-all-settings-m2.stdout.txt");

    #[test]
    fn parses_the_real_list_output() {
        let rows = parse_list(LIST).unwrap();
        assert_eq!(rows.len(), 3);
        // The internal laptop panel: no name, reached over WMI.
        assert_eq!(rows[0].number, 1);
        assert_eq!(rows[0].name, None);
        assert_eq!(rows[0].method, "WMI");
        // The two AOC panels.
        assert_eq!(rows[1].name.as_deref(), Some("27P2DG5"));
        assert_eq!(rows[1].method, "DDC/CI");
        assert!(rows[1].id.contains("UID4613"));
        assert!(rows[2].id.contains("UID4612"));
    }

    #[test]
    fn a_changed_list_header_is_a_hard_error() {
        let altered = LIST.replacen("Monitor ID", "Device Path", 1);
        let err = parse_list(&altered).unwrap_err();
        assert!(matches!(err, ParseError::ListHeader(_)));
        // The message must point at the version, not just say "parse error".
        assert!(err.to_string().contains("0.101"), "{err}");
    }

    #[test]
    fn a_truncated_row_is_rejected_rather_than_half_read() {
        let bad = "# | Name | Method | Monitor ID\n2 | 27P2DG5 | DDC/CI\n";
        assert!(matches!(
            parse_list(bad).unwrap_err(),
            ParseError::ListRow { .. }
        ));
    }

    #[test]
    fn a_non_numeric_monitor_number_is_rejected() {
        let bad = "# | Name | Method | Monitor ID\nx | 27P2DG5 | DDC/CI | some-id\n";
        assert!(parse_list(bad).is_err());
    }

    #[test]
    fn empty_output_is_an_error_not_an_empty_monitor_list() {
        assert!(parse_list("").is_err());
    }

    #[test]
    fn parses_the_real_capabilities_output() {
        let caps = parse_capabilities(CAPS).unwrap();
        assert!(caps.feature_present);
        let codes: Vec<u8> = caps.options.iter().map(|(c, _)| c.get()).collect();
        assert_eq!(codes, vec![0x01, 0x03, 0x11, 0x0F]);
        assert_eq!(caps.options[2].1, "HDMI-1");
        assert_eq!(caps.model.as_deref(), Some("27P2Q"));
        assert!(caps.raw_capabilities.unwrap().starts_with("(vcp("));
    }

    #[test]
    fn the_parsed_codes_agree_with_the_raw_mccs_string() {
        // Two independent readings of the same claim; a disagreement would
        // mean one of the two parsers is wrong.
        let caps = parse_capabilities(CAPS).unwrap();
        let raw = caps.raw_capabilities.clone().unwrap();
        let from_raw = switcher_core::mccs::input_source_values(&raw)
            .unwrap()
            .unwrap();
        let from_table: Vec<u8> = caps.options.iter().map(|(c, _)| c.get()).collect();
        assert_eq!(from_raw, from_table);
    }

    #[test]
    fn capabilities_without_a_vcp_section_is_an_error() {
        assert_eq!(
            parse_capabilities("Monitor 2 (X) via DDC/CI\n  Model: X\n").unwrap_err(),
            ParseError::MissingVcpSection
        );
    }

    #[test]
    fn a_monitor_without_feature_0x60_parses_but_reports_absence() {
        let text = "Monitor 2 (X) via DDC/CI\n  VCP codes:\n    0x10 Brightness\n";
        let caps = parse_capabilities(text).unwrap();
        assert!(!caps.feature_present);
        assert!(caps.options.is_empty());
    }

    #[test]
    fn parses_the_real_get_output_for_both_panels() {
        // The two panels are on *different* inputs for the same computer.
        let m2 = parse_get(GET_M2).unwrap();
        assert_eq!(m2.input_source, Some(InputCode(0x11)));
        assert!(m2.id.unwrap().contains("UID4613"));

        let m3 = parse_get(GET_M3).unwrap();
        assert_eq!(m3.input_source, Some(InputCode(0x0F)));
        assert!(m3.id.unwrap().contains("UID4612"));
        assert_eq!(m3.protocol.as_deref(), Some("DDC/CI"));
    }

    #[test]
    fn picks_input_source_out_of_a_full_settings_dump() {
        let all = parse_get(GET_ALL).unwrap();
        assert_eq!(all.input_source, Some(InputCode(0x11)));
    }

    #[test]
    fn a_monitor_with_no_input_source_line_reads_as_none_not_as_failure() {
        let text = "Monitor 1 (Internal)\n  protocol           WMI\n  brightness         40\n";
        let parsed = parse_get(text).unwrap();
        assert_eq!(parsed.input_source, None);
        assert_eq!(parsed.protocol.as_deref(), Some("WMI"));
    }

    #[test]
    fn get_output_without_a_monitor_header_is_rejected() {
        assert!(parse_get("input-source       HDMI-1 (0x11)\n").is_err());
    }

    #[test]
    fn an_unparseable_input_value_is_an_error_not_a_silent_none() {
        let text = "Monitor 2 (X)\n  input-source       something unexpected\n";
        assert!(matches!(
            parse_get(text).unwrap_err(),
            ParseError::BadInputCode(_)
        ));
    }

    #[test]
    fn tolerates_extra_whitespace_and_crlf() {
        let text = "Monitor 2 (27P2DG5)\r\n   protocol      DDC/CI  \r\n   input-source    HDMI-1 (0x11)  \r\n";
        assert_eq!(parse_get(text).unwrap().input_source, Some(InputCode(0x11)));
    }
}
