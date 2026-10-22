// SPDX-License-Identifier: FSL-1.1-ALv2
//! Durable event admission and asynchronous materialization. The caller owns the transaction.
mod filters;
mod ingest;
mod process;
use crate::{Fault, Result, metering};
use chrono::{DateTime, Duration, Utc};
use connectrpc::ErrorCode;
pub use filters::*;
pub use ingest::*;
pub use process::*;
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
pub const MAX_KEY_BYTES: usize = 200;
pub const MAX_TYPE_BYTES: usize = 100;
pub const MAX_ATTRIBUTES: usize = 100;
pub const MAX_ATTRIBUTE_NAME_BYTES: usize = 100;
pub const MAX_ATTRIBUTE_VALUE_BYTES: usize = 1024;
pub const MAX_NUMBER_BYTES: usize = 64;
pub const MAX_GROUP_ATTRIBUTES: usize = 20;
pub const JOB_KIND: &str = "metering";
pub const COMPATIBILITY_ATTRIBUTE: &str = "quantity";
pub const COMPATIBILITY_AGGREGATION: i32 = 2;
#[derive(Clone, Copy, Debug)]
pub enum Reason {
    InvalidInput,
    InvalidAttribute,
    NoMatchingMeter,
    CardinalityExceeded,
    IdentityConflict,
    StaleRevision,
    NotFound,
    Cancelled,
    WindowClosed,
    Forbidden,
    PeriodUnavailable,
}
#[derive(Clone, Copy, Debug)]
pub enum Limit {
    None,
    Groups,
    DistinctValues,
}
pub fn reject(reason: Reason, message: &str) -> Fault {
    let (code, reason) = match reason {
        Reason::InvalidInput => (ErrorCode::InvalidArgument, 1),
        Reason::InvalidAttribute => (ErrorCode::InvalidArgument, 9),
        Reason::NoMatchingMeter => (ErrorCode::InvalidArgument, 8),
        Reason::CardinalityExceeded => (ErrorCode::ResourceExhausted, 10),
        Reason::IdentityConflict => (ErrorCode::AlreadyExists, 11),
        Reason::StaleRevision => (ErrorCode::Aborted, 7),
        Reason::NotFound => (ErrorCode::NotFound, 12),
        Reason::Cancelled => (ErrorCode::FailedPrecondition, 13),
        Reason::WindowClosed => (ErrorCode::FailedPrecondition, 14),
        Reason::Forbidden => (ErrorCode::PermissionDenied, 3),
        Reason::PeriodUnavailable => (ErrorCode::FailedPrecondition, 5),
    };
    Fault::new(code, reason, message)
}
pub fn reject_limit(limit: Limit, message: &str) -> Fault {
    tracing::warn!(?limit, "meter cardinality rejected");
    {
        let mut fault = reject(Reason::CardinalityExceeded, message);
        fault.limit = match limit {
            Limit::None => None,
            Limit::Groups => Some(crate::telemetry::LimitKind::Groups),
            Limit::DistinctValues => Some(crate::telemetry::LimitKind::DistinctValues),
        };
        fault
    }
}
pub fn permanent(message: &str) -> Fault {
    let mut f = Fault::new(ErrorCode::Internal, 0, message);
    f.permanent = true;
    f
}
pub fn group_digest(key: &str) -> Vec<u8> {
    Sha256::digest(key.as_bytes()).to_vec()
}
#[derive(Clone, Debug)]
pub struct Policy {
    pub lateness: Duration,
    pub reporting_grace: Duration,
    pub limits: metering::Limits,
}
#[derive(Clone, Debug)]
pub struct Input {
    pub tenant_id: String,
    pub service_id: String,
    pub key: String,
    pub event_type: String,
    pub occurred_at: DateTime<Utc>,
    pub attributes: BTreeMap<String, metering::Value>,
    /// Numeric attribute text exactly as the caller reported it, before `attributes`
    /// canonicalized the numbers. Only events that reach a call-time meter are
    /// checked against it.
    pub reported_numbers: BTreeMap<String, String>,
}
#[derive(Clone, Debug)]
pub struct Correction {
    pub tenant_id: String,
    pub service_id: String,
    pub key: String,
    pub expected_revision: i32,
    pub kind: metering::RevisionKind,
    pub event_type: String,
    pub attributes: BTreeMap<String, metering::Value>,
    /// See [`Input::reported_numbers`].
    pub reported_numbers: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Default)]
