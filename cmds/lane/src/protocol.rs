// SPDX-License-Identifier: Apache-2.0
use crate::error::{Result, invalid};
use prost_reflect::{
    DescriptorPool, DynamicMessage, MessageDescriptor, MethodDescriptor, SerializeOptions,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
#[must_use]
pub fn id() -> String {
    uuid::Uuid::now_v7().to_string()
}
#[must_use]
pub fn valid_id(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|v| v.get_version_num() == 7 && v.to_string() == s)
}
pub fn pool() -> &'static DescriptorPool {
    static POOL: OnceLock<DescriptorPool> = OnceLock::new();
    POOL.get_or_init(|| {
        DescriptorPool::decode(include_bytes!(concat!(env!("OUT_DIR"), "/lane.bin")).as_ref())
            .expect("compiled Lane descriptors")
    })
}
#[must_use]
pub fn method(name: &str) -> Option<MethodDescriptor> {
    pool()
        .services()
        .flat_map(|s| s.methods().collect::<Vec<_>>())
        .find(|m| m.name() == name)
}
pub fn typed(desc: MessageDescriptor, value: &Value, defaults: bool) -> Result<Value> {
    let msg =
        DynamicMessage::deserialize(desc, value).map_err(|_| invalid("Invalid typed request."))?;
    let mut buf = Vec::new();
    msg.serialize_with_options(
        &mut serde_json::Serializer::new(&mut buf),
        &SerializeOptions::new().skip_default_fields(!defaults),
    )
    .map_err(|_| invalid("Invalid typed message."))?;
    Ok(serde_json::from_slice(&buf)?)
}
pub fn request(name: &str, value: &Value) -> Result<Value> {
    typed(
        method(name)
            .ok_or_else(|| invalid("Unknown typed method."))?
            .input(),
        value,
        true,
    )
}
pub fn message(name: &str, value: &Value) -> Result<Value> {
    typed(
        pool()
            .get_message_by_name(&format!("lane.v1.{name}"))
            .ok_or_else(|| invalid("Unknown message."))?,
        value,
        true,
    )
}
pub fn attribute(
    desc: MessageDescriptor,
    value: &mut Value,
    origin: &Value,
    allocated: &mut Vec<String>,
) -> Result<()> {
    if desc.get_field_by_name("meta").is_some() {
        if !value["meta"].is_object() {
            value["meta"] = json!({});
        }
        if strv(&value["meta"], "requestId").is_empty() {
            let id = id();
            value["meta"]["requestId"] = id.clone().into();
            allocated.push(id);
        }
        if !valid_id(strv(&value["meta"], "requestId")) {
            return Err(invalid("Request ID must be a lowercase UUID v7."));
        }
        value["meta"]["origin"] = origin.clone();
    }
    for f in desc.fields() {
        if f.name() == "meta" {
            continue;
        }
        if let prost_reflect::Kind::Message(d) = f.kind()
            && let Some(v) = value.get_mut(f.json_name())
        {
            if let Some(items) = v.as_array_mut() {
                for item in items {
                    attribute(d.clone(), item, origin, allocated)?;
                }
            } else if v.is_object() {
                attribute(d, v, origin, allocated)?;
            }
        }
    }
    Ok(())
}
pub fn strv<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}
pub fn hash(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}
/// RFC 8785 ordering uses UTF-16 code units, including for supplementary characters.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|k| format!("{}:{}", json!(k), canonical(&m[*k])))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        _ => v.to_string(),
    }
}
#[must_use]
pub fn fingerprint(v: &Value) -> String {
    hash(canonical(v))
}
