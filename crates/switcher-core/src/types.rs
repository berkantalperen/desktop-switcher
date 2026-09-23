//! Domain vocabulary: monitor identity, VCP 0x60 input codes, and the
//! deliberately non-collapsing result types the plan calls for.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The MCCS feature code for Input Source. This is the *feature*, never a value.
pub const VCP_INPUT_SOURCE: u8 = 0x60;

/// A value written to or read from VCP `0x60`.
///
/// Values are monitor specific. Nothing in this type implies a given code is
/// supported, correct, or attached to a live source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InputCode(pub u8);

impl InputCode {
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    /// The label MCCS 2.2 suggests for well known codes.
    ///
    /// Advisory only: a monitor may use a standard code for a nonstandard port,
    /// so this is for display, never for deciding what to write.
    pub fn standard_label(self) -> Option<&'static str> {
        Some(match self.0 {
            0x01 => "VGA-1",
            0x02 => "VGA-2",
            0x03 => "DVI-1",
            0x04 => "DVI-2",
            0x05 => "Composite-1",
            0x06 => "Composite-2",
            0x07 => "S-Video-1",
            0x08 => "S-Video-2",
            0x09 => "Tuner-1",
            0x0A => "Tuner-2",
            0x0B => "Tuner-3",
            0x0C => "Component-1",
            0x0D => "Component-2",
            0x0E => "Component-3",
            0x0F => "DisplayPort-1",
            0x10 => "DisplayPort-2",
            0x11 => "HDMI-1",
            0x12 => "HDMI-2",
            _ => return None,
        })
    }
}

impl fmt::Display for InputCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:02X}", self.0)
    }
}

/// Why a string was rejected as an input code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputCodeParseError {
    /// A bare number such as `11` is ambiguous: decimal 11 is `0x0B`, but a
    /// user copying `11` out of a capabilities listing means `0x11`. Refuse
    /// rather than pick one and switch the monitor to the wrong input.
    #[error("input code `{0}` is ambiguous: write it with an explicit `0x` prefix (e.g. `0x11`)")]
    MissingHexPrefix(String),
    #[error("input code `{0}` is not a valid 8-bit hex value")]
    NotHexByte(String),
    #[error("input code is empty")]
    Empty,
}

impl FromStr for InputCode {
    type Err = InputCodeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(InputCodeParseError::Empty);
        }
        let Some(hex) = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
        else {
            return Err(InputCodeParseError::MissingHexPrefix(trimmed.to_string()));
        };
        if hex.is_empty() || hex.len() > 2 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(InputCodeParseError::NotHexByte(trimmed.to_string()));
        }
        u8::from_str_radix(hex, 16)
            .map(InputCode)
            .map_err(|_| InputCodeParseError::NotHexByte(trimmed.to_string()))
    }
}

impl Serialize for InputCode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for InputCode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// How much we actually know about a claim.
///
/// Never upgrade a level without the evidence that level names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Evidence {
    /// No information at all.
    #[default]
    Unknown,
    /// The capabilities string lists it. Capabilities strings are routinely
    /// wrong, incomplete, or list inputs the panel does not physically have.
    Reported,
    /// We read this value back from the monitor at some point.
    ///
    /// A register fact, not a picture fact: VCP 0x60 reports the value the
    /// monitor last stored, which can differ from the input it is displaying.
    ReadConfirmed,
    /// We wrote the value and a subsequent read returned it.
    ///
    /// Still a register fact. It proves the monitor accepted and kept the
    /// value, and nothing more — one panel held `0x0F` for several minutes
    /// while displaying VGA the whole time.
    WriteConfirmed,
    /// A human watched the physical input change and said so. The only level
    /// that justifies using a code in an unattended switch.
    UserConfirmed,
}

impl Evidence {
    /// Whether this level is good enough to issue an unattended write.
    pub fn is_trusted_for_switching(self) -> bool {
        matches!(self, Evidence::UserConfirmed)
    }

    pub fn label(self) -> &'static str {
        match self {
            Evidence::Unknown => "unknown",
            Evidence::Reported => "reported",
            Evidence::ReadConfirmed => "read-confirmed",
            Evidence::WriteConfirmed => "write-confirmed",
            Evidence::UserConfirmed => "user-confirmed",
        }
    }
}

impl fmt::Display for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// How the host reaches the display. Only `DdcCi` can carry VCP 0x60.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    DdcCi,
    /// Windows WMI brightness path, i.e. the internal laptop panel.
    /// Not an input-switch target, and kept available for recovery.
    Wmi,
    Other(String),
}

