// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::Op;
use server_common::{Fault, Result};
use sha2::{Digest, Sha256};
// Replay fingerprints follow Go's deterministic protobuf encoding. In particular,
// google.protobuf.Struct maps sort their known entries by string key, including nested values,
// and every other (unknown) field keeps its retained order and bytes.
//
// Stored fingerprints contain a 0x01 marker followed by the canonical SHA-256 digest.
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
const DIGEST_LEN: usize = 32;
const CANONICAL_V1_MARKER: u8 = 1;

#[derive(Clone, Copy)]
enum Kind {
    Struct,
    Value,
    List,
    Entry,
}
fn entry_key(f: &Field) -> Vec<u8> {
    fields(&f.payload)
        .ok()
        // An unknown group numbered 1 is opaque data, never the map key.
        .and_then(|fs| {
            fs.into_iter()
                .find(|f| f.number == 1 && f.wire != START_GROUP)
        })
        .map(|f| f.payload)
        .unwrap_or_default()
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
        // Sort known map entries only; opaque unknown fields keep their retained order.
        let (mut known, unknown): (Vec<_>, Vec<_>) =
            fs.into_iter().partition(|f| f.number == 1 && f.wire == 2);
        known.sort_by_cached_key(entry_key);
        known.extend(unknown);
        fs = known;
    }
    Ok(encode(fs))
}
fn digest(op: Op, raw: &[u8]) -> Result<Vec<u8>> {
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

/// The value stored with a new replay record: `CanonicalV1`.
pub fn fingerprint(op: Op, raw: &[u8]) -> Result<Vec<u8>> {
    let mut stored = vec![CANONICAL_V1_MARKER];
    stored.extend(digest(op, raw)?);
    Ok(stored)
}

/// Compares a canonical stored fingerprint with the received request.
pub fn matches(stored: &[u8], op: Op, raw: &[u8]) -> Result<bool> {
    let received = fingerprint(op, raw)?;
    if stored.len() != DIGEST_LEN + 1 || stored.first() != Some(&CANONICAL_V1_MARKER) {
        tracing::error!(
            stored_len = stored.len(),
            "unsupported replay fingerprint format"
        );
        return Err(Fault::unavailable());
    }
    Ok(stored == received)
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
        let raw = request.encode_to_vec();
        const GO: &str = "c610c6b7dd3f6f4343d4a7d1b6977c522497ba909d19793a366d5a3a08c3c3b0";
        // The stored value includes the format marker.
        assert_eq!(hex::encode(go_digest(Op::CreateEndUser, &raw)), GO);
        assert_eq!(
            hex::encode(fingerprint(Op::CreateEndUser, &raw).unwrap()),
            format!("01{GO}")
        );
    }

    fn go_digest(op: Op, raw: &[u8]) -> Vec<u8> {
        super::digest(op, raw).unwrap()
    }
    fn unhex(s: &str) -> Vec<u8> {
        hex::decode(s).unwrap()
    }

    // Requests and digests below come from the pinned Go oracle: the repository's generated Go
    // types, `proto.Unmarshal`, a cleared idempotency key, `Deterministic: true` marshalling and
    // SHA-256 over operation, NUL and the encoding.
    const PRIMARY: &str = "0a08692d757365722d31122430313962303030302d303030302d373030302d383030302d3030303030303030303030311a08692d757365722d31320c0a070a016112022001a00601";
    const PRIMARY_GO: &str = "d817bf7c3dc45bd987c7941965565720171959134ef5a3b8d8de2c625528a574";
    const REQUEST_PREFIX: &str = "0a08692d757365722d31122430313962303030302d303030302d373030302d383030302d3030303030303030303030311a08692d757365722d31";
    const KNOWN_ENTRY: &str = "0a070a016112022001";
    const OPAQUE_B: &str = "a206030a0162";
    const OPAQUE_C: &str = "a206030a0163";
    const BC_GO: &str = "1d9e2ae1441faf0ea127333ad005d9cdaaa5e24ff5205bfa440aad6a79db6f3e";
    const CB_GO: &str = "cfaf2e90860d386ea113cb5fef40954e7dc9f3eb35621136a88e34984e0472c4";
    const CREATE_NESTED: &str = "0a016b122430313962303030302d303030302d373030302d383030302d3030303030303030303030311a0365787432710a4d0a016d12482a460a070a017a120220010a310a0161122c322a0a1d2a1b0a070a0179120220010a070a016212020800a00601ba06030a017a0a031a0174ad0601020304b10601020304050607080a070a016312020800a00601ad0601020304b1060102030405060708ba06030a017a";
    const CREATE_NESTED_GO: &str =
        "5b45708bcbd156a5c708b8f8889d7954f16812dd8f249dade22a61958d13bc61";
    const UPDATE_NESTED: &str = "0a016b122430313962303030302d303030302d373030302d383030302d3030303030303030303030311a037569643a710a4d0a016d12482a460a070a017a120220010a310a0161122c322a0a1d2a1b0a070a0179120220010a070a016212020800a00601ba06030a017a0a031a0174ad0601020304b10601020304050607080a070a016312020800a00601ad0601020304b1060102030405060708ba06030a017a420a0a086d65746164617461";
    const UPDATE_NESTED_GO: &str =
        "5a9280f27794d2160b55f7dc691056b5be197a75faf7883fe66b61566b0a31d6";
    const CREATE_SCALARS: &str = "0a016b122430313962303030302d303030302d373030302d383030302d3030303030303030303030311a0365787432330a070a0162120220010a060a00120208000a070a016112020800ad0601020304b1060102030405060708ba06030a017aa00601";
    const CREATE_SCALARS_GO: &str =
        "728df06e89ec98bfa7c834b18dab9643b6fe529866e99894fc08e2f5d5a1df3d";

    fn bc_cb(first: &str, second: &str) -> Vec<u8> {
        // Common request prefix, then a metadata Struct holding a known entry and two opaque
        // occurrences of unknown field 100.
        unhex(&format!("{REQUEST_PREFIX}3215{KNOWN_ENTRY}{first}{second}"))
    }

    #[test]
    fn canonical_digest_matches_the_go_oracle() {
        for (op, request, expected) in [
            (Op::CreateEndUser, PRIMARY, PRIMARY_GO),
            (Op::CreateEndUser, CREATE_NESTED, CREATE_NESTED_GO),
            (Op::UpdateEndUser, UPDATE_NESTED, UPDATE_NESTED_GO),
            (Op::CreateEndUser, CREATE_SCALARS, CREATE_SCALARS_GO),
        ] {
            assert_eq!(hex::encode(go_digest(op, &unhex(request))), expected);
        }
    }

    #[test]
    fn opaque_unknown_field_order_is_preserved() {
        let bc = bc_cb(OPAQUE_B, OPAQUE_C);
        let cb = bc_cb(OPAQUE_C, OPAQUE_B);
        assert_eq!(hex::encode(go_digest(Op::CreateEndUser, &bc)), BC_GO);
        assert_eq!(hex::encode(go_digest(Op::CreateEndUser, &cb)), CB_GO);
        let stored = fingerprint(Op::CreateEndUser, &bc).unwrap();
        assert!(matches(&stored, Op::CreateEndUser, &bc).unwrap());
        assert!(!matches(&stored, Op::CreateEndUser, &cb).unwrap());
    }

    #[test]
    fn known_entries_sort_and_unknown_fields_change_the_digest() {
        let sorted = unhex(CREATE_SCALARS);
        // Changing an unknown scalar value or the order of unknown fields changes the digest.
        let mut changed = sorted.clone();
        *changed.last_mut().unwrap() = 2;
        assert_ne!(
            go_digest(Op::CreateEndUser, &sorted),
            go_digest(Op::CreateEndUser, &changed)
        );
        // Known entries in another order are the same request; unknown content is not reordered.
        let a = create_user(&[entry("a"), entry("b")].concat());
        let b = create_user(&[entry("b"), entry("a")].concat());
        assert_eq!(
            go_digest(Op::CreateEndUser, &a),
            go_digest(Op::CreateEndUser, &b)
        );
        // Absent metadata and an explicit empty Struct stay distinct.
        let empty_struct = [create_user(&[]), vec![0x32, 0x00]].concat();
        assert_ne!(
            go_digest(Op::CreateEndUser, &create_user(&[])),
            go_digest(Op::CreateEndUser, &empty_struct)
        );
    }

    fn entry(key: &str) -> Vec<u8> {
        let mut e = vec![0x0a, key.len() as u8];
        e.extend_from_slice(key.as_bytes());
        e.extend_from_slice(&[0x12, 0x02, 0x08, 0x00]);
        let mut f = vec![0x0a, e.len() as u8];
        f.extend(e);
        f
    }

    #[test]
    fn stored_formats_are_routed_by_length_and_marker() {
        let raw = unhex(PRIMARY);
        let op = Op::CreateEndUser;
        let go = unhex(PRIMARY_GO);
        let envelope = |d: &[u8]| [&[1u8][..], d].concat();
        // New rows are the envelope around the corrected digest.
        assert_eq!(fingerprint(op, &raw).unwrap(), envelope(&go));
        assert!(matches(&envelope(&go), op, &raw).unwrap());
        assert!(!matches(&envelope(&[7; 32]), op, &raw).unwrap());
        // Unsupported lengths and markers fail as unavailable.
        for stored in [
            go.clone(),
            vec![],
            vec![1],
            vec![1; 31],
            vec![1; 34],
            [&[2u8][..], &go].concat(),
            [&[0u8][..], &go].concat(),
            [&go[..], &[1][..]].concat(),
        ] {
            assert!(matches(&stored, op, &raw).is_err(), "{}", stored.len());
        }
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
