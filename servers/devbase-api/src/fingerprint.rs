// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::Op;
use server_common::{Fault, Result};
use sha2::{Digest, Sha256};
// Existing replay fingerprints use Go's deterministic protobuf encoding. In particular,
// google.protobuf.Struct maps must sort by their string keys, including nested values.
// Keep this wire canonicalization until persisted replay formats are explicitly migrated.
// Wire types that open and close a protobuf group. Groups cannot be declared in proto3, but the
// generated decoder keeps a balanced unknown group and the re-encoded request contains it.
const START_GROUP: u64 = 3;
const END_GROUP: u64 = 4;
// Maximum group nesting accepted. It matches the pinned decoder's recursion limit
// (`buffa::RECURSION_LIMIT`), so every group the decoder keeps fits this parser.
const MAX_GROUP_DEPTH: usize = 100;
struct Field {
    number: u64,
    wire: u64,
    // For a group, the opaque content including the nested tags and the matching end tag.
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
            START_GROUP => {
                // The content is opaque unknown data. Scan it iteratively with an explicit
                // stack so hostile nesting cannot exhaust the call stack.
                let mut open = vec![tag >> 3];
                let mut rest = input;
                while let Some(&expected) = open.last() {
                    let inner = varint(&mut rest)?;
                    match inner & 7 {
                        0 => {
                            varint(&mut rest)?;
                        }
                        1 | 5 => {
                            let n = if inner & 7 == 1 { 8 } else { 4 };
                            if rest.len() < n {
                                return Err(Fault::unavailable());
                            }
                            rest = &rest[n..];
                        }
                        2 => {
                            let n = varint(&mut rest)? as usize;
                            if rest.len() < n {
                                return Err(Fault::unavailable());
                            }
                            rest = &rest[n..];
                        }
                        START_GROUP => {
                            if open.len() >= MAX_GROUP_DEPTH {
                                return Err(Fault::invalid("protobuf group nesting too deep"));
                            }
                            open.push(inner >> 3);
                        }
                        END_GROUP => {
                            if inner >> 3 != expected {
                                return Err(Fault::invalid("mismatched protobuf end group"));
                            }
                            open.pop();
                        }
                        _ => return Err(Fault::invalid("unsupported protobuf wire field")),
                    }
                }
                let b = input[..input.len() - rest.len()].to_vec();
                input = rest;
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
        // A group payload already holds its nested tags and end tag, so it has no length prefix.
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
                // An unknown group numbered 1 is opaque data, never the map key.
                .and_then(|fs| {
                    fs.into_iter()
                        .find(|f| f.number == 1 && f.wire != START_GROUP)
                })
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
    use delino_protocol::devbase::v1::{CreateEndUserRequest, CreateProjectRequest};

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

    fn create_project() -> Vec<u8> {
        let request: CreateProjectRequest = serde_json::from_value(serde_json::json!({
            "idempotencyKey": "h-group-1",
            "tenantId": "019b0000-0000-7000-8000-000000000001",
            "name": "h-group-1"
        }))
        .unwrap();
        request.encode_to_vec()
    }
    fn create_user(metadata: &[u8]) -> Vec<u8> {
        let request: CreateEndUserRequest = serde_json::from_value(serde_json::json!({
            "idempotencyKey": "h-user-1",
            "projectId": "019b0000-0000-7000-8000-000000000001",
            "externalId": "h-user-1"
        }))
        .unwrap();
        let mut bytes = request.encode_to_vec();
        if !metadata.is_empty() {
            bytes.push(0x32);
            put(metadata.len() as u64, &mut bytes);
            bytes.extend_from_slice(metadata);
        }
        bytes
    }
    fn with(base: &[u8], extra: &[u8]) -> Vec<u8> {
        [base, extra].concat()
    }
    fn digest(op: Op, raw: &[u8]) -> Vec<u8> {
        fingerprint(op, raw).unwrap()
    }
    // Field 100 empty group, and the same group wrapping a nested group (field 101).
    const EMPTY: [u8; 4] = [0xa3, 0x06, 0xa4, 0x06];
    const NESTED: [u8; 8] = [0xa3, 0x06, 0xab, 0x06, 0xac, 0x06, 0xa4, 0x06];

    #[test]
    fn balanced_unknown_groups_are_retained_in_the_hash() {
        let base = create_project();
        let plain = digest(Op::CreateProject, &base);
        let empty = digest(Op::CreateProject, &with(&base, &EMPTY));
        let nested = digest(Op::CreateProject, &with(&base, &NESTED));
        assert_eq!(empty, digest(Op::CreateProject, &with(&base, &EMPTY)));
        assert_ne!(plain, empty);
        assert_ne!(plain, nested);
        assert_ne!(empty, nested);
        // Changing only the content of the group changes the hash.
        let varint_inside = [0xa3, 0x06, 0x08, 0x01, 0xa4, 0x06];
        let other_inside = [0xa3, 0x06, 0x08, 0x02, 0xa4, 0x06];
        assert_ne!(
            digest(Op::CreateProject, &with(&base, &varint_inside)),
            digest(Op::CreateProject, &with(&base, &other_inside))
        );
        // Length-delimited and fixed-width content and an unknown scalar still parse.
        let mixed = [
            0xa3, 0x06, 0x0a, 0x02, b'h', b'i', 0x0d, 1, 2, 3, 4, 0x09, 1, 2, 3, 4, 5, 6, 7, 8,
            0xa4, 0x06,
        ];
        digest(Op::CreateProject, &with(&base, &mixed));
        digest(Op::CreateProject, &with(&base, &[0xa0, 0x06, 0x01]));
    }

    #[test]
    fn unknown_groups_inside_metadata_stay_opaque_and_keep_known_keys_canonical() {
        // Struct { fields: { "b": Value{null}, "a": Value{null} } } plus an unknown group in the
        // Struct, in an entry and in a value. Entry key order stays canonical.
        let entry = |key: &str, group: &[u8]| {
            let mut e = vec![0x0a, key.len() as u8];
            e.extend_from_slice(key.as_bytes());
            e.extend_from_slice(&[0x12, 0x02, 0x08, 0x00]);
            e.extend_from_slice(group);
            let mut f = vec![0x0a, e.len() as u8];
            f.extend(e);
            f
        };
        // A group numbered 1 in an entry must not be taken for the key.
        let group_one = [0x0b, 0x0c];
        let ab = [entry("a", &[]), entry("b", &group_one)].concat();
        let ba = [entry("b", &group_one), entry("a", &[])].concat();
        let with_group = |md: &[u8]| [md, &EMPTY[..]].concat();
        assert_eq!(
            digest(Op::CreateEndUser, &create_user(&with_group(&ab))),
            digest(Op::CreateEndUser, &create_user(&with_group(&ba)))
        );
        // Group content inside metadata changes the hash; without it the hash differs again.
        assert_ne!(
            digest(Op::CreateEndUser, &create_user(&with_group(&ab))),
            digest(Op::CreateEndUser, &create_user(&ab))
        );
        assert_ne!(
            digest(Op::CreateEndUser, &create_user(&EMPTY)),
            digest(Op::CreateEndUser, &create_user(&NESTED))
        );
        // Absent metadata and an explicit empty Struct stay distinct.
        let empty_struct = [create_user(&[]), vec![0x32, 0x00]].concat();
        assert_ne!(
            digest(Op::CreateEndUser, &create_user(&[])),
            digest(Op::CreateEndUser, &empty_struct)
        );
    }

    #[test]
    fn malformed_and_excessive_groups_are_rejected() {
        let base = create_project();
        let reject = |extra: &[u8]| fingerprint(Op::CreateProject, &with(&base, extra)).is_err();
        // Unterminated, mismatched end tag, top-level end group, truncated content.
        assert!(reject(&[0xa3, 0x06]));
        assert!(reject(&[0xa3, 0x06, 0xac, 0x06]));
        assert!(reject(&[0xa4, 0x06]));
        assert!(reject(&[0xa3, 0x06, 0xab, 0x06, 0xa4, 0x06]));
        assert!(reject(&[0xa3, 0x06, 0x0a, 0x05, 1]));
        // Wire types 6 and 7 stay unsupported, inside a group too.
        assert!(reject(&[0xa3, 0x06, 0x06, 0xa4, 0x06]));
        assert!(reject(&[0xa6, 0x06]));
        // Nesting at the decoder's limit is accepted and one level deeper is not.
        let nested = |depth: usize| {
            let mut v = Vec::new();
            for _ in 0..depth {
                v.extend_from_slice(&[0xa3, 0x06]);
            }
            for _ in 0..depth {
                v.extend_from_slice(&[0xa4, 0x06]);
            }
            v
        };
        assert!(!reject(&nested(MAX_GROUP_DEPTH)));
        assert!(reject(&nested(MAX_GROUP_DEPTH + 1)));
        assert!(reject(&nested(100_000)));
    }
}
