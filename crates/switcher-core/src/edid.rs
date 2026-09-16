//! Minimal EDID 1.x reader, used to recover a panel's serial number.
//!
//! This matters because the identifiers the two backends hand out track the
//! *port* a monitor is plugged into, not the monitor. Two identical AOC panels
//! swapped between two cables keep their port ids and exchange their pictures.
//! The EDID serial is the one field that follows the panel, so it is what a
//! binding is checked against before any write.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EdidError {
    #[error("EDID block is {0} bytes, expected at least 128")]
    TooShort(usize),
    #[error("EDID header signature is missing")]
    BadHeader,
}

const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
const DESCRIPTOR_OFFSETS: [usize; 4] = [54, 72, 90, 108];
const TAG_SERIAL: u8 = 0xFF;
const TAG_NAME: u8 = 0xFC;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EdidInfo {
    /// Three-letter PNP id, e.g. `AOC`.
    pub manufacturer: Option<String>,
    /// Descriptor 0xFC, e.g. `27P2DG5`.
    pub model_name: Option<String>,
    /// Descriptor 0xFF, e.g. `ASFPA9A001109`.
    pub serial_text: Option<String>,
    /// Bytes 12..16. Often 0, and often identical across units of a model.
    pub serial_binary: u32,
    pub product_code: u16,
    pub manufacture_year: Option<u16>,
    pub manufacture_week: Option<u8>,
}

impl EdidInfo {
    /// The best serial available, preferring the human-readable descriptor.
    ///
    /// Returns `None` rather than inventing one: a monitor with no serial must
    /// be handled by the ambiguity path, not bound to a guess.
    pub fn best_serial(&self) -> Option<String> {
        if let Some(text) = self.serial_text.as_ref() {
            if !text.is_empty() && text != "0" {
                return Some(text.clone());
            }
        }
        if self.serial_binary != 0 {
            return Some(format!("bin:{:08X}", self.serial_binary));
        }
        None
    }
}

pub fn parse(edid: &[u8]) -> Result<EdidInfo, EdidError> {
    if edid.len() < 128 {
        return Err(EdidError::TooShort(edid.len()));
    }
    if edid[..8] != HEADER {
        return Err(EdidError::BadHeader);
    }

    let mut info = EdidInfo {
        manufacturer: decode_pnp_id(u16::from_be_bytes([edid[8], edid[9]])),
        product_code: u16::from_le_bytes([edid[10], edid[11]]),
        serial_binary: u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]),
        ..Default::default()
    };

    if edid[16] > 0 && edid[16] <= 54 {
        info.manufacture_week = Some(edid[16]);
    }
    if edid[17] > 0 {
        info.manufacture_year = Some(1990 + u16::from(edid[17]));
    }

    for &off in &DESCRIPTOR_OFFSETS {
        let block = &edid[off..off + 18];
        // A text descriptor starts 00 00 00 <tag> 00.
        if block[0] != 0 || block[1] != 0 || block[2] != 0 {
            continue;
        }
        let text = decode_descriptor_text(&block[5..18]);
        match block[3] {
            TAG_SERIAL => info.serial_text = text,
            TAG_NAME => info.model_name = text,
            _ => {}
        }
    }

    Ok(info)
}

/// Three 5-bit letters packed into a big-endian u16, offset from `A` = 1.
fn decode_pnp_id(packed: u16) -> Option<String> {
    let mut out = String::with_capacity(3);
    for shift in [10u16, 5, 0] {
        let v = ((packed >> shift) & 0x1F) as u8;
        if v == 0 || v > 26 {
            return None;
        }
        out.push((b'A' + v - 1) as char);
    }
    Some(out)
}

/// Descriptor text is terminated by 0x0A and padded with spaces.
fn decode_descriptor_text(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0x0A).unwrap_or(bytes.len());
    let text: String = bytes[..end]
        .iter()
        .filter(|&&b| (0x20..0x7F).contains(&b))
        .map(|&b| b as char)
        .collect();
    let trimmed = text.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic but structurally valid EDID for tests.
    fn synth(serial: Option<&str>, name: Option<&str>, bin_serial: u32) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&HEADER);
        // "AOC" = A=1, O=15, C=3 -> (1<<10)|(15<<5)|3
        let packed: u16 = (1 << 10) | (15 << 5) | 3;
        e[8..10].copy_from_slice(&packed.to_be_bytes());
        e[10..12].copy_from_slice(&0x2702u16.to_le_bytes());
        e[12..16].copy_from_slice(&bin_serial.to_le_bytes());
        e[16] = 41;
        e[17] = 33; // 2023

        let mut write_desc = |off: usize, tag: u8, text: &str| {
            e[off] = 0;
            e[off + 1] = 0;
            e[off + 2] = 0;
            e[off + 3] = tag;
            e[off + 4] = 0;
            let bytes = text.as_bytes();
            for i in 0..13 {
                e[off + 5 + i] = if i < bytes.len() {
                    bytes[i]
                } else if i == bytes.len() {
                    0x0A
                } else {
                    0x20
                };
            }
        };
        if let Some(s) = serial {
            write_desc(54, TAG_SERIAL, s);
        }
        if let Some(n) = name {
            write_desc(72, TAG_NAME, n);
        }
        e
    }

    #[test]
    fn reads_identity_fields() {
        let edid = synth(Some("ASFPA9A001109"), Some("27P2DG5"), 0);
        let info = parse(&edid).unwrap();
        assert_eq!(info.manufacturer.as_deref(), Some("AOC"));
        assert_eq!(info.model_name.as_deref(), Some("27P2DG5"));
        assert_eq!(info.serial_text.as_deref(), Some("ASFPA9A001109"));
        assert_eq!(info.manufacture_year, Some(2023));
        assert_eq!(info.manufacture_week, Some(41));
        assert_eq!(info.best_serial().as_deref(), Some("ASFPA9A001109"));
    }

    #[test]
    fn falls_back_to_binary_serial_then_gives_up() {
        let with_bin = parse(&synth(None, Some("27P2DG5"), 0xDEADBEEF)).unwrap();
        assert_eq!(with_bin.best_serial().as_deref(), Some("bin:DEADBEEF"));

        // No usable serial at all must stay None so the ambiguity path runs.
        let none = parse(&synth(None, Some("27P2DG5"), 0)).unwrap();
        assert_eq!(none.best_serial(), None);
    }

    #[test]
    fn placeholder_zero_serial_is_not_treated_as_a_serial() {
        let info = parse(&synth(Some("0"), Some("NE16NZH"), 0)).unwrap();
        assert_eq!(info.best_serial(), None);
    }

    #[test]
    fn rejects_short_and_headerless_blocks() {
        assert_eq!(parse(&[0u8; 12]).unwrap_err(), EdidError::TooShort(12));
        assert_eq!(parse(&[0xAAu8; 128]).unwrap_err(), EdidError::BadHeader);
    }
}
