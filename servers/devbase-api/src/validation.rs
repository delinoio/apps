// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::Op;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use server_common::{Fault, Result, canonical_id, integer, string};
pub fn identifier(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 256 || s.contains('\0') || s.trim() != s {
        Err(Fault::invalid(
            "identifier must be 1 to 256 bytes without surrounding whitespace",
        ))
    } else {
        Ok(())
    }
}
pub fn metadata(q: &Value) -> Result<Value> {
    let value = q
        .get("metadata")
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or(json!({}));
    if !value.is_object() || serde_json::to_vec(&value)?.len() > 16 << 10 {
        return Err(Fault::invalid(
            "metadata must be a JSON object of at most 16 KiB",
        ));
    }
    if contains_nul(&value) {
        return Err(Fault::invalid("metadata must not contain U+0000"));
    }
    Ok(value)
}
/// Reports whether any string value or object key holds U+0000. PostgreSQL
/// `jsonb` cannot store that character, so it is rejected as invalid input
/// instead of surfacing as a persistence failure. The scan is iterative and
/// the nesting depth is already bounded by the 16 KiB size limit.
fn contains_nul(value: &Value) -> bool {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(s) if s.contains('\0') => return true,
            Value::Array(items) => pending.extend(items),
            Value::Object(map) => {
                for (key, item) in map {
                    if key.contains('\0') {
                        return true;
                    }
                    pending.push(item);
                }
            }
            _ => {}
        }
    }
    false
}
pub fn mask(q: &Value) -> Vec<&str> {
    string(q, "updateMask")
        .split(',')
        .filter(|s| !s.is_empty())
        .collect()
}
pub fn page(q: &Value) -> Result<(i64, Option<DateTime<Utc>>, uuid::Uuid)> {
    let size = match integer(q, "pageSize") {
        0 => 50,
        n if (1..=100).contains(&n) => n,
        _ => return Err(Fault::invalid("page_size must be 1 to 100")),
    };
    let token = string(q, "pageToken");
    if token.is_empty() {
        return Ok((size, None, uuid::Uuid::nil()));
    }
    let invalid = || Fault::invalid("page_token is invalid");
    let raw = URL_SAFE_NO_PAD.decode(token).map_err(|_| invalid())?;
    let raw = std::str::from_utf8(&raw).map_err(|_| invalid())?;
    let (micro, id) = raw.split_once('.').ok_or_else(invalid)?;
    let micro: i64 = micro.parse().map_err(|_| invalid())?;
    if !(0..=253402300799999999).contains(&micro) || !canonical_id(id) {
        return Err(invalid());
    }
    Ok((
        size,
        Some(DateTime::from_timestamp_micros(micro).ok_or_else(invalid)?),
        server_common::uuid(id)?,
    ))
}
pub fn cursor(at: DateTime<Utc>, id: uuid::Uuid) -> String {
    URL_SAFE_NO_PAD.encode(format!("{}.{id}", at.timestamp_micros()))
}
/// Validates the fields that identify a replay record: the idempotency key of a mutation. This
/// must pass before any replay lookup. Everything else in the request is payload and is checked
/// by `validate_payload`.
pub fn validate_routing(op: Op, q: &Value) -> Result<()> {
    if op.mutation() {
        let key = string(q, "idempotencyKey");
        if !(8..=128).contains(&key.len())
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
        {
            return Err(Fault::invalid("invalid idempotency_key"));
        }
    }
    Ok(())
}
/// Validates the non-routing payload of a request. For mutations the caller may defer this result
/// until after the replay lookup, because a matching scrubbed replay record is decided whatever
/// the payload contains.
pub fn validate_payload(op: Op, q: &Value) -> Result<()> {
    match op {
        Op::CreateProject | Op::UpdateProject => identifier(string(q, "name"))?,
        Op::GetApiKey | Op::RevokeApiKey => {
            if !canonical_id(string(q, "apiKeyId")) {
                return Err(Fault::invalid("api_key_id must be a canonical UUID v7"));
            }
        }
        Op::GetEndUser | Op::DeleteEndUser | Op::UpdateEndUser => {
            if !canonical_id(string(q, "endUserId")) {
                return Err(Fault::invalid("end_user_id must be a canonical UUID v7"));
            }
        }
        Op::LookupEndUser => identifier(string(q, "externalId"))?,
        Op::ListProjects | Op::ListApiKeys | Op::ListEndUsers => {
            page(q)?;
        }
        Op::IssueApiKey => {
            if !matches!(
                string(q, "permission"),
                "API_KEY_PERMISSION_READ_ONLY" | "API_KEY_PERMISSION_CRUD"
            ) {
                return Err(Fault::invalid("permission must be READ_ONLY or CRUD"));
            }
            if !string(q, "expireTime").is_empty() {
                DateTime::parse_from_rfc3339(string(q, "expireTime"))
                    .map_err(|_| Fault::invalid("expire_time is invalid"))?;
            }
        }
        _ => {}
    }
    if matches!(op, Op::CreateEndUser | Op::UpdateEndUser) {
        let fields = if op == Op::CreateEndUser {
            vec!["externalId", "name", "email", "metadata"]
        } else {
            let fields = mask(q);
            if fields.is_empty()
                || fields
                    .iter()
                    .any(|s| !matches!(*s, "externalId" | "name" | "email" | "metadata"))
            {
                return Err(Fault::invalid("invalid update_mask"));
            }
            fields
        };
        for field in fields {
            match field {
                "externalId" => identifier(string(q, field))?,
                "name" => {
                    let s = string(q, field);
                    if s.len() > 256 || s.contains('\0') {
                        return Err(Fault::invalid("name must be at most 256 bytes"));
                    }
                }
                "email" => {
                    let s = string(q, field);
                    if !s.is_empty()
                        && (s.len() > 254
                            || s.chars().any(|c| " \t\r\n<>,;\0".contains(c))
                            || !email_address::EmailAddress::is_valid(s))
                    {
                        return Err(Fault::invalid(
                            "email must be a valid address of at most 254 bytes",
                        ));
                    }
                }
                "metadata" => {
                    metadata(q)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routing_checks_only_the_idempotency_key() {
        let bad_key = json!({"idempotencyKey":"short"});
        assert!(validate_routing(Op::CreateEndUser, &bad_key).is_err());
        assert!(validate_routing(Op::GetEndUser, &bad_key).is_ok());
        // A payload that fails validation does not fail routing.
        let q = json!({"idempotencyKey":"user-create","externalId":""});
        assert!(validate_routing(Op::CreateEndUser, &q).is_ok());
        assert!(validate_payload(Op::CreateEndUser, &q).is_err());
    }
    #[test]
    fn payload_rejects_each_invalid_non_routing_field() {
        let key = "user-update";
        let long = "a".repeat(257);
        for (op, q) in [
            (
                Op::CreateEndUser,
                json!({"idempotencyKey":key,"externalId":""}),
            ),
            (Op::CreateProject, json!({"idempotencyKey":key,"name":""})),
            (
                Op::UpdateEndUser,
                json!({"idempotencyKey":key,"endUserId":"x","updateMask":"name"}),
            ),
            (
                Op::UpdateEndUser,
                json!({"idempotencyKey":key,"endUserId":uuid::Uuid::now_v7().to_string(),"updateMask":""}),
            ),
            (
                Op::UpdateEndUser,
                json!({"idempotencyKey":key,"endUserId":uuid::Uuid::now_v7().to_string(),"updateMask":"unknown"}),
            ),
            (
                Op::UpdateEndUser,
                json!({"idempotencyKey":key,"endUserId":uuid::Uuid::now_v7().to_string(),"updateMask":"name","name":long}),
            ),
            (
                Op::CreateEndUser,
                json!({"idempotencyKey":key,"externalId":"e","metadata":{"k":"v".repeat(17 << 10)}}),
            ),
        ] {
            assert!(validate_routing(op, &q).is_ok(), "{op:?} {q}");
            assert!(validate_payload(op, &q).is_err(), "{op:?} {q}");
        }
        let valid = json!({"idempotencyKey":key,"externalId":"e"});
        assert!(validate_payload(Op::CreateEndUser, &valid).is_ok());
    }
    /// Fourth-group leading nibbles 0-7 and c-f are version-7 text without the RFC 4122 variant.
    fn variant_vector(n: u8) -> String {
        format!("01900000-0000-7000-{n:x}000-000000000001")
    }
    #[test]
    fn resource_ids_enforce_the_rfc_variant_with_the_existing_message() {
        for n in 0..16u8 {
            let id = variant_vector(n);
            let rfc = (8..12).contains(&n);
            for (op, field, message) in [
                (
                    Op::GetApiKey,
                    "apiKeyId",
                    "api_key_id must be a canonical UUID v7",
                ),
                (
                    Op::RevokeApiKey,
                    "apiKeyId",
                    "api_key_id must be a canonical UUID v7",
                ),
                (
                    Op::GetEndUser,
                    "endUserId",
                    "end_user_id must be a canonical UUID v7",
                ),
                (
                    Op::DeleteEndUser,
                    "endUserId",
                    "end_user_id must be a canonical UUID v7",
                ),
            ] {
                let q = json!({ field: id });
                match validate_payload(op, &q) {
                    Ok(()) => assert!(rfc, "{op:?} {id}"),
                    Err(e) => {
                        assert!(!rfc, "{op:?} {id}");
                        assert_eq!(e.code, connectrpc::ErrorCode::InvalidArgument);
                        assert_eq!(e.message, message);
                    }
                }
            }
        }
    }
    #[test]
    fn page_token_ids_enforce_the_rfc_variant() {
        for n in 0..16u8 {
            let id = variant_vector(n);
            let token = cursor_for(&id);
            let q = json!({ "pageToken": token });
            match page(&q) {
                Ok((_, at, parsed)) => {
                    assert!((8..12).contains(&n), "{id}");
                    assert_eq!(parsed.to_string(), id);
                    assert!(at.is_some());
                }
                Err(e) => {
                    assert!(!(8..12).contains(&n), "{id}");
                    assert_eq!(e.code, connectrpc::ErrorCode::InvalidArgument);
                    assert_eq!(e.message, "page_token is invalid");
                }
            }
        }
        // A server-issued cursor keeps working.
        let issued = cursor(Utc::now(), uuid::Uuid::now_v7());
        assert!(page(&json!({ "pageToken": issued })).is_ok());
    }
    fn cursor_for(id: &str) -> String {
        URL_SAFE_NO_PAD.encode(format!("0.{id}"))
    }
    fn check(metadata: Value) -> Result<Value> {
        metadata_of(json!({ "metadata": metadata }))
    }
    fn metadata_of(q: Value) -> Result<Value> {
        super::metadata(&q)
    }
    #[test]
    fn rejects_nul_in_values_and_keys_at_any_depth() {
        for fixture in [
            json!({"value":"\u{0}"}),
            json!({"\u{0}":"value"}),
            json!({"nested":{"value":"a\u{0}b"}}),
            json!({"nested":{"a\u{0}b":true}}),
            json!({"items":[0,{"value":"\u{0}"}]}),
            json!({"items":[{"\u{0}":null}]}),
        ] {
            let err = check(fixture.clone()).expect_err("NUL must be rejected");
            let message = format!("{err:?}");
            assert!(message.contains("U+0000"), "{fixture}: {message}");
            assert!(!message.contains("value"), "{fixture}: {message}");
        }
    }
    #[test]
    fn accepts_storage_compatible_metadata() {
        let benign = json!({
            "value":"한글 😀",
            "literal":"\\u0000",
            "items":[null,true,3,{"key":"value"}]
        });
        assert_eq!(check(benign.clone()).unwrap(), benign);
        assert_eq!(metadata_of(json!({})).unwrap(), json!({}));
        assert_eq!(metadata_of(json!({"metadata":null})).unwrap(), json!({}));
        assert_eq!(check(json!({})).unwrap(), json!({}));
    }
    #[test]
    fn keeps_size_and_type_checks() {
        assert!(check(json!([1])).is_err());
        assert!(check(json!("text")).is_err());
        // `{"k":"<n>"}` serializes to n + 8 bytes.
        let at_limit = json!({"k":"a".repeat((16 << 10) - 8)});
        assert!(check(at_limit).is_ok());
        let over_limit = json!({"k":"a".repeat((16 << 10) - 7)});
        assert!(check(over_limit).is_err());
        // Oversized input keeps reporting the size message even with NUL.
        let oversized = json!({"k":format!("\u{0}{}", "a".repeat(16 << 10))});
        let message = format!("{:?}", check(oversized).unwrap_err());
        assert!(message.contains("16 KiB"), "{message}");
    }
}
