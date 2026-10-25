// SPDX-License-Identifier: FSL-1.1-ALv2
//! Inspect original evidence without rewriting it. Consent is checked by the upload transaction.
use crate::{Fault, Result};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::{fmt, io::Read};

const MAX_DECODED: usize = 64 << 20;
fn invalid() -> Fault {
    Fault::precondition("uninspectable artifact; retain original locally or on GitHub")
}
fn secret(text: &str) -> Result<()> {
    let lower = text.to_ascii_lowercase();
    if [
        "-----begin",
        "private key",
        "authorization:",
        "bearer ",
        "ghp_",
        "ghs_",
        "github_pat_",
        "sk-ant-",
        "sk-proj-",
        "dlk_",
        "aws_secret_access_key",
        "client_secret",
        "access_token",
        "refresh_token",
        "api_key",
        "password",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return Err(Fault::precondition(
            "secret detected; remove credentials before upload",
        ));
    }
    Ok(())
}

fn secret_in_decoded_text(text: &str) -> Result<()> {
    // Plain logs may contain JSON-style escapes without being JSON documents. Decode ASCII
    // escapes before scanning so a log prefix cannot conceal an existing secret needle.
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && bytes.get(i + 1) == Some(&b'u')
            && let Some(hex) = bytes.get(i + 2..i + 6)
            && let Ok(hex) = std::str::from_utf8(hex)
            && let Ok(value) = u16::from_str_radix(hex, 16)
            && let Ok(value) = u8::try_from(value)
            && value.is_ascii()
        {
            decoded.push(value);
            i += 6;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    secret(std::str::from_utf8(&decoded).map_err(|_| invalid())?)
}

fn inspect_json(bytes: &[u8], raw: bool) -> Result<()> {
    let mut d = serde_json::Deserializer::from_slice(bytes);
    Inspect { raw }.deserialize(&mut d).map_err(|_| invalid())?;
    d.end().map_err(|_| invalid())
}

fn complete_json_prefix(bytes: &[u8]) -> bool {
    let mut d = serde_json::Deserializer::from_slice(bytes);
    Inspect { raw: true }.deserialize(&mut d).is_ok()
}

fn text_label_prefix(line: &str) -> bool {
    // A closed, non-JSON bracket label is ordinary log text (GoogleTest, timestamps,
    // progress, or logger levels). JSON punctuation inside it retains fail-closed parsing.
    let line = line.trim_start();
    let Some(open) = line.chars().next() else {
        return false;
    };
    let close = match open {
        '[' => ']',
        '{' => '}',
        _ => return false,
    };
    let Some(end) = line.find(close) else {
        return false;
    };
    let label = &line[1..end];
    !label.is_empty()
        && !label
            .chars()
            .any(|c| matches!(c, '[' | ']' | '{' | '}' | '"' | ','))
        && (open == '[' || !label.contains(':'))
        && inspect_json(&line.as_bytes()[..=end], true).is_err()
}

// Visit every JSON key and string, including duplicate keys and escaped values. A Value alone
// discards overwritten keys, which could hide secrets in the original retained bytes.
#[derive(Clone, Copy)]
struct Inspect {
    raw: bool,
}
impl<'de> DeserializeSeed<'de> for Inspect {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Inspect {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("eligible evidence")
    }
    fn visit_str<E: de::Error>(self, s: &str) -> std::result::Result<(), E> {
        secret(s).map_err(|_| E::custom("secret detected"))?;
        if !self.raw && (s.contains("diff --git") || s.contains("@@ -")) {
            return Err(E::custom("source content is not eligible"));
        }
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> std::result::Result<(), A::Error> {
        let mut keys = std::collections::BTreeSet::new();
        while let Some(k) = a.next_key::<String>()? {
            // Structured observations must describe every original byte; decoding
            // duplicate keys into Value would silently discard earlier content.
            if !self.raw && !keys.insert(k.clone()) {
                return Err(de::Error::custom("duplicate structured artifact key"));
            }
            self.visit_str::<A::Error>(&k)?;
            if !self.raw
                && matches!(
                    k.as_str(),
                    "source_code"
                        | "sourceCode"
                        | "diff"
                        | "patch"
                        | "content"
                        | "source_snippet"
                        | "sourceSnippet"
                        | "repository_files"
                )
            {
                return Err(de::Error::custom("source content is not eligible"));
            }
            a.next_value_seed(self)?;
        }
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<(), A::Error> {
        while a.next_element_seed(self)?.is_some() {}
        Ok(())
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<(), E> {
        Ok(())
    }
}

pub fn content_allowed(bytes: &[u8], kind: &str) -> Result<()> {
    if bytes.len() > MAX_DECODED {
        return Err(invalid());
    }
    let raw = matches!(
        kind,
        "ARTIFACT_KIND_RAW_PROFILE" | "ARTIFACT_KIND_RAW_TEST_LOG"
    );
    if bytes.starts_with(&[0x1f, 0x8b]) {
        if kind != "ARTIFACT_KIND_RAW_PROFILE" {
            return Err(invalid());
        }
        return pprof(bytes);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    // Binary that happens to be UTF-8 is not a text profile or test log.
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(invalid());
    }
    secret(text)?;
    if !raw {
        return inspect_json(bytes, false);
    }
    if inspect_json(bytes, true).is_ok() {
        return Ok(());
    }
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next().unwrap_or("").trim_start();
    if let Some(second) = lines.next()
        && inspect_json(first.as_bytes(), true).is_ok()
    {
        if inspect_json(second.as_bytes(), true).is_ok()
            && lines.all(|line| inspect_json(line.as_bytes(), true).is_ok())
        {
            return Ok(());
        }
        return Err(invalid());
    }
    if first.starts_with(['{', '[', '"']) && complete_json_prefix(first.as_bytes()) {
        // A complete JSON value followed by non-JSON text on the same line is not a log label.
        return Err(invalid());
    }
    if first.starts_with(['{', '[']) && !text_label_prefix(first) {
        // A JSON-looking first line that fails single-document or NDJSON inspection remains
        // ineligible, including truncated/deep JSON and mixed streams.
        return Err(invalid());
    }
    secret_in_decoded_text(text)?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Message {
    Profile,
    ValueType,
    Sample,
    Label,
    Mapping,
    Location,
    Line,
    Function,
}
#[derive(Clone, Copy)]
enum Field {
    Number,
    Numbers,
    Index,
    Indices,
    Text,
    Nested(Message),
}
// Exhaustive public pprof wire schema. Do not skip unknown fields: a future string/source
// extension is ineligible until this privacy boundary can inspect it. No generated schema
// decoder is used because typical protobuf decoders silently discard unknown fields.
fn field(message: Message, tag: u64) -> Result<Field> {
    use Field::*;
    use Message::*;
    Ok(match (message, tag) {
        (Profile, 1 | 11) => Nested(ValueType),
        (Profile, 2) => Nested(Sample),
        (Profile, 3) => Nested(Mapping),
        (Profile, 4) => Nested(Location),
        (Profile, 5) => Nested(Function),
        (Profile, 6) => Text,
        (Profile, 7 | 8 | 14 | 15) => Index,
        (Profile, 9 | 10 | 12) => Number,
        (Profile, 13) => Indices,
        (ValueType, 1 | 2) | (Label, 1 | 2 | 4) | (Mapping, 5 | 6) | (Function, 2..=4) => Index,
        (Sample, 1 | 2) => Numbers,
        (Sample, 3) => Nested(Label),
        (Label, 3)
        | (Mapping, 1..=4 | 7..=10)
        | (Location, 1..=3 | 5)
        | (Line, 1..=3)
        | (Function, 1 | 5) => Number,
        (Location, 4) => Nested(Line),
        _ => return Err(invalid()),
    })
}
fn varint(input: &mut &[u8]) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let (&byte, rest) = input.split_first().ok_or_else(invalid)?;
        *input = rest;
        if shift == 63 && byte > 1 {
            return Err(invalid());
        }
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(invalid())
}
#[derive(Default)]
struct Scan {
    strings: usize,
    max_index: u64,
    fields: usize,
}
fn message(mut input: &[u8], schema: Message, scan: &mut Scan) -> Result<()> {
    while !input.is_empty() {
        scan.fields += 1;
        if scan.fields > 1_000_000 {
            return Err(invalid());
        }
        let key = varint(&mut input)?;
        let kind = field(schema, key >> 3)?;
        match (kind, key & 7) {
            (Field::Number | Field::Numbers | Field::Index | Field::Indices, 0) => {
                let n = varint(&mut input)?;
                if matches!(kind, Field::Index | Field::Indices) {
                    scan.max_index = scan.max_index.max(n);
                }
            }
            (Field::Text | Field::Nested(_) | Field::Numbers | Field::Indices, 2) => {
                let len = usize::try_from(varint(&mut input)?).map_err(|_| invalid())?;
                let data = input.get(..len).ok_or_else(invalid)?;
                input = &input[len..];
                match kind {
                    Field::Text => {
                        let text = std::str::from_utf8(data).map_err(|_| invalid())?;
                        if scan.strings == 0 && !text.is_empty() {
                            return Err(invalid());
                        }
                        secret(text)?; // Includes unused strings, filenames, labels, comments and doc URLs.
                        scan.strings += 1;
                    }
                    Field::Nested(nested) => message(data, nested, scan)?,
                    Field::Numbers | Field::Indices => {
                        let mut packed = data;
                        while !packed.is_empty() {
                            let n = varint(&mut packed)?;
                            if matches!(kind, Field::Indices) {
                                scan.max_index = scan.max_index.max(n);
                            }
                        }
                    }
                    _ => unreachable!(),
                }
            }
            _ => return Err(invalid()),
        }
    }
    Ok(())
}
fn pprof(bytes: &[u8]) -> Result<()> {
    let mut decoder = flate2::bufread::GzDecoder::new(bytes);
    let header = decoder.header().ok_or_else(invalid)?;
    // Gzip metadata is outside the protobuf schema but is retained in the original artifact.
    // Extra fields have no supported privacy schema, so reject them rather than ignore them.
    if header.extra().is_some() {
        return Err(invalid());
    }
    for text in [header.filename(), header.comment()].into_iter().flatten() {
        secret(std::str::from_utf8(text).map_err(|_| invalid())?)?;
    }
    let mut decoded = Vec::new();
    decoder
        .by_ref()
        .take(MAX_DECODED as u64 + 1)
        .read_to_end(&mut decoded)
        .map_err(|_| invalid())?;
    if decoded.len() > MAX_DECODED || !decoder.into_inner().is_empty() {
        return Err(invalid());
    }
    let mut scan = Scan::default();
    message(&decoded, Message::Profile, &mut scan)?;
    if scan.strings == 0 || scan.max_index >= scan.strings as u64 {
        return Err(invalid());
    }
    Ok(())
}
