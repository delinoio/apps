// SPDX-License-Identifier: Apache-2.0
use super::domain::{heads, mapped, result};
use super::*;
use crate::protocol::fingerprint;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use unicode_casefold::UnicodeCaseFold;
fn page(q: &Value, scope: &str) -> Result<(usize, String)> {
    let n = q["page"]["size"].as_u64().unwrap_or(0);
    if n > 1000 {
        return Err(invalid("Page size exceeds 1000."));
    }
    let cursor = strv(&q["page"], "cursor");
    let mut last = String::new();
    if !cursor.is_empty() {
        let bytes = URL_SAFE_NO_PAD
            .decode(cursor)
            .map_err(|_| invalid("Invalid cursor."))?;
        let v: Value = serde_json::from_slice(&bytes)?;
        if v["Scope"] != scope {
            return Err(invalid("Cursor does not match query."));
        }
        last = strv(&v, "Last").into();
    }
    Ok((if n == 0 { 100 } else { n as usize }, last))
}
fn cursor(scope: &str, last: &str) -> String {
    URL_SAFE_NO_PAD.encode(json!({"Scope":scope,"Last":last}).to_string())
}
fn contains(q: &Value, k: &str, v: &Value) -> bool {
    q[k].as_array()
        .is_none_or(|a| a.is_empty() || a.contains(v))
}
impl Store {
    pub(super) fn query(&self, c: &Connection, method: &str, q: &Value) -> Result<Value> {
        match method {
            "GetTask" | "GetRepository" => {
                let id = if method == "GetRepository" {
                    mapped(c, strv(q, "id"))?
                } else {
                    strv(q, "id").into()
                };
                let h = heads(c, &id)?;
                if h[0]["kind"]
                    != if method == "GetTask" {
                        "ENTITY_KIND_TASK"
                    } else {
                        "ENTITY_KIND_REPOSITORY"
                    }
                {
                    return Err(Failure::new("NOT_FOUND", "Entity does not exist."));
                }
                Ok(json!({"result":result(&h)}))
            }
            "ListTasks" | "ListRepositories" => {
                for (key, valid) in [
                    (
                        "progress",
                        &[
                            "PROGRESS_TODO",
                            "PROGRESS_IN_PROGRESS",
                            "PROGRESS_BLOCKED",
                            "PROGRESS_DONE",
                            "PROGRESS_CANCELED",
                        ][..],
                    ),
                    (
                        "proposal",
                        &[
                            "PROPOSAL_REGULAR",
                            "PROPOSAL_PENDING",
                            "PROPOSAL_ACCEPTED",
                            "PROPOSAL_REJECTED",
                        ][..],
                    ),
                ] {
                    if let Some(a) = q[key].as_array() {
                        if a.iter().any(|v| !valid.contains(&v.as_str().unwrap_or(""))) {
                            return Err(invalid("Invalid state filter."));
                        }
                    }
                }
                let mut query = q.clone();
                query.as_object_mut().unwrap().remove("page");
                let scope = format!(
                    "{}lane.v1.{}Request{}",
                    self.space,
                    method,
                    fingerprint(&query)
                );
                let (n, last) = page(q, &scope)?;
                let mut items = vec![];
                let mut prev = String::new();
                let mut next = String::new();
                for id in rows(
                    c,
                    "SELECT id FROM entities WHERE kind=? AND deleted=0 AND id>? ORDER BY id",
                    params![if method == "ListTasks" { 2 } else { 1 }, last],
                )? {
                    let h = heads(c, &id)?;
                    let mut matches = method == "ListRepositories";
                    for v in &h {
                        let t = &v["task"];
                        if !t.is_object() {
                            continue;
                        }
                        let target = strv(q, "repositoryId");
                        if !target.is_empty()
                            && mapped(c, strv(t, "repositoryId"))? != mapped(c, target)?
                        {
                            continue;
                        }
                        if !contains(q, "progress", &t["progress"])
                            || !contains(q, "proposal", &t["proposal"])
                        {
                            continue;
                        }
                        let search = strv(q, "search").case_fold().collect::<String>();
                        if strv(t, "title")
                            .case_fold()
                            .collect::<String>()
                            .contains(&search)
                            || strv(t, "description")
                                .case_fold()
                                .collect::<String>()
                                .contains(&search)
                        {
                            matches = true;
                        }
                    }
                    if !matches {
                        continue;
                    }
                    if items.len() == n {
                        next = cursor(&scope, &prev);
                        break;
                    }
                    items.push(result(&h));
                    prev = id;
                }
                Ok(json!({"items":items,"nextCursor":next}))
            }
            "ListHistory" => {
                heads(c, strv(q, "id"))?;
                let scope = format!(
                    "{}:{}:{}{}",
                    self.space,
                    method,
                    strv(q, "id"),
                    if q["promptsOnly"] == true {
                        ":prompts"
                    } else {
                        ""
                    }
                );
                let (n, last) = page(q, &scope)?;
                let mut versions = vec![];
                let mut prev = String::new();
                let mut next = String::new();
                let mut stmt=c.prepare("SELECT envelope_json,prompt_version FROM versions WHERE entity_id=? ORDER BY created_at_us,version_id")?;
                for row in stmt.query_map([strv(q, "id")], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
                })? {
                    let (raw, prompt) = row?;
                    let v: Value = serde_json::from_str(&raw)?;
                    let time = chrono::DateTime::parse_from_rfc3339(strv(&v, "createdAt"))
                        .map_err(|_| invalid("Invalid history timestamp."))?
                        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
                    let key = format!("{time}:{}", strv(&v, "versionId"));
                    if key <= last || v["deleted"] == true || (q["promptsOnly"] == true && !prompt)
                    {
                        continue;
                    }
                    if versions.len() == n {
                        next = cursor(&scope, &prev);
                        break;
                    }
                    versions.push(v);
                    prev = key;
                }
                Ok(json!({"versions":versions,"nextCursor":next}))
            }
            "ListEvents" => {
                let mut query = q.clone();
                query.as_object_mut().unwrap().remove("page");
                let scope = format!("{}{method}{}", self.space, fingerprint(&query));
                let (n, last) = page(q, &scope)?;
                let mut items = vec![];
                let mut prev = String::new();
                let mut next = String::new();
                for raw in rows(
                    c,
                    "SELECT body_json FROM remote_objects WHERE kind=2 AND deleted=0 AND id>? ORDER BY id",
                    [last],
                )? {
                    let v: Value = serde_json::from_str(&raw)?;
                    if (q.get("repositoryId").is_some() && q["repositoryId"] != v["repositoryId"])
                        || !contains(q, "states", &v["state"])
                    {
                        continue;
                    }
                    if items.len() == n {
                        next = cursor(&scope, &prev);
                        break;
                    }
                    prev = strv(&v, "id").into();
                    items.push(v);
                }
                Ok(json!({"items":items,"nextCursor":next}))
            }
            "GetEvent" => {
                let raw: Option<String> = c
                    .query_row(
                        "SELECT body_json FROM remote_objects WHERE kind=2 AND id=? AND deleted=0",
                        [strv(q, "id")],
                        |r| r.get(0),
                    )
                    .optional()?;
                let event: Value =
                    serde_json::from_str(&raw.ok_or_else(|| {
                        Failure::new("NOT_FOUND", "Cached event does not exist.")
                    })?)?;
                let mut processing = vec![];
                for raw in rows(
                    c,
                    "SELECT body_json FROM remote_objects WHERE kind=3 AND deleted=0 ORDER BY id",
                    [],
                )? {
                    let v: Value = serde_json::from_str(&raw)?;
                    if v["eventId"] == q["id"] {
                        processing.push(v);
                    }
                }
                Ok(json!({"event":event,"processing":processing}))
            }
            _ => Err(Failure::new(
                "ONLINE_REQUIRED",
                "Use --remote after browser-approved login for this command.",
            )),
        }
    }
}
