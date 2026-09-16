//! Parser for MCCS capability strings.
//!
//! A capability string is a nested, parenthesised structure, e.g.
//!
//! ```text
//! (vcp(02 04 10 14(01 05 06) 60(01 03 11 0F ) 62)prot(monitor)type(LCD)mccs_ver(2.2))
//! ```
//!
//! Scanning the whole blob for `60(...)` with a regex is wrong: `60` also
//! appears as a value inside other features' value lists, inside `model(...)`
//! text, and inside vendor sections. This module walks the structure with
//! balanced-paren extraction so `0x60` is only read from the `vcp` section.
//!
//! A capability string is what the monitor *claims*. Nothing here is evidence
//! that a listed input actually exists or has a live source attached.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MccsError {
    #[error("capability string has unbalanced parentheses")]
    UnbalancedParens,
    #[error("capability string has no `vcp(...)` section")]
    NoVcpSection,
    #[error("value list appears before any feature code")]
    ValueListWithoutFeature,
    #[error("`{0}` is not a valid hex byte in the vcp section")]
    BadHexByte(String),
}

/// Feature code -> advertised values. An empty vec means the feature was
/// listed without a value list (normal for continuous features).
pub type VcpFeatures = BTreeMap<u8, Vec<u8>>;

/// Returns the contents of `key(...)` at the top level of `caps`, handling
/// nesting inside the section.
///
/// `Ok(None)` means the key is absent. An unbalanced section is an error
/// rather than an absence, so a truncated capability string is reported as
/// truncated instead of as a monitor that does not support the feature.
pub fn try_extract_section<'a>(caps: &'a str, key: &str) -> Result<Option<&'a str>, MccsError> {
    if !caps.is_ascii() {
        return Ok(None);
    }
    let bytes = caps.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = caps[from..].find(key) {
        let at = from + rel;
        let boundary_ok = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        let mut j = at + key.len();
        while j < bytes.len() && bytes[j] == b' ' {
            j += 1;
        }
        if boundary_ok && j < bytes.len() && bytes[j] == b'(' {
            return balanced_from(caps, j).map(|(inner, _)| Some(inner));
        }
        from = at + key.len();
    }
    Ok(None)
}

/// Convenience wrapper that treats a malformed section as absent.
pub fn extract_section<'a>(caps: &'a str, key: &str) -> Option<&'a str> {
    try_extract_section(caps, key).ok().flatten()
}

/// Given the index of an opening paren, return its contents and the index just
/// past the matching close paren.
fn balanced_from(s: &str, open: usize) -> Result<(&str, usize), MccsError> {
    let bytes = s.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return Err(MccsError::UnbalancedParens);
    }
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((&s[open + 1..i], i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(MccsError::UnbalancedParens)
}

fn parse_hex_byte(tok: &str) -> Result<u8, MccsError> {
    if tok.is_empty() || tok.len() > 2 || !tok.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(MccsError::BadHexByte(tok.to_string()));
    }
    u8::from_str_radix(tok, 16).map_err(|_| MccsError::BadHexByte(tok.to_string()))
}

/// Parse the `vcp(...)` section of a capability string into feature -> values.
pub fn parse_vcp_features(caps: &str) -> Result<VcpFeatures, MccsError> {
    let inner = try_extract_section(caps, "vcp")?.ok_or(MccsError::NoVcpSection)?;
    parse_feature_list(inner)
}

fn parse_feature_list(inner: &str) -> Result<VcpFeatures, MccsError> {
    let mut out: VcpFeatures = BTreeMap::new();
    let bytes = inner.as_bytes();
    let mut pending: Option<u8> = None;
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'(' {
            let code = pending.take().ok_or(MccsError::ValueListWithoutFeature)?;
            let (values, next) = balanced_from(inner, i)?;
            let mut parsed = Vec::new();
            for tok in values.split_whitespace() {
                parsed.push(parse_hex_byte(tok)?);
            }
            out.insert(code, parsed);
            i = next;
            continue;
        }
        if c == b')' {
            return Err(MccsError::UnbalancedParens);
        }

        let start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'('
            && bytes[i] != b')'
        {
            i += 1;
        }
        if let Some(prev) = pending.take() {
            out.entry(prev).or_default();
        }
        pending = Some(parse_hex_byte(&inner[start..i])?);
    }

    if let Some(prev) = pending.take() {
        out.entry(prev).or_default();
    }
    Ok(out)
}

