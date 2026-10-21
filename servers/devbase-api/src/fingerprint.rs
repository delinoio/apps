// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::Op;
use server_common::{Fault, Result};
use sha2::{Digest, Sha256};
// Existing replay fingerprints use Go's deterministic protobuf encoding. In particular,
// google.protobuf.Struct maps must sort by their string keys, including nested values.
// Keep this wire canonicalization until persisted replay formats are explicitly migrated.
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
#[derive(Clone, Copy)]
enum Kind {
    Struct,
    Value,
    List,
    Entry,
}
fn canonical(raw: &[u8], kind: Kind) -> Result<Vec<u8>> {
    let mut fs = fields(raw)?;
    for f in &mut fs {
        let child = match (kind, f.number, f.wire) {
            (Kind::Struct, 1, 2) => Some(Kind::Entry),
            (Kind::Entry, 2, 2) | (Kind::List, 1, 2) => Some(Kind::Value),
            (Kind::Value, 5, 2) => Some(Kind::Struct),
            (Kind::Value, 6, 2) => Some(Kind::List),
            _ => None,
        };
        if let Some(child) = child {
            f.payload = canonical(&f.payload, child)?
        }
    }
    if matches!(kind, Kind::Struct) {
        fs.sort_by_cached_key(|f| {
            fields(&f.payload)
                .ok()
                .and_then(|fs| fs.into_iter().find(|f| f.number == 1))
                .map(|f| f.payload)
                .unwrap_or_default()
        });
    }
    Ok(encode(fs))
}
pub fn fingerprint(op: Op, raw: &[u8]) -> Result<Vec<u8>> {
    let mut fs = fields(raw)?;
    fs.retain(|f| f.number != 1);
    let metadata = match op {
        Op::CreateEndUser => Some(6),
        Op::UpdateEndUser => Some(7),
        _ => None,
    };
    for f in &mut fs {
        if Some(f.number) == metadata && f.wire == 2 {
            f.payload = canonical(&f.payload, Kind::Struct)?
        }
    }
    let mut hash = Sha256::new();
    hash.update(op.name());
    hash.update([0]);
    hash.update(encode(fs));
    Ok(hash.finalize().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::Message;
    use delino_protocol::devbase::v1::CreateEndUserRequest;

    #[test]
    fn matches_go_deterministic_metadata_fingerprint() {
        // Generated with Go proto.MarshalOptions{Deterministic: true}; includes
        // an empty map key and nested list/struct values to cover wire defaults.
        let request: CreateEndUserRequest = serde_json::from_value(serde_json::json!({
            "idempotencyKey": "not-in-fingerprint",
            "projectId": "019b0000-0000-7000-8000-000000000001",
            "externalId": "external",
            "metadata": {"z": 1.0, "a": {"y": 2.0, "b": [null, true, 3.0, "text"]}, "": false}
        }))
        .unwrap();
        assert_eq!(
            hex::encode(fingerprint(Op::CreateEndUser, &request.encode_to_vec()).unwrap()),
            "c610c6b7dd3f6f4343d4a7d1b6977c522497ba909d19793a366d5a3a08c3c3b0"
        );
    }
}
