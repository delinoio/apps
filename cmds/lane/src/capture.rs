// SPDX-License-Identifier: Apache-2.0
//! `RealQA` validation shared by local capture persistence and synchronization.
use crate::error::{Result, invalid};
use crate::protocol::{hash, strv, valid_id};
use serde_json::Value;

/// Query strings and fragments are metadata, never repository selection inputs.
pub fn normalize_prefix(input: &str) -> Result<String> {
    if input.trim() != input || input.contains('\\') || input.chars().any(char::is_control) {
        return Err(invalid(
            "Use an HTTP or HTTPS URL without whitespace or backslashes.",
        ));
    }
    let mut url = url::Url::parse(input).map_err(|_| invalid("Invalid URL prefix."))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid("Use an HTTP or HTTPS URL without credentials."));
    }
    url.set_query(None);
    url.set_fragment(None);
    // One trailing slash denotes the same path boundary as its absence.
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(if path.is_empty() { "/" } else { &path });
    Ok(url.to_string())
}

pub fn matches(prefix: &str, page: &str) -> Result<bool> {
    let prefix = normalize_prefix(prefix)?;
    let page = normalize_prefix(page)?;
    let prefix = url::Url::parse(&prefix).map_err(|_| invalid("Invalid URL prefix."))?;
    let page = url::Url::parse(&page).map_err(|_| invalid("Invalid page URL."))?;
    if prefix.origin() != page.origin() {
        return Ok(false);
    }
    let path = prefix.path();
    Ok(path == "/"
        || page.path() == path
        || page
            .path()
            .strip_prefix(path)
            .is_some_and(|suffix| suffix.starts_with('/')))
}

/// Conflicted and deleted alternatives never participate in preselection.
pub fn select_repository<'a>(page: &str, rules: &'a [Value]) -> Result<Option<&'a str>> {
    let mut best: Option<(usize, &str)> = None;
    for rule in rules {
        if rule["result"] == "RESULT_KIND_CONFLICT" || rule["result"] == "RESULT_KIND_DELETED" {
            continue;
        }
        let connection = &rule["version"]["urlConnection"];
        let repository = strv(connection, "repositoryId");
        if !valid_id(repository) {
            continue;
        }
        let prefix = normalize_prefix(strv(connection, "prefix"))?;
        if matches(&prefix, page)? && best.is_none_or(|(length, _)| prefix.len() > length) {
            best = Some((prefix.len(), repository));
        }
    }
    Ok(best.map(|(_, repository)| repository))
}

/// Validate metadata without needing image bytes (for pending synchronized images).
/// PNG IHDR dimensions are limited to 2^31 - 1; SQLite byte counts use signed i64.
pub fn validate_reference(capture: &Value) -> Result<()> {
    let hash = strv(capture, "sha256");
    if !valid_id(strv(capture, "attachmentId"))
        || !capture["width"]
            .as_u64()
            .is_some_and(|n| n > 0 && i32::try_from(n).is_ok())
        || !capture["height"]
            .as_u64()
            .is_some_and(|n| n > 0 && i32::try_from(n).is_ok())
        || !strv(capture, "byteSize")
            .parse::<i64>()
            .is_ok_and(|n| n > 0)
        || hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("Invalid immutable attachment reference."));
    }
    Ok(())
}

/// Validate immutable image identity before accepting a final PNG into storage.
pub fn validate_png(capture: &Value, png: &[u8]) -> Result<()> {
    validate_reference(capture)?;
    if !valid_id(strv(capture, "attachmentId"))
        || png.len() < 33
        || png.get(..8) != Some(b"\x89PNG\r\n\x1a\n".as_slice())
        || png.get(8..16) != Some(b"\0\0\0\rIHDR".as_slice())
    {
        return Err(invalid(
            "A final PNG and UUID v7 attachment identity are required.",
        ));
    }
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    if width == 0
        || height == 0
        || capture["width"].as_u64() != Some(u64::from(width))
        || capture["height"].as_u64() != Some(u64::from(height))
        || strv(capture, "sha256") != hash(png)
        || strv(capture, "byteSize").parse::<usize>().ok() != Some(png.len())
    {
        return Err(invalid(
            "PNG content does not match its attachment reference.",
        ));
    }
    // Check all chunks, including CRCs and the terminal boundary. This rejects
    // truncated images and appended originals without decoding untrusted pixels.
    let mut offset = 8usize;
    let mut image_data = false;
    while offset < png.len() {
        let header = png
            .get(offset..offset.saturating_add(8))
            .ok_or_else(|| invalid("Truncated PNG chunk."))?;
        let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(12)
            .and_then(|n| n.checked_add(size))
            .ok_or_else(|| invalid("Invalid PNG chunk size."))?;
        let chunk = png
            .get(offset..end)
            .ok_or_else(|| invalid("Truncated PNG chunk."))?;
        let expected = u32::from_be_bytes(chunk[8 + size..].try_into().unwrap());
        let mut crc = !0u32;
        for byte in &chunk[4..8 + size] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320_u32 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        if !crc != expected {
            return Err(invalid("Invalid PNG checksum."));
        }
        let kind = &header[4..8];
        if kind == b"IDAT" {
            image_data = true;
        }
        if kind == b"IEND" {
            return if size == 0 && image_data && end == png.len() {
                Ok(())
            } else {
                Err(invalid("Invalid final PNG boundary."))
            };
        }
        offset = end;
    }
    Err(invalid("PNG is missing its final boundary."))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_only_normalized_origin_and_path_boundaries() {
        assert_eq!(
            normalize_prefix("HTTPS://EXAMPLE.COM:443/app?secret=x#y").unwrap(),
            "https://example.com/app"
        );
        for page in [
            "https://example.com/app",
            "https://example.com/app/child?x=1",
        ] {
            assert!(matches("https://EXAMPLE.COM:443/app", page).unwrap());
        }
        for page in [
            "https://example.com/apple",
            "https://example.com/App",
            "http://example.com/app",
            "https://example.com:444/app",
        ] {
            assert!(!matches("https://example.com/app", page).unwrap());
        }
        for input in [
            "file:///app",
            "https://user:password@example.com/app",
            " https://example.com",
            "https://example.com\\app",
        ] {
            assert!(normalize_prefix(input).is_err());
        }
    }
    #[test]
    fn chooses_longest_resolved_rule_and_does_not_guess() {
        let first = "01900100-0000-7000-8000-000000000001";
        let second = "01900100-0000-7000-8000-000000000002";
        let rules = vec![
            serde_json::json!({"version":{"urlConnection":{"prefix":"https://example.com/","repositoryId":first}}}),
            serde_json::json!({"version":{"urlConnection":{"prefix":"https://example.com/app","repositoryId":second}}}),
            serde_json::json!({"result":"RESULT_KIND_CONFLICT","version":{"urlConnection":{"prefix":"https://example.com/app/page","repositoryId":first}}}),
        ];
        assert_eq!(
            select_repository("https://example.com/app/page", &rules).unwrap(),
            Some(second)
        );
        assert_eq!(
            select_repository("https://elsewhere.test/app", &rules).unwrap(),
            None
        );
    }
}