pub struct Accepted {
    pub event_id: String,
    pub revision: i32,
    pub replayed: bool,
    pub meters: usize,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id: String,
    pub tenant_id: String,
    pub service_id: String,
    pub key: String,
    pub event_type: String,
    pub occurred_at: DateTime<Utc>,
    pub attributes: BTreeMap<String, metering::Value>,
    pub revision: i32,
    pub applied_revision: i32,
    pub cancelled: bool,
}
pub fn validate_content(
    event_type: &str,
    attributes: &BTreeMap<String, metering::Value>,
) -> Result<()> {
    if event_type.is_empty() || event_type.len() > MAX_TYPE_BYTES {
        return Err(reject(
            Reason::InvalidInput,
            "event type must contain 1 to 100 bytes",
        ));
    }
    if attributes.len() > MAX_ATTRIBUTES {
        return Err(reject(
            Reason::InvalidAttribute,
            "an event has at most 100 attributes",
        ));
    }
    for (name, v) in attributes {
        if name.is_empty() || name.len() > MAX_ATTRIBUTE_NAME_BYTES {
            return Err(reject(
                Reason::InvalidAttribute,
                "attribute names must contain 1 to 100 bytes",
            ));
        }
        let valid = match v.kind {
            metering::ValueKind::String => v.text.len() <= MAX_ATTRIBUTE_VALUE_BYTES,
            metering::ValueKind::Boolean => v.text == "true" || v.text == "false",
            metering::ValueKind::Number => {
                v.text.len() <= MAX_NUMBER_BYTES
                    && metering::Value::number(&v.text).is_ok_and(|n| n.text == v.text)
            }
        };
        if !valid {
            return Err(reject(Reason::InvalidAttribute, "invalid attribute scalar"));
        }
    }
    Ok(())
}
pub fn encode_values(values: &BTreeMap<String, metering::Value>) -> Result<Json> {
    Ok(Json::Object(
        values
            .iter()
            .map(|(k, v)| (k.clone(), json!({"kind":v.kind,"value":v.text})))
            .collect(),
    ))
}
pub fn decode_values(raw: &Json) -> Result<BTreeMap<String, metering::Value>> {
    raw.as_object()
        .ok_or_else(|| permanent("stored values must be an object"))?
        .iter()
        .map(|(k, v)| {
            let kind = serde_json::from_value(v["kind"].clone())
                .map_err(|_| permanent("stored value kind"))?;
            let text = v["value"]
                .as_str()
                .ok_or_else(|| permanent("stored scalar value"))?
                .to_owned();
            Ok((k.clone(), metering::Value { kind, text }))
        })
        .collect()
}
fn canonical_attributes(values: &BTreeMap<String, metering::Value>) -> Vec<Json> {
    values
        .iter()
        .map(|(k, v)| json!([k, v.kind, v.text]))
        .collect()
}
// Match Go encoding/json escaping so persisted replay hashes survive the port.
pub(super) fn go_json(parts: &Json) -> String {
    serde_json::to_string(parts)
        .expect("JSON values serialize")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}