impl Transport {
    pub fn supports_input_switching(&self) -> bool {
        matches!(self, Transport::DdcCi)
    }
}

impl fmt::Display for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Transport::DdcCi => f.write_str("DDC/CI"),
            Transport::Wmi => f.write_str("WMI"),
            Transport::Other(s) => f.write_str(s),
        }
    }
}

/// What a host knows about one physical display right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorIdentity {
    /// Backend-stable handle: a PowerToys DevicePath id, or a ddcutil selector.
    pub backend_id: String,
    /// Discovery-time ordinal. An addressing convenience, NOT identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovery_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// EDID serial: the only field that tracks the panel rather than the port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    pub transport: Transport,
}

impl MonitorIdentity {
    pub fn describe(&self) -> String {
        let model = self.model.as_deref().unwrap_or("unknown model");
        match &self.serial {
            Some(sn) => format!("{model} (s/n {sn})"),
            None => format!("{model} (no serial in EDID)"),
        }
    }
}

/// A display found by a backend, with the raw evidence it was parsed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedMonitor {
    pub identity: MonitorIdentity,
    /// Unmodified backend output this record came from.
    pub raw: String,
}

impl DetectedMonitor {
    pub fn handle(&self) -> MonitorHandle {
        MonitorHandle {
            backend_id: self.identity.backend_id.clone(),
            discovery_index: self.identity.discovery_index,
            model: self.identity.model.clone(),
            serial: self.identity.serial.clone(),
        }
    }
}

/// Everything a backend needs to address one display in a later command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorHandle {
    pub backend_id: String,
    pub discovery_index: Option<u32>,
    pub model: Option<String>,
    pub serial: Option<String>,
}

/// One entry in a monitor's advertised input list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputSourceOption {
    pub code: InputCode,
    /// Label as printed by the backend, when it supplied one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl fmt::Display for InputSourceOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.label.as_deref().or_else(|| self.code.standard_label()) {
            Some(l) => write!(f, "{l} ({})", self.code),
            None => write!(f, "{}", self.code),
        }
    }
}

/// Parsed capabilities for VCP 0x60, plus the untouched source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputCapabilities {
    /// Whether the monitor advertised feature 0x60 at all.
    pub feature_present: bool,
    pub options: Vec<InputSourceOption>,
    /// Raw MCCS capabilities string, if the backend exposed one.
    pub raw_capabilities: Option<String>,
    /// Full unmodified backend stdout, always retained as evidence.
    pub raw_output: String,
}

impl InputCapabilities {
    pub fn advertises(&self, code: InputCode) -> bool {
        self.options.iter().any(|o| o.code == code)
    }
}

/// The result of trying to read VCP 0x60.
///
/// These stay separate on purpose: collapsing them into `Option` would let
/// "the monitor stopped answering because we switched it away" masquerade as
/// "this monitor has no input source feature".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputReading {
    Value(InputCode),
    /// The monitor answered, and does not implement 0x60.
    Unsupported,
    /// The read failed while the link was believed to be up.
    ReadFailed {
        detail: String,
    },
    /// The read failed right after we switched this monitor away from us,
    /// which is expected and is not evidence that the switch failed.
    UnavailableAfterSwitch {
        detail: String,
    },
}

impl InputReading {
    pub fn value(&self) -> Option<InputCode> {
        match self {
            InputReading::Value(c) => Some(*c),
            _ => None,
        }
    }
}

impl fmt::Display for InputReading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputReading::Value(c) => match c.standard_label() {
                Some(l) => write!(f, "{l} ({c})"),
                None => write!(f, "{c}"),
            },
            InputReading::Unsupported => f.write_str("feature 0x60 not supported"),
            InputReading::ReadFailed { detail } => write!(f, "read failed: {detail}"),
            InputReading::UnavailableAfterSwitch { detail } => {
                write!(f, "unreadable after switch (expected): {detail}")
            }
        }
    }
}

/// What actually happened when we wrote VCP 0x60.
///
/// `Accepted` deliberately does not say "succeeded": a zero exit status only
/// means the DDC write was dispatched without an error being reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// Command returned success. Visual state unconfirmed.
    Accepted,
    /// The monitor read the value back. It has told us what is in its input
    /// register, which is not the same as what it is putting on the panel.
    StoredByMonitor(InputCode),
    ConfirmedByUser,
    Failed {
        detail: String,
    },
    /// We could not determine whether the write landed.
    Unknown {
        detail: String,
    },
}

