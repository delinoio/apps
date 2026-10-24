// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::Op;
use crate::{Fault, Result};
use sha2::{Digest, Sha256};
struct Field {
    number: u64,
    wire: u64,
    payload: Vec<u8>,
}
fn varint(input: &mut &[u8]) -> Result<u64> {
    let mut n = 0;
    for shift in (0..70).step_by(7) {
        let b = *input.first().ok_or_else(Fault::unavailable)?;
        *input = &input[1..];
        n |= ((b & 127) as u64) << shift;
        if b < 128 {
            return Ok(n);
        }
    }
    Err(Fault::unavailable())
}
fn put(mut n: u64, out: &mut Vec<u8>) {
    while n >= 128 {
        out.push((n as u8) | 128);
        n >>= 7
    }
    out.push(n as u8)
}
fn fields(mut input: &[u8]) -> Result<Vec<Field>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        let tag = varint(&mut input)?;
        let wire = tag & 7;
        let before = input;
        let payload = match wire {
            0 => {
                varint(&mut input)?;
                before[..before.len() - input.len()].to_vec()
            }
            1 | 5 => {
                let n = if wire == 1 { 8 } else { 4 };
                if input.len() < n {
                    return Err(Fault::unavailable());
                }
                let b = input[..n].to_vec();
                input = &input[n..];
                b
            }
            2 => {
                let n = varint(&mut input)? as usize;
                if input.len() < n {
                    return Err(Fault::unavailable());
                }
                let b = input[..n].to_vec();
                input = &input[n..];
                b
            }
            _ => return Err(Fault::invalid("unsupported protobuf wire field")),
        };
        out.push(Field {
            number: tag >> 3,
            wire,
            payload,
        });
    }
    Ok(out)
}
fn encode(fields: Vec<Field>) -> Vec<u8> {
    let mut out = Vec::new();
    for f in fields {
        put(f.number << 3 | f.wire, &mut out);
        if f.wire == 2 {
            put(f.payload.len() as u64, &mut out)
        }
        out.extend(f.payload);
    }
    out
}
// Sort map entries so equivalent requests have the same fingerprint.
fn canonical_map(raw: &[u8], number: u64) -> Result<Vec<u8>> {
    let mut fs = fields(raw)?;
    let mut maps: Vec<_> = fs
        .iter()
        .filter(|f| f.number == number && f.wire == 2)
        .map(|f| f.payload.clone())
        .collect();
    maps.sort_by_cached_key(|entry| {
        fields(entry)
            .ok()
            .and_then(|fs| fs.into_iter().find(|f| f.number == 1))
            .map(|f| f.payload)
            .unwrap_or_default()
    });
    let mut maps = maps.into_iter();
    for f in &mut fs {
        if f.number == number && f.wire == 2 {
            f.payload = maps.next().unwrap();
        }
    }
    Ok(encode(fs))
}
pub fn fingerprint(op: Op, raw: &[u8]) -> Result<String> {
    let raw = match op {
        Op::CorrectEvent => canonical_map(raw, 7)?,
        Op::Authorize => canonical_map(raw, 6)?,
        Op::AuthorizeApiKey => canonical_map(raw, 5)?,
        Op::IngestEvent => {
            let mut fs = fields(raw)?;
            for f in &mut fs {
                if f.number == 2 && f.wire == 2 {
                    f.payload = canonical_map(&f.payload, 7)?;
                }
            }
            encode(fs)
        }
        _ => raw.to_vec(),
    };
    let mut hash = Sha256::new();
    hash.update(format!("/delcore.v1.DelcoreService/{}\0", op.name()));
    hash.update(raw);
    Ok(format!("{:x}", hash.finalize()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sorts_map_entries_without_sorting_repeated_fields() {
        let entry = |key: u8| Field {
            number: 7,
            wire: 2,
            payload: vec![10, 1, key, 18, 3, 10, 1, b'v'],
        };
        let raw = encode(vec![entry(b'z'), entry(b'a')]);
        assert_eq!(
            canonical_map(&raw, 7).unwrap(),
            encode(vec![entry(b'a'), entry(b'z')])
        );
    }
    #[test]
    fn api_key_authorization_canonicalizes_group_values_without_covering_other_ops() {
        let entry = |key: u8| Field {
            number: 5,
            wire: 2,
            payload: vec![10, 1, key, 18, 3, 10, 1, b'v'],
        };
        let sorted = encode(vec![entry(b'a'), entry(b'z')]);
        let shuffled = encode(vec![entry(b'z'), entry(b'a')]);
        assert_eq!(
            fingerprint(Op::AuthorizeApiKey, &shuffled).unwrap(),
            fingerprint(Op::AuthorizeApiKey, &sorted).unwrap()
        );
        // The procedure name is part of the hash, so operations never collide.
        assert_ne!(
            fingerprint(Op::AuthorizeApiKey, &sorted).unwrap(),
            fingerprint(Op::Authorize, &sorted).unwrap()
        );
        assert_ne!(
            fingerprint(Op::CreateApiKey, &sorted).unwrap(),
            fingerprint(Op::RevokeApiKey, &sorted).unwrap()
        );
    }
    #[test]
    fn hashes_go_compatible_ingestion_map_wire() {
        use buffa::Message;
        let request: delino_protocol::delcore::v1::IngestEventRequest = serde_json::from_value(serde_json::json!({
            "requestKey":"019b0000-0000-7000-8000-000000000001",
            "event":{"tenantId":"019b0000-0000-7000-8000-000000000001","eventKey":"key","eventType":"event",
                "attributes":{"z":{"text":"v"},"":{"text":"v"},"a":{"text":"v"}}}
        })).unwrap();
        // SHA-256 over the procedure, NUL, and ascending-field protobuf wire,
        // including explicitly encoded empty map keys as Go does.
        assert_eq!(
            fingerprint(Op::IngestEvent, &request.encode_to_vec()).unwrap(),
            "b05c0086c2d4c37db16fbc7eb5517f47893f1d7dedeb3c2d21e354eda8418f90"
        );
    }
}