fn fingerprint(parts: Json) -> String {
    hex::encode(Sha256::digest(go_json(&parts).as_bytes()))
}
pub fn event_fingerprint(input: &Input) -> String {
    let timestamp = input
        .occurred_at
        .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
    // RFC3339Nano omits insignificant trailing fractional zeroes.
    let timestamp = if let Some((a, b)) = timestamp.split_once('.') {
        format!("{}.{}Z", a, b.trim_end_matches('Z').trim_end_matches('0'))
    } else {
        timestamp
    };
    fingerprint(json!([
        "event",
        input.tenant_id,
        input.service_id,
        input.key,
        input.event_type,
        timestamp,
        canonical_attributes(&input.attributes)
    ]))
}
pub fn correction_fingerprint(event_id: &str, c: &Correction) -> String {
    fingerprint(json!([
        "correction",
        event_id,
        c.expected_revision,
        c.kind,
        c.event_type,
        canonical_attributes(&c.attributes)
    ]))
}
pub fn compatibility_definition(key: &str) -> metering::Definition {
    metering::Definition {
        key: key.into(),
        event_type: key.into(),
        aggregation: metering::Aggregation::Sum,
        value_attribute: COMPATIBILITY_ATTRIBUTE.into(),
        filters: vec![],
        group_by: vec![],
        value_source: metering::ValueSource::Attribute,
        markup: String::new(),
    }
}
pub fn is_compatibility_definition(d: &metering::Definition) -> bool {
    d.event_type == d.key
        && d.aggregation == metering::Aggregation::Sum
        && d.value_attribute == COMPATIBILITY_ATTRIBUTE
        && d.filters.is_empty()
        && d.group_by.is_empty()
        && d.value_source == metering::ValueSource::Attribute
}
pub async fn untyped_meter_in_use(tx: &mut sqlx::PgConnection, meter_id: &str) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM metering_events WHERE $1::text::uuid=ANY(meter_ids))",
    )
    .bind(meter_id)
    .fetch_one(tx)
    .await?)
}
pub fn decode_meter(key: &str, raw: &Json) -> Result<metering::Definition> {
    let text = |name| raw[name].as_str().unwrap_or("");
    if text("event_type").is_empty() {
        return Ok(compatibility_definition(key));
    }
    catalog_definition(
        text("name"),
        text("event_type"),
        raw["aggregation"].as_i64().unwrap_or(0) as i32,
        text("value_attribute"),
        raw["filters"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        &raw["group_by"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default(),
        raw["value_source"].as_i64().unwrap_or(0) as i32,
        text("markup"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_and_stored_values() {
        let values = BTreeMap::from([
            ("region".into(), metering::Value::string("eu")),
            ("n".into(), metering::Value::number("-1.5").unwrap()),
            ("ok".into(), metering::Value::boolean(true)),
        ]);
        validate_content("use", &values).unwrap();
        assert_eq!(
            decode_values(&encode_values(&values).unwrap()).unwrap(),
            values
        );
        assert_eq!(encode_values(&BTreeMap::new()).unwrap(), json!({}));
        assert!(
            decode_values(&json!({"x":{"kind":"object","value":"{}"}}))
                .unwrap_err()
                .permanent
        );
        assert!(validate_content("", &values).is_err());
        for (name, value) in [
            ("".to_string(), metering::Value::string("x")),
            ("n".repeat(101), metering::Value::string("x")),
            ("s".into(), metering::Value::string("v".repeat(1025))),
            (
                "b".into(),
                metering::Value {
                    kind: metering::ValueKind::Boolean,
                    text: "yes".into(),
                },
            ),
            (
                "n".into(),
                metering::Value {
                    kind: metering::ValueKind::Number,
                    text: "1.0".into(),
                },
            ),
        ] {
            assert_eq!(
                validate_content("use", &BTreeMap::from([(name, value)]))
                    .unwrap_err()
                    .reason,
                9
            );
        }
    }
    #[test]
    fn persisted_fingerprint_matches_go_rfc3339_and_json() {
        let input = Input {
            tenant_id: "t".into(),
            service_id: "s".into(),
            key: "k".into(),
            event_type: "use".into(),
            occurred_at: "2026-01-02T03:04:05.000006Z".parse().unwrap(),
            attributes: BTreeMap::from([
                ("a".into(), metering::Value::number("1.50").unwrap()),
                ("b".into(), metering::Value::string("<&>\u{2028}")),
            ]),
            reported_numbers: BTreeMap::new(),
        };
        let go_json = r#"["event","t","s","k","use","2026-01-02T03:04:05.000006Z",[["a","number","1.5"],["b","string","\u003c\u0026\u003e\u2028"]]]"#;
        assert_eq!(
            event_fingerprint(&input),
            hex::encode(Sha256::digest(go_json.as_bytes()))
        );
        let mut changed = input.clone();
        changed
            .attributes
            .insert("a".into(), metering::Value::string("1.5"));
        assert_ne!(event_fingerprint(&input), event_fingerprint(&changed));
    }
    #[test]
    fn untyped_compatibility_is_immutable() {
        let d = decode_meter("units", &Json::Null).unwrap();
        assert!(is_compatibility_definition(&d));
        let event = metering::Event {
            event_type: "units".into(),
            attributes: BTreeMap::from([(
                "quantity".into(),
                metering::Value::number("4").unwrap(),
            )]),
        };
        let matches = metering::match_event(&[d], &event).unwrap();
        assert_eq!(matches[0].group_key, "[]");
    }
}