/// The values advertised for VCP 0x60.
///
/// `Ok(None)` means the monitor parsed cleanly but never listed feature 0x60,
/// which is different from a parse failure and different from an empty list.
pub fn input_source_values(caps: &str) -> Result<Option<Vec<u8>>, MccsError> {
    Ok(parse_vcp_features(caps)?
        .get(&crate::types::VCP_INPUT_SOURCE)
        .cloned())
}

/// Pull `model(...)` out of a capability string, when present.
pub fn model(caps: &str) -> Option<String> {
    extract_section(caps, "model").map(|s| s.trim().to_string())
}

/// Pull `mccs_ver(...)` out of a capability string, when present.
pub fn mccs_version(caps: &str) -> Option<String> {
    extract_section(caps, "mccs_ver").map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured verbatim from PowerToys PowerDisplay 0.101.2362.0 against an
    /// AOC 27P2DG5 on 2026-09-16. See tests/fixtures/powertoys/.
    const AOC_REAL: &str = "(vcp(02 04 05 08 10 12 14(01 05 06 08 0B) 16 18 1A 52 60(01 03 11 0F ) 62 86(02 05) C8 C9 CC(01 02 03 04 05 06 07 09 0A 0B 0C 0D 0E 12 14 16 1E) B6 DF C6 DC(00 0B 0C 0D 0E 0F 10) D6(01 04) ED F8)prot(monitor)type(LCD)cmds(01 02 03 07 0C F3)mccs_ver(2.2)asset_eep(64)mpu_ver(005)model(27P2Q)mswhql(1))";

    #[test]
    fn parses_real_aoc_capability_string() {
        let values = input_source_values(AOC_REAL).unwrap().unwrap();
        assert_eq!(values, vec![0x01, 0x03, 0x11, 0x0F]);
        assert_eq!(model(AOC_REAL).as_deref(), Some("27P2Q"));
        assert_eq!(mccs_version(AOC_REAL).as_deref(), Some("2.2"));
    }

    #[test]
    fn features_without_value_lists_are_kept_with_empty_values() {
        let features = parse_vcp_features(AOC_REAL).unwrap();
        assert_eq!(features.get(&0x02), Some(&vec![]));
        assert_eq!(
            features.get(&0x14),
            Some(&vec![0x01, 0x05, 0x06, 0x08, 0x0B])
        );
    }

    #[test]
    fn does_not_mistake_a_value_for_the_input_feature() {
        // 0x60 appears here only as a *value* of feature 0xDC, and inside the
        // model text. Scanning the whole blob for `60` would find both.
        let caps = "(vcp(10 DC(60 61))prot(monitor)model(60))";
        assert_eq!(input_source_values(caps).unwrap(), None);
    }

    #[test]
    fn a_feature_list_outside_the_vcp_section_is_not_read() {
        // Some vendors emit extra sections. Only `vcp(...)` defines features,
        // so a `60(...)` living anywhere else must be ignored.
        let caps = "(vcp(10 12)vendorspecific(60(01 0F))prot(monitor))";
        assert_eq!(input_source_values(caps).unwrap(), None);
    }

    #[test]
    fn oversized_tokens_in_the_vcp_section_are_rejected() {
        // A 4-digit token means this is not the structure we think it is.
        // Better to fail loudly than to silently read half the section.
        assert!(matches!(
            parse_vcp_features("(vcp(10 E060(01 02)))").unwrap_err(),
            MccsError::BadHexByte(_)
        ));
    }

    #[test]
    fn handles_whitespace_variation() {
        let caps = "(vcp(  60(01   0F  )  10 )prot(monitor))";
        assert_eq!(
            input_source_values(caps).unwrap().unwrap(),
            vec![0x01, 0x0F]
        );
    }

    #[test]
    fn missing_vcp_section_is_an_error_not_an_empty_list() {
        let err = input_source_values("(prot(monitor)type(LCD))").unwrap_err();
        assert_eq!(err, MccsError::NoVcpSection);
    }

    #[test]
    fn unbalanced_parens_are_rejected() {
        assert_eq!(
            parse_vcp_features("(vcp(60(01 0F)").unwrap_err(),
            MccsError::UnbalancedParens
        );
    }

    #[test]
    fn malformed_hex_is_rejected_rather_than_skipped() {
        assert!(matches!(
            parse_vcp_features("(vcp(60(01 ZZ))").unwrap_err(),
            MccsError::BadHexByte(_)
        ));
    }

    #[test]
    fn feature_listed_with_empty_value_list_is_distinct_from_absent() {
        let caps = "(vcp(60()10))";
        assert_eq!(input_source_values(caps).unwrap(), Some(vec![]));
    }
}
