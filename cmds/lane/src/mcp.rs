// SPDX-License-Identifier: Apache-2.0
use crate::{
    error::{Result, invalid},
    protocol::{self, strv},
};
use prost_reflect::{Kind, MessageDescriptor};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
fn object(props: Value) -> Value {
    json!({"type":"object","properties":props,"additionalProperties":false})
}
fn schema(m: MessageDescriptor) -> Value {
    if m.full_name() == "google.protobuf.Timestamp" {
        return json!({"type":"string","format":"date-time"});
    }
    let mut props = json!({});
    for f in m.fields() {
        if f.name() == "origin" {
            continue;
        }
        let mut s = match f.kind() {
            Kind::Message(m) => schema(m),
            Kind::Enum(e) => {
                json!({"type":"string","enum":e.values().map(|v|v.name().to_owned()).collect::<Vec<_>>()})
            }
            Kind::Bool => json!({"type":"boolean"}),
            Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
                json!({"type":"string","pattern":"^-?(0|[1-9][0-9]*)$"})
            }
            Kind::Uint64 | Kind::Fixed64 => json!({"type":"string","pattern":"^(0|[1-9][0-9]*)$"}),
            Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
                json!({"type":"integer","minimum":-2_147_483_648_i64,"maximum":2_147_483_647})
            }
            Kind::Uint32 | Kind::Fixed32 => {
                json!({"type":"integer","minimum":0,"maximum":4_294_967_295_u64})
            }
            Kind::Float | Kind::Double => json!({"type":"number"}),
            Kind::Bytes => json!({"type":"string","contentEncoding":"base64"}),
            Kind::String => json!({"type":"string"}),
        };
        if f.is_list() {
            s = json!({"type":"array","items":s});
        }
        props[f.json_name()] = s;
    }
    let mut s = object(props);
    let mut constraints = vec![];
    for oneof in m.oneofs() {
        let fields: Vec<_> = oneof.fields().collect();
        for (i, a) in fields.iter().enumerate() {
            for b in &fields[i + 1..] {
                constraints.push(json!({"not":{"required":[a.json_name(),b.json_name()]}}));
            }
        }
    }
    if !constraints.is_empty() {
        s["allOf"] = json!(constraints);
    }
    s
}
fn snake(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.extend(c.to_lowercase());
    }
    result
}
#[must_use]
pub fn tools() -> Vec<(String, Value)> {
    let mut out = vec![];
    for service in protocol::pool().services().filter(|s| {
        matches!(
            s.name(),
            "LaneService" | "AccountService" | "WebhookService"
        )
    }) {
        for m in service.methods() {
            let mut desc = format!("{} using the selected Lane target.", m.name());
            if service.name() != "LaneService" {
                desc.push_str(" Online-only; requires explicit remote selection and approval.");
            }
            if matches!(m.name(), "GetEvent" | "ListEvents") {
                desc = format!(
                    "{}; local selection reads retained cached events only.",
                    m.name()
                );
            }
            if m.name().starts_with("Delete") {
                desc.push_str(" Permanently deletes data; invocation confirms task, repository, receiver, or event deletion.");
            }
            if matches!(m.name(), "DeleteAccountData" | "BeginDataDeletion") {
                desc.push_str(" Account deletion requires human account confirmation and browser reauthentication; fabricated proof is forbidden.");
            }
            out.push((m.name().into(),json!({"name":format!("lane_{}",snake(m.name())),"description":desc,"inputSchema":schema(m.input())})));
        }
    }
    for (name, description, props, required) in [
        (
            "GetLocalStatus",
            "Read selected local space and device synchronization status.",
            json!({}),
            vec![],
        ),
        (
            "ConnectSync",
            "Start explicit browser approval. When reconnectRequired is true, a matching nonempty accountId confirms uploading retained data to the new dataset.",
            json!({"accountId":{"type":"string"}}),
            vec![],
        ),
        (
            "DisconnectSync",
            "Disconnect with an explicit retained-local-data choice.",
            json!({"retention":{"type":"string","enum":["LOCAL_RETENTION_KEEP_LOCAL","LOCAL_RETENTION_REMOVE_LOCAL"]}}),
            vec!["retention"],
        ),
        (
            "RemoveDeviceData",
            "Permanently remove the selected local space after coordinated handle shutdown; invocation confirms removal.",
            json!({}),
            vec![],
        ),
        (
            "GetNotificationPreferences",
            "Read this device's notification preferences and permission.",
            json!({}),
            vec![],
        ),
        (
            "SetNotificationPreferences",
            "Set this device's notification preferences.",
            json!({"enabled":{"type":"boolean"},"content":{"type":"string","enum":["NOTIFICATION_CONTENT_DETAIL","NOTIFICATION_CONTENT_GENERIC"]}}),
            vec!["enabled"],
        ),
    ] {
        let mut s = object(props);
        if !required.is_empty() {
            s["required"] = json!(required);
        }
        out.push((name.into(),json!({"name":format!("lane_{}",snake(name)),"description":description,"inputSchema":s})));
    }
    out
}
fn validate(s: &Value, v: &Value) -> Result<()> {
    let bad = || invalid("Arguments do not match the tool input schema.");
    match strv(s, "type") {
        "object" => {
            let m = v.as_object().ok_or_else(bad)?;
            for (k, v) in m {
                validate(s["properties"].get(k).ok_or_else(bad)?, v)?;
            }
            if let Some(required) = s["required"].as_array()
                && required
                    .iter()
                    .any(|k| !m.contains_key(k.as_str().unwrap_or("")))
            {
                return Err(bad());
            }
            if let Some(all) = s["allOf"].as_array() {
                for c in all {
                    if c["not"]["required"]
                        .as_array()
                        .is_some_and(|a| a.iter().all(|k| m.contains_key(k.as_str().unwrap_or(""))))
                    {
                        return Err(bad());
                    }
                }
            }
        }
        "array" => {
            for item in v.as_array().ok_or_else(bad)? {
                validate(&s["items"], item)?;
            }
        }
        "string" => {
            let text = v.as_str().ok_or_else(bad)?;
            if s["enum"].as_array().is_some_and(|a| !a.contains(v)) {
                return Err(bad());
            }
            if let Some(pattern) = s["pattern"].as_str() {
                let digits = if pattern.starts_with("^-?") {
                    text.strip_prefix('-').unwrap_or(text)
                } else {
                    text
                };
                if digits.is_empty()
                    || (digits.len() > 1 && digits.starts_with('0'))
                    || !digits.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(bad());
                }
            }
        }
        "boolean" => {
            if !v.is_boolean() {
                return Err(bad());
            }
        }
        "integer" | "number" => {
            let n = v.as_f64().ok_or_else(bad)?;
            if (s["type"] == "integer" && n.fract() != 0.0)
                || s["minimum"].as_f64().is_some_and(|min| n < min)
                || s["maximum"].as_f64().is_some_and(|max| n > max)
            {
                return Err(bad());
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn serve(
    input: &mut dyn BufRead,
    out: &mut (dyn Write + Send),
    invoke: impl Fn(&str, Value, Arc<AtomicBool>) -> Result<Value> + Sync,
) -> Result<()> {
    let tools = tools();
    let mut initialized = false;
    let output = Mutex::new(out);
    let active = Mutex::new(HashMap::<String, Arc<AtomicBool>>::new());
    let write = |value: Value| -> Result<()> {
        let mut out = output
            .lock()
            .map_err(|_| invalid("MCP output lock failed."))?;
        writeln!(out, "{value}")?;
        out.flush()?;
        drop(out);
        Ok(())
    };
    std::thread::scope(|scope| {
        let mut workers: Vec<std::thread::ScopedJoinHandle<'_, Result<()>>> = Vec::new();
        let result = (|| {
            loop {
                // Reap completed workers so a long-running MCP session keeps bounded state.
                let mut index = 0;
                while index < workers.len() {
                    if workers[index].is_finished() {
                        workers
                            .swap_remove(index)
                            .join()
                            .map_err(|_| invalid("MCP worker failed."))??;
                    } else {
                        index += 1;
                    }
                }
                let mut raw = Vec::new();
                let n =
                    std::io::Read::take(&mut *input, (4 << 20) + 1).read_until(b'\n', &mut raw)?;
                if n == 0 {
                    return Ok(());
                }
                if n > 4 << 20 {
                    return Err(invalid("MCP frame exceeds the size limit."));
                }
                if raw.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let Ok(req) = decode(&raw) else {
                    write(rpc_error(Value::Null, -32700, "Parse error"))?;
                    continue;
                };
                let id = req.get("id").cloned().unwrap_or(Value::Null);
                let method = strv(&req, "method");
                let valid_id = id.is_string() || id.as_i64().is_some();
                let result = if !req.is_object()
                    || req["jsonrpc"] != "2.0"
                    || method.is_empty()
                    || (req.get("id").is_some() && !valid_id)
                {
                    rpc_error(Value::Null, -32600, "Invalid Request")
                } else if req.get("id").is_none() {
                    if method == "notifications/cancelled" {
                        let key = req["params"]["requestId"].to_string();
                        if let Some(cancelled) = active.lock().unwrap().get(&key) {
                            cancelled.store(true, Ordering::Relaxed);
                        }
                    }
                    continue;
                } else if active.lock().unwrap().contains_key(&id.to_string()) {
                    rpc_error(id, -32600, "Request ID is already active")
                } else {
                    let p = &req["params"];
                    let result: std::result::Result<Value, (i32, &str)> = match method {
                        "initialize" => {
                            if initialized
                                || strv(p, "protocolVersion").is_empty()
                                || !p["capabilities"].is_object()
                                || !p["clientInfo"].is_object()
                            {
                                Err((-32602, "Invalid initialize parameters"))
                            } else {
                                initialized = true;
                                Ok(
                                    json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"lane","version":env!("CARGO_PKG_VERSION")}}),
                                )
                            }
                        }
                        "ping" => Ok(json!({})),
                        "tools/list" => {
                            if !initialized {
                                Err((-32600, "Initialize first"))
                            } else if !p.is_null()
                                && (!p.is_object() || !strv(p, "cursor").is_empty())
                            {
                                Err((-32602, "Invalid list parameters"))
                            } else {
                                Ok(json!({"tools":tools.iter().map(|(_,t)|t).collect::<Vec<_>>()}))
                            }
                        }
                        "tools/call" => {
                            if !initialized {
                                Err((-32600, "Initialize first"))
                            } else if let Some((method, tool)) =
                                tools.iter().find(|(_, t)| t["name"] == p["name"])
                            {
                                let q = p.get("arguments").cloned().unwrap_or_else(|| json!({}));
                                if validate(&tool["inputSchema"], &q).is_err()
                                    || protocol::method(method)
                                        .is_some_and(|_| protocol::request(method, &q).is_err())
                                {
                                    Err((-32602, "Invalid tool arguments"))
                                } else {
                                    let mut pending = active.lock().unwrap();
                                    if pending.len() >= 32 {
                                        Err((-32000, "Too many concurrent requests"))
                                    } else {
                                        let cancelled = Arc::new(AtomicBool::new(false));
                                        let key = id.to_string();
                                        pending.insert(key.clone(), cancelled.clone());
                                        drop(pending);
                                        let method = method.clone();
                                        let invoke = &invoke;
                                        let write = &write;
                                        let active = &active;
                                        workers.push(scope.spawn(move || {
                                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| invoke(&method, q, cancelled.clone())));
                                    let response = match result {
                                        Ok(result) => {
                                            let (value, failed) = match result {
                                                Ok(v) => (v, false),
                                                Err(e) => (json!({"error":e}), true),
                                            };
                                            json!({"jsonrpc":"2.0","id":id,"result":{"structuredContent":value,"content":[{"type":"text","text":value.to_string()}],"isError":failed}})
                                        }
                                        Err(_) => rpc_error(id, -32603, "Internal error"),
                                    };
                                    let mut pending = active.lock().unwrap();
                                    let result = if cancelled.load(Ordering::Relaxed) { Ok(()) } else { write(response) };
                                    pending.remove(&key);
                                    result
                                }));
                                        continue;
                                    }
                                }
                            } else {
                                Err((-32602, "Unknown tool"))
                            }
                        }
                        _ => Err((-32601, "Method not found")),
                    };
                    match result {
                        Ok(value) => json!({"jsonrpc":"2.0","id":id,"result":value}),
                        Err((code, msg)) => rpc_error(id, code, msg),
                    }
                };
                write(result)?;
            }
        })();
        if result.is_err() {
            for cancelled in active.lock().unwrap().values() {
                cancelled.store(true, Ordering::Relaxed);
            }
        }
        for worker in workers {
            worker.join().map_err(|_| invalid("MCP worker failed."))??;
        }
        result
    })
}
fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
/// Reject duplicate keys before validation so hidden origin fields cannot be discarded.
pub fn decode(raw: &[u8]) -> Result<Value> {
    use serde::Deserialize;
    let mut d = serde_json::Deserializer::from_slice(raw);
    let value = StrictValue::deserialize(&mut d)?;
    d.end()?;
    Ok(value.0)
}
struct StrictValue(Value);
impl<'de> serde::Deserialize<'de> for StrictValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = StrictValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON value without duplicate keys")
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                v: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = vec![];
                while let Some(v) = a.next_element::<StrictValue>()? {
                    values.push(v.0);
                }
                Ok(StrictValue(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if values.contains_key(&k) {
                        return Err(serde::de::Error::custom("duplicate object key"));
                    }
                    values.insert(k, a.next_value::<StrictValue>()?.0);
                }
                Ok(StrictValue(Value::Object(values)))
            }
        }
        d.deserialize_any(Visitor)
    }
}
