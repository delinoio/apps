// SPDX-License-Identifier: FSL-1.1-ALv2
//! Request field validation shared by the services.
use crate::{ApiError, Result};
use buffa::{EnumValue, Enumeration};
use uuid::Uuid;

/// A canonical lowercase UUID v7 identifier.
pub fn id(field: &str, value: &str) -> Result<Uuid> {
    if !server_common::canonical_id(value) {
        return Err(ApiError::invalid(field, "invalid identifier"));
    }
    Uuid::parse_str(value).map_err(|_| ApiError::invalid(field, "invalid identifier"))
}

/// An optional identifier: empty means absent.
pub fn optional_id(field: &str, value: &str) -> Result<Option<Uuid>> {
    if value.is_empty() {
        Ok(None)
    } else {
        id(field, value).map(Some)
    }
}

/// A known, non-zero enum value.
pub fn known<E: Enumeration>(field: &str, value: &EnumValue<E>) -> Result<E> {
    match value.as_known() {
        Some(e) if e.to_i32() != 0 => Ok(e),
        _ => Err(ApiError::invalid(field, "unknown or unspecified value")),
    }
}

/// An enum value that may be unspecified (zero) but must otherwise be known.
pub fn optional_enum<E: Enumeration>(field: &str, value: &EnumValue<E>) -> Result<Option<E>> {
    match value.as_known() {
        Some(e) if e.to_i32() == 0 => Ok(None),
        Some(e) => Ok(Some(e)),
        None => Err(ApiError::invalid(field, "unknown value")),
    }
}

/// Display names, dashboard names and widget titles: `min` to `max` Unicode scalar values after
/// trimming and no control characters. Returns the trimmed text.
pub fn text(field: &str, value: &str, min: usize, max: usize) -> Result<String> {
    let trimmed = value.trim();
    let count = trimmed.chars().count();
    if count < min || count > max || trimmed.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            field,
            format!("must hold {min} to {max} characters without control characters"),
        ));
    }
    Ok(trimmed.into())
}

/// Model identifiers and metric dimensions: printable ASCII without whitespace.
pub fn token(field: &str, value: &str, min: usize, max: usize) -> Result<()> {
    if value.len() < min || value.len() > max || !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ApiError::invalid(
            field,
            "must be printable ASCII without whitespace",
        ));
    }
    Ok(())
}

/// Collector and OS version strings reported by devices.
pub fn version_text(field: &str, value: &str) -> Result<()> {
    if value.len() > 100 || value.chars().any(char::is_control) {
        return Err(ApiError::invalid(field, "invalid version text"));
    }
    Ok(())
}

/// A lowercase hex SHA-256 fingerprint or empty.
pub fn fingerprint(field: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || (value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    {
        Ok(())
    } else {
        Err(ApiError::invalid(
            field,
            "must be a lowercase hex SHA-256 digest",
        ))
    }
}

/// An IANA time zone name.
pub fn time_zone(field: &str, value: &str) -> Result<chrono_tz::Tz> {
    value
        .parse::<chrono_tz::Tz>()
        .map_err(|_| ApiError::invalid(field, "unknown time zone"))
}

/// A decimal amount string in major currency units.
pub fn decimal(field: &str, value: &str) -> Result<()> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let (int, frac) = match digits.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (digits, None),
    };
    let int_ok = int == "0"
        || (!int.is_empty() && !int.starts_with('0') && int.bytes().all(|b| b.is_ascii_digit()));
    let frac_ok =
        frac.is_none_or(|f| (1..=12).contains(&f.len()) && f.bytes().all(|b| b.is_ascii_digit()));
    if int_ok && frac_ok && int.len() <= 18 {
        Ok(())
    } else {
        Err(ApiError::invalid(field, "invalid decimal amount"))
    }
}

/// Page size (default 50, at most 200).
pub fn page_size(size: u32) -> Result<i64> {
    match size {
        0 => Ok(50),
        1..=200 => Ok(i64::from(size)),
        _ => Err(ApiError::invalid(
            "page.size",
            "page size must be at most 200",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn field_rules() {
        assert_eq!(text("name", "  Main  ", 1, 100).unwrap(), "Main");
        assert!(text("name", "   ", 1, 100).is_err());
        assert!(text("name", &"x".repeat(101), 1, 100).is_err());
        assert!(text("name", &"é".repeat(100), 1, 100).is_ok());
        assert!(text("name", &"é".repeat(101), 1, 100).is_err());
        assert!(text("name", "a\u{7}", 1, 100).is_err());
        assert!(token("model", "gpt-5", 1, 200).is_ok());
        assert!(token("model", "gpt 5", 1, 200).is_err());
        assert!(token("model", "", 1, 200).is_err());
        for ok in ["0", "12.5", "-3.000000000001", "100"] {
            assert!(decimal("a", ok).is_ok(), "{ok}");
        }
        for bad in ["", "01", "1.", ".5", "1.0000000000001", "1e5", "--1"] {
            assert!(decimal("a", bad).is_err(), "{bad}");
        }
        assert!(fingerprint("f", &"a".repeat(64)).is_ok());
        assert!(fingerprint("f", &"A".repeat(64)).is_err());
        assert!(time_zone("tz", "Asia/Kathmandu").is_ok());
        assert!(time_zone("tz", "Mars/Base").is_err());
        assert_eq!(page_size(0).unwrap(), 50);
        assert!(page_size(201).is_err());
        assert!(id("id", &crate::new_id().to_string()).is_ok());
        assert!(id("id", "01900000-0000-4000-8000-000000000001").is_err());
    }
}