impl WriteOutcome {
    /// True only when we have positive evidence the write did not land.
    pub fn is_failure(&self) -> bool {
        matches!(self, WriteOutcome::Failed { .. })
    }

    pub fn evidence(&self) -> Evidence {
        match self {
            WriteOutcome::ConfirmedByUser => Evidence::UserConfirmed,
            WriteOutcome::StoredByMonitor(_) => Evidence::WriteConfirmed,
            _ => Evidence::Unknown,
        }
    }
}

impl fmt::Display for WriteOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteOutcome::Accepted => f.write_str("issued, visual state unconfirmed"),
            WriteOutcome::StoredByMonitor(c) => {
                write!(f, "monitor stored {c}; it does not report what it displays")
            }
            WriteOutcome::ConfirmedByUser => f.write_str("confirmed by user"),
            WriteOutcome::Failed { detail } => write!(f, "failed: {detail}"),
            WriteOutcome::Unknown { detail } => write!(f, "unknown: {detail}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A read-back is the monitor quoting its own register back at us. It is
    /// not a witness to what the panel is displaying, and nothing shown to a
    /// person may suggest that it is.
    ///
    /// Written after a panel stored `0x0F`, kept showing VGA, and was reported
    /// as "confirmed by read (0x0F) ... confirmed".
    #[test]
    fn a_read_back_never_reports_itself_as_confirmation() {
        let outcome = WriteOutcome::StoredByMonitor(InputCode(0x0F));
        let text = outcome.to_string().to_lowercase();
        assert!(
            !text.contains("confirm"),
            "a register read must not claim confirmation, said: {text}"
        );
        assert!(
            text.contains("not") && text.contains("display"),
            "it must say what it does not prove, said: {text}"
        );
    }

    /// Only a person who watched the screen gets to use that word.
    #[test]
    fn only_a_human_confirms() {
        assert!(WriteOutcome::ConfirmedByUser
            .to_string()
            .to_lowercase()
            .contains("confirm"));
        assert_eq!(
            WriteOutcome::ConfirmedByUser.evidence(),
            Evidence::UserConfirmed
        );
    }

    #[test]
    fn hex_prefixed_codes_parse() {
        assert_eq!("0x11".parse::<InputCode>().unwrap(), InputCode(0x11));
        assert_eq!("0X0f".parse::<InputCode>().unwrap(), InputCode(0x0F));
        assert_eq!(" 0x1 ".parse::<InputCode>().unwrap(), InputCode(0x01));
    }

    #[test]
    fn bare_numbers_are_refused_as_ambiguous() {
        // The whole point: `11` must never silently become decimal 11 (0x0B).
        let err = "11".parse::<InputCode>().unwrap_err();
        assert!(matches!(err, InputCodeParseError::MissingHexPrefix(_)));
        assert!(err.to_string().contains("0x"));
    }

    #[test]
    fn malformed_codes_are_refused() {
        assert!("0x".parse::<InputCode>().is_err());
        assert!("0xZZ".parse::<InputCode>().is_err());
        assert!("0x111".parse::<InputCode>().is_err());
        assert!("".parse::<InputCode>().is_err());
    }

    #[test]
    fn codes_render_as_two_digit_hex() {
        assert_eq!(InputCode(0x0F).to_string(), "0x0F");
        assert_eq!(InputCode(0x11).to_string(), "0x11");
    }

    #[test]
    fn only_user_confirmation_unlocks_switching() {
        assert!(Evidence::UserConfirmed.is_trusted_for_switching());
        for level in [
            Evidence::Unknown,
            Evidence::Reported,
            Evidence::ReadConfirmed,
            Evidence::WriteConfirmed,
        ] {
            assert!(!level.is_trusted_for_switching(), "{level} must not unlock");
        }
    }

    #[test]
    fn accepted_write_is_not_treated_as_confirmed() {
        assert_eq!(WriteOutcome::Accepted.evidence(), Evidence::Unknown);
        assert!(!WriteOutcome::Accepted.is_failure());
        assert!(WriteOutcome::Accepted.to_string().contains("unconfirmed"));
    }

    #[test]
    fn internal_panel_is_not_switchable() {
        assert!(!Transport::Wmi.supports_input_switching());
        assert!(Transport::DdcCi.supports_input_switching());
    }
}
