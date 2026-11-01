// SPDX-License-Identifier: Apache-2.0
//! Rust bindings generated from `protos/anydash/v1/anydash.proto` by `build.rs`.
#![allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    missing_docs,
    reason = "Generated protocol code follows prost conventions."
)]
include!(concat!(env!("OUT_DIR"), "/anydash.v1.rs"));
/// Descriptors for canonical Protobuf JSON output.
pub fn pool() -> &'static prost_reflect::DescriptorPool {
    static POOL: std::sync::OnceLock<prost_reflect::DescriptorPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        prost_reflect::DescriptorPool::decode(
            include_bytes!(concat!(env!("OUT_DIR"), "/anydash.bin")).as_ref(),
        )
        .expect("compiled AnyDash descriptors")
    })
}
/// Canonical Protobuf JSON: enum value names, camelCase fields and decimal 64-bit strings.
pub fn to_json<M: prost::Message>(message: &M, name: &str) -> serde_json::Value {
    let Some(descriptor) = pool().get_message_by_name(&format!("anydash.v1.{name}")) else {
        return serde_json::Value::Null;
    };
    let Ok(dynamic) =
        prost_reflect::DynamicMessage::decode(descriptor, message.encode_to_vec().as_slice())
    else {
        return serde_json::Value::Null;
    };
    let mut buffer = vec![];
    let mut serializer = serde_json::Serializer::new(&mut buffer);
    if dynamic
        .serialize_with_options(
            &mut serializer,
            &prost_reflect::SerializeOptions::new().skip_default_fields(false),
        )
        .is_err()
    {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&buffer).unwrap_or_default()
}
