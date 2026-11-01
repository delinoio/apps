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

/// A page URL that is not an acceptable HTTP(S) URL never matches; an invalid
/// prefix is an error because stored rules are normalized on write.
pub fn matches(prefix: &str, page: &str) -> Result<bool> {
    let prefix = normalize_prefix(prefix)?;
    let Ok(page) = normalize_prefix(page) else {
        return Ok(false);
    };
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
    // Mirror the native store's structural PNG rules (lane_capture_helpers.h):
    // a valid IHDR pixel format, CRCs, chunk order and the terminal boundary.
    // This rejects truncated images and appended originals without decoding
    // untrusted pixels; the server additionally decodes the pixel stream.
    let depth = png[24];
    let color = png[25];
    let valid_format = match color {
        0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(depth, 8 | 16),
        _ => false,
    };
    if !valid_format {
        return Err(invalid("Invalid PNG pixel format."));
    }
    if png[26] != 0 || png[27] != 0 || png[28] > 1 {
        return Err(invalid("Invalid PNG encoding method."));
    }
    let mut offset = 8usize;
    let (mut image_data, mut ended_data, mut palette) = (false, false, false);
    let mut data_bytes = 0usize;
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
        let kind = &header[4..8];
        if !kind.iter().all(u8::is_ascii_alphabetic) {
            return Err(invalid("Invalid PNG chunk type."));
        }
        if kind[2] & 32 != 0 {
            return Err(invalid("Invalid PNG reserved chunk bit."));
        }
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
        match kind {
            b"IHDR" => {
                if offset != 8 || size != 13 {
                    return Err(invalid("Duplicate PNG header."));
                }
            }
            b"PLTE" => {
                if palette
                    || image_data
                    || size == 0
                    || size > 768
                    || !size.is_multiple_of(3)
                    || matches!(color, 0 | 4)
                {
                    return Err(invalid("Invalid PNG palette."));
                }
                palette = true;
            }
            b"IDAT" => {
                if ended_data || (color == 3 && !palette) {
                    return Err(invalid("Invalid PNG image data order."));
                }
                image_data = true;
                data_bytes += size;
            }
            b"IEND" => {
                return if size == 0 && image_data && data_bytes > 0 && end == png.len() {
                    Ok(())
                } else {
                    Err(invalid("Invalid final PNG boundary."))
                };
            }
            _ => {
                // Unknown chunks must be ancillary; they end any IDAT sequence.
                if kind[0] & 32 == 0 {
                    return Err(invalid("Unknown critical PNG chunk."));
                }
                if image_data {
                    ended_data = true;
                }
            }
        }
        offset = end;
    }
    Err(invalid("PNG is missing its final boundary."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    const ONE_PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
    fn chunk(kind: [u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
        out.extend_from_slice(&kind);
        out.extend_from_slice(data);
        let mut crc = !0u32;
        for byte in &out[4..] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320_u32 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        out.extend_from_slice(&(!crc).to_be_bytes());
        out
    }
    /// A 1x1 PNG with the given IHDR pixel fields and chunks after IHDR.
    fn png(depth: u8, color: u8, method: [u8; 3], chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = vec![0, 0, 0, 1, 0, 0, 0, 1, depth, color];
        header.extend_from_slice(&method);
        out.extend(chunk(*b"IHDR", &header));
        for c in chunks {
            out.extend_from_slice(c);
        }
        out
    }
    fn reference(bytes: &[u8]) -> Value {
        serde_json::json!({"attachmentId":"01900100-0000-7000-8000-000000000001","width":1,"height":1,"sha256":hash(bytes),"byteSize":bytes.len().to_string()})
    }
    fn check(bytes: &[u8]) -> Result<()> {
        validate_png(&reference(bytes), bytes)
    }
    #[test]
    fn png_structure_matches_native_validation() {
        let original = STANDARD.decode(ONE_PIXEL).unwrap();
        check(&original).unwrap();
        // The pixel stream of the original RGBA image.
        let data = original[41..41 + 13].to_vec();
        let idat = chunk(*b"IDAT", &data);
        let iend = chunk(*b"IEND", &[]);
        let text = chunk(*b"tEXt", b"k\0v");
        let plte = chunk(*b"PLTE", &[0, 0, 0]);
        check(&png(8, 6, [0, 0, 0], &[idat.clone(), iend.clone()])).unwrap();
        check(&png(
            8,
            6,
            [0, 0, 0],
            &[text.clone(), idat.clone(), text.clone(), iend.clone()],
        ))
        .unwrap();
        check(&png(
            8,
            3,
            [0, 0, 0],
            &[plte.clone(), idat.clone(), iend.clone()],
        ))
        .unwrap();
        check(&png(8, 6, [0, 0, 1], &[idat.clone(), iend.clone()])).unwrap();
        let empty_idat = chunk(*b"IDAT", &[]);
        for (case, bytes) in [
            (
                "color type",
                png(8, 5, [0, 0, 0], &[idat.clone(), iend.clone()]),
            ),
            (
                "truecolor depth",
                png(4, 2, [0, 0, 0], &[idat.clone(), iend.clone()]),
            ),
            (
                "palette depth",
                png(
                    16,
                    3,
                    [0, 0, 0],
                    &[plte.clone(), idat.clone(), iend.clone()],
                ),
            ),
            (
                "compression",
                png(8, 6, [1, 0, 0], &[idat.clone(), iend.clone()]),
            ),
            (
                "filter",
                png(8, 6, [0, 1, 0], &[idat.clone(), iend.clone()]),
            ),
            (
                "interlace",
                png(8, 6, [0, 0, 2], &[idat.clone(), iend.clone()]),
            ),
            (
                "missing palette",
                png(8, 3, [0, 0, 0], &[idat.clone(), iend.clone()]),
            ),
            (
                "grayscale palette",
                png(8, 0, [0, 0, 0], &[plte.clone(), idat.clone(), iend.clone()]),
            ),
            (
                "late palette",
                png(8, 2, [0, 0, 0], &[idat.clone(), plte.clone(), iend.clone()]),
            ),
            (
                "palette size",
                png(
                    8,
                    3,
                    [0, 0, 0],
                    &[chunk(*b"PLTE", &[0, 0]), idat.clone(), iend.clone()],
                ),
            ),
            (
                "duplicate palette",
                png(
                    8,
                    3,
                    [0, 0, 0],
                    &[plte.clone(), plte, idat.clone(), iend.clone()],
                ),
            ),
            (
                "split image data",
                png(
                    8,
                    6,
                    [0, 0, 0],
                    &[idat.clone(), text, idat.clone(), iend.clone()],
                ),
            ),
            (
                "empty image data",
                png(8, 6, [0, 0, 0], &[empty_idat, iend.clone()]),
            ),
            (
                "unknown critical",
                png(
                    8,
                    6,
                    [0, 0, 0],
                    &[chunk(*b"ABCD", &[]), idat.clone(), iend.clone()],
                ),
            ),
            (
                "reserved bit",
                png(
                    8,
                    6,
                    [0, 0, 0],
                    &[chunk(*b"abcd", &[]), idat.clone(), iend.clone()],
                ),
            ),
            (
                "chunk type",
                png(
                    8,
                    6,
                    [0, 0, 0],
                    &[chunk(*b"ab1d", &[]), idat.clone(), iend.clone()],
                ),
            ),
            (
                "duplicate header",
                png(
                    8,
                    6,
                    [0, 0, 0],
                    &[
                        chunk(*b"IHDR", &original[16..29]),
                        idat.clone(),
                        iend.clone(),
                    ],
                ),
            ),
            (
                "missing end",
                png(8, 6, [0, 0, 0], std::slice::from_ref(&idat)),
            ),
            ("trailing bytes", {
                let mut bytes = png(8, 6, [0, 0, 0], &[idat, iend]);
                bytes.push(0);
                bytes
            }),
        ] {
            assert!(check(&bytes).is_err(), "{case}");
        }
    }
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
