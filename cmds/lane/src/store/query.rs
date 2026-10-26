// SPDX-License-Identifier: Apache-2.0
use super::domain::{heads, mapped, result};
use super::{
    Connection, Failure, OptionalExtension, Result, Store, Value, invalid, json, params, rows, strv,
};
use crate::protocol::fingerprint;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
fn page(request: &Value, scope: &str) -> Result<(usize, String)> {
    let size = request["page"]["size"].as_u64().unwrap_or(0);
    if size > 1000 {
        return Err(invalid("Page size exceeds 1000."));
    }
    let cursor = strv(&request["page"], "cursor");
    let last = if cursor.is_empty() {
        String::new()
    } else {
        let bytes = URL_SAFE_NO_PAD
            .decode(cursor)
            .map_err(|_| invalid("Invalid cursor."))?;
        let v: Value = serde_json::from_slice(&bytes)?;
        if v["Scope"] != scope {
            return Err(invalid("Cursor does not match query."));
        }
        strv(&v, "Last").into()
    };
    Ok((
        if size == 0 {
            100
        } else {
            usize::try_from(size).map_err(|_| invalid("Invalid page size."))?
        },
        last,
    ))
}
fn cursor(scope: &str, last: &str) -> String {
    URL_SAFE_NO_PAD.encode(
        json!({
        "Scope":scope,"Last":last}
        )
        .to_string(),
    )
}
fn contains(request: &Value, k: &str, v: &Value) -> bool {
    request[k]
        .as_array()
        .is_none_or(|a| a.is_empty() || a.contains(v))
}
impl Store {
    pub(super) fn query(
        &self,
        connection: &Connection,
        method: &str,
        request: &Value,
    ) -> Result<Value> {
        match method {
            "GetTask" | "GetRepository" => {
                let id = if method == "GetRepository" {
                    mapped(connection, strv(request, "id"))?
                } else {
                    strv(request, "id").into()
                };
                let heads = heads(connection, &id)?;
                if heads[0]["kind"]
                    != if method == "GetTask" {
                        "ENTITY_KIND_TASK"
                    } else {
                        "ENTITY_KIND_REPOSITORY"
                    }
                {
                    return Err(Failure::new("NOT_FOUND", "Entity does not exist."));
                }
                Ok(json!({
                "result":result(&heads)}
                ))
            }
            "ListTasks" => self.list_tasks(connection, request),
            "GetPlanningItem" | "ListPlanningItems" => {
                self.planning_query(connection, method, request)
            }
            "ListRepositories" | "ListURLConnections" => {
                let mut query = request.clone();
                query.as_object_mut().unwrap().remove("page");
                let scope = format!(
                    "{}lane.v1.{}Request{}",
                    self.space,
                    method,
                    fingerprint(&query)
                );
                let (size, last) = page(request, &scope)?;
                let mut items = vec![];
                let mut prev = String::new();
                let mut next = String::new();
                for id in rows(
                    connection,
                    "SELECT id FROM entities WHERE kind=? AND deleted=0 AND id>? ORDER BY id",
                    params![if method == "ListURLConnections" { 3 } else { 1 }, last],
                )? {
                    if items.len() == size {
                        next = cursor(&scope, &prev);
                        break;
                    }
                    items.push(result(&heads(connection, &id)?));
                    prev = id;
                }
                Ok(json!({
                "items":items,"nextCursor":next}
                ))
            }
            "ListHistory" => {
                heads(connection, strv(request, "id"))?;
                let scope = format!(
                    "{}:{}:{}{}",
                    self.space,
                    method,
                    strv(request, "id"),
                    if request["promptsOnly"] == true {
                        ":prompts"
                    } else {
                        ""
                    }
                );
                let (size, last) = page(request, &scope)?;
                let mut versions = vec![];
                let mut prev = String::new();
                let mut next = String::new();
                let mut stmt=connection.prepare("SELECT envelope_json,prompt_version FROM versions WHERE entity_id=? ORDER BY created_at_us,version_id")?;
                for row in stmt.query_map([strv(request, "id")], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
                })? {
                    let (raw, prompt) = row?;
                    let v: Value = serde_json::from_str(&raw)?;
                    if v["deleted"] == true {
                        continue;
                    }
                    let time = chrono::DateTime::parse_from_rfc3339(strv(&v, "createdAt"))
                        .map_err(|_| invalid("Invalid history timestamp."))?
                        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
                    let key = format!("{time}:{}", strv(&v, "versionId"));
                    if key <= last || (request["promptsOnly"] == true && !prompt) {
                        continue;
                    }
                    if versions.len() == size {
                        next = cursor(&scope, &prev);
                        break;
                    }
                    versions.push(v);
                    prev = key;
                }
                Ok(json!({
                "versions":versions,"nextCursor":next}
                ))
            }
            "ListEvents" => {
                let mut query = request.clone();
                query.as_object_mut().unwrap().remove("page");
                let scope = format!("{}{method}{}", self.space, fingerprint(&query));
                let (size, last) = page(request, &scope)?;
                let mut items = vec![];
                let mut prev = String::new();
                let mut next = String::new();
                for raw in rows(
                    connection,
                    "SELECT body_json FROM remote_objects WHERE kind=2 AND deleted=0 AND id>? ORDER BY id",
                    [last],
                )? {
                    let v: Value = serde_json::from_str(&raw)?;
                    if (request.get("repositoryId").is_some()
                        && request["repositoryId"] != v["repositoryId"])
                        || !contains(request, "states", &v["state"])
                    {
                        continue;
                    }
                    if items.len() == size {
                        next = cursor(&scope, &prev);
                        break;
                    }
                    prev = strv(&v, "id").into();
                    items.push(v);
                }
                Ok(json!({
                "items":items,"nextCursor":next}
                ))
            }
            "GetEvent" => {
                let raw: Option<String> = connection
                    .query_row(
                        "SELECT body_json FROM remote_objects WHERE kind=2 AND id=? AND deleted=0",
                        [strv(request, "id")],
                        |r| r.get(0),
                    )
                    .optional()?;
                let event: Value =
                    serde_json::from_str(&raw.ok_or_else(|| {
                        Failure::new("NOT_FOUND", "Cached event does not exist.")
                    })?)?;
                let mut processing = vec![];
                for raw in rows(
                    connection,
                    "SELECT body_json FROM remote_objects WHERE kind=3 AND deleted=0 ORDER BY id",
                    [],
                )? {
                    let v: Value = serde_json::from_str(&raw)?;
                    if v["eventId"] == request["id"] {
                        processing.push(v);
                    }
                }
                Ok(json!({
                "event":event,"processing":processing}
                ))
            }
            _ => Err(Failure::new(
                "ONLINE_REQUIRED",
                "Use --remote after browser-approved login for this command.",
            )),
        }
    }
}
