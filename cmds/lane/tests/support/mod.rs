// SPDX-License-Identifier: Apache-2.0
//! Shared helpers for the planning tests: fixture loading, seeding a store with fixture
//! entities and calling typed methods with a local CLI origin.
#![allow(
    dead_code,
    reason = "Each integration test binary uses a different subset of these helpers."
)]
use lane::{
    error::Result,
    protocol::{self, id, strv},
    store::Store,
};
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::HashMap;

pub fn fixture(name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/../../protos/lane/v1/fixtures/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap()
}
pub fn kind_number(kind: &str) -> i64 {
    match kind {
        "ENTITY_KIND_REPOSITORY" => 1,
        "ENTITY_KIND_TASK" => 2,
        "ENTITY_KIND_URL_CONNECTION" => 3,
        "ENTITY_KIND_LABEL" => 4,
        "ENTITY_KIND_MILESTONE" => 5,
        "ENTITY_KIND_CYCLE" => 6,
        other => panic!("unknown kind {other}"),
    }
}
/// Insert fixture entity heads directly. Returns entity ID to head version ID.
pub fn seed(store: &Store, entities: &Value) -> HashMap<String, String> {
    let mut heads = HashMap::new();
    store
        .with(true, |c| {
            for entity in entities["entities"].as_array().unwrap() {
                let entity_id = strv(entity, "entityId").to_owned();
                let version_id = id();
                let mut envelope = entity.clone();
                envelope["versionId"] = version_id.clone().into();
                envelope["parentVersionIds"] = json!([]);
                let deleted = entity["deleted"] == true;
                if !deleted {
                    envelope["createdAt"] = "2026-01-01T00:00:00.000000Z".into();
                    envelope["origin"] = json!({"kind":"ORIGIN_KIND_CLI","deviceId":store.device,"deviceName":"Fixture"});
                }
                let kind = strv(entity, "kind");
                let body = ["task", "label", "milestone", "cycle"]
                    .iter()
                    .find_map(|f| entity.get(*f).filter(|b| b.is_object()));
                let repository = body.and_then(|b| b["repositoryId"].as_str());
                let url = entity["repository"]["normalizedUrl"].as_str();
                c.execute(
                    "INSERT INTO entities(id,kind,repository_id,normalized_url,deleted) VALUES(?,?,?,?,?)",
                    params![entity_id, kind_number(kind), repository, url, deleted],
                )?;
                c.execute(
                    "INSERT INTO versions(version_id,entity_id,envelope_json,created_at_us) VALUES(?,?,?,1)",
                    params![version_id, entity_id, envelope.to_string()],
                )?;
                c.execute(
                    "INSERT INTO heads VALUES(?,?)",
                    params![entity_id, version_id],
                )?;
                heads.insert(entity_id, version_id);
            }
            Ok(())
        })
        .unwrap();
    heads
}
pub fn origin(store: &Store) -> Value {
    json!({"kind":"ORIGIN_KIND_CLI","deviceId":store.device,"deviceName":"Lane","accountId":store.identity().unwrap().account})
}
/// Call a typed method as the local CLI, filling meta (a fresh request ID when absent).
pub fn call(store: &Store, method: &str, request: &Value) -> Result<Value> {
    let mut q = protocol::request(method, request)?;
    protocol::attribute(
        protocol::method(method).unwrap().input(),
        &mut q,
        &origin(store),
        &mut vec![],
    )?;
    store.invoke(method, &q)
}
/// Normalized comparison form of a message body.
pub fn norm(message: &str, value: &Value) -> Value {
    protocol::message(message, value).unwrap()
}
