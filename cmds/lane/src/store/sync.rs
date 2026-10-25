// SPDX-License-Identifier: Apache-2.0
use super::domain::{mapped, redact};
use super::*;
use crate::protocol;
use std::collections::{BTreeMap, HashSet};
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub fn validate(v: &Value) -> Result<()> {
    if v["formatVersion"] != 1 {
        return Err(Failure::new(
            "UNSUPPORTED_FORMAT",
            "Unsupported synchronization format.",
        ));
    }
    if !valid_id(strv(v, "entityId"))
        || !valid_id(strv(v, "versionId"))
        || !matches!(
            strv(v, "kind"),
            "ENTITY_KIND_TASK" | "ENTITY_KIND_REPOSITORY"
        )
    {
        return Err(invalid("Invalid entity identity."));
    }
    let mut prev = "";
    for p in array(&v["parentVersionIds"]) {
        let p = p.as_str().unwrap_or("");
        if !valid_id(p) || p == strv(v, "versionId") || (!prev.is_empty() && p <= prev) {
            return Err(invalid("Invalid parent identity or order."));
        }
        prev = p;
    }
    if v["promptRestoration"] == true && (v["deleted"] == true || v["kind"] != "ENTITY_KIND_TASK") {
        return Err(invalid("Invalid prompt restoration marker."));
    }
    if v["deleted"] == true {
        if v.get("task").is_some() || v.get("repository").is_some() {
            return Err(invalid("Deleted version has a body."));
        }
        return Ok(());
    }
    let time = chrono::DateTime::parse_from_rfc3339(strv(v, "createdAt"))
        .map_err(|_| invalid("Invalid entity timestamp."))?;
    if time.timestamp_subsec_nanos() % 1000 != 0 {
        return Err(invalid("Invalid entity timestamp."));
    }
    let o = &v["origin"];
    if !matches!(
        strv(o, "kind"),
        "ORIGIN_KIND_APP" | "ORIGIN_KIND_CLI" | "ORIGIN_KIND_MCP"
    ) || (!strv(o, "deviceId").is_empty() && !valid_id(strv(o, "deviceId")))
    {
        return Err(invalid("Invalid entity origin."));
    }
    if v["kind"] == "ENTITY_KIND_TASK" {
        let t = &v["task"];
        if !valid_id(strv(t, "repositoryId"))
            || strv(t, "title").trim().is_empty()
            || !matches!(
                strv(t, "progress"),
                "PROGRESS_TODO"
                    | "PROGRESS_IN_PROGRESS"
                    | "PROGRESS_BLOCKED"
                    | "PROGRESS_DONE"
                    | "PROGRESS_CANCELED"
            )
            || !matches!(
                strv(t, "proposal"),
                "PROPOSAL_REGULAR" | "PROPOSAL_PENDING" | "PROPOSAL_ACCEPTED" | "PROPOSAL_REJECTED"
            )
        {
            return Err(invalid("Invalid task body."));
        }
    } else if strv(&v["repository"], "normalizedUrl").is_empty() {
        return Err(invalid("Invalid repository body."));
    }
    Ok(())
}
pub fn mappings(c: &Connection, mappings: &Value) -> Result<()> {
    for m in array(mappings) {
        let local = strv(m, "localId");
        let canonical = strv(m, "canonicalId");
        if local.is_empty() || canonical.is_empty() || local == canonical {
            return Err(invalid("Invalid repository mapping."));
        }
        let old: Option<String> = c
            .query_row(
                "SELECT canonical_id FROM repository_mappings WHERE local_id=?",
                [local],
                |r| r.get(0),
            )
            .optional()?;
        if old.is_some_and(|v| v != canonical) {
            return Err(invalid("Changed repository mapping."));
        }
        c.execute(
            "INSERT OR IGNORE INTO repository_mappings VALUES(?,?)",
            params![local, canonical],
        )?;
        mapped(c, local)?;
        c.execute(
            "UPDATE entities SET normalized_url=NULL WHERE id=?",
            [local],
        )?;
        c.execute(
            "UPDATE entities SET repository_id=? WHERE repository_id=?",
            params![canonical, local],
        )?;
    }
    Ok(())
}
pub fn insert_version(c: &Connection, v: &Value) -> Result<()> {
    let v = protocol::message("EntityVersion", v)?;
    let version = strv(&v, "versionId");
    let entity = strv(&v, "entityId");
    let old: Option<String> = c
        .query_row(
            "SELECT envelope_json FROM versions WHERE version_id=?",
            [version],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(raw) = old {
        if protocol::message("EntityVersion", &serde_json::from_str(&raw)?)? != v {
            return Err(invalid("Immutable version changed."));
        }
        return Ok(());
    }
    let mut prompt = v["promptRestoration"] == true
        || (array(&v["parentVersionIds"]).is_empty() && v["task"].get("prompt").is_some());
    for parent in array(&v["parentVersionIds"]) {
        if v["deleted"] == true {
            break;
        }
        let parent = parent.as_str().unwrap_or("");
        let row: Option<(String, String)> = c
            .query_row(
                "SELECT entity_id,envelope_json FROM versions WHERE version_id=?",
                [parent],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (owner, raw) = row.ok_or_else(|| invalid("Missing version parent."))?;
        if owner != entity {
            return Err(invalid("Cross-entity parent."));
        }
        let previous: Value = serde_json::from_str(&raw)?;
        prompt |=
            v["task"].is_object() && previous["task"].get("prompt") != v["task"].get("prompt");
    }
    let us = chrono::DateTime::parse_from_rfc3339(strv(&v, "createdAt"))
        .map(|t| t.timestamp_micros())
        .unwrap_or(0);
    c.execute("INSERT INTO versions(version_id,entity_id,envelope_json,created_at_us,prompt_version) VALUES(?,?,?,?,?)",params![version,entity,v.to_string(),us,prompt])?;
    Ok(())
}
pub fn apply(c: &Connection, r: &Value) -> Result<()> {
    let v = &r["version"];
    validate(v)?;
    let entity = strv(v, "entityId");
    let deleted: Option<bool> = c
        .query_row("SELECT deleted FROM entities WHERE id=?", [entity], |r| {
            r.get(0)
        })
        .optional()?;
    if deleted == Some(true) && v["deleted"] != true {
        return Ok(());
    }
    if v["deleted"] == true {
        return delete_entity(c, v);
    }
    let repo = if v["task"].is_object() {
        Some(mapped(c, strv(&v["task"], "repositoryId"))?)
    } else {
        None
    };
    let mut normalized = v["repository"]["normalizedUrl"].as_str();
    if let Some(repo) = repo.as_ref() {
        let parent_deleted: Option<bool> = c
            .query_row(
                "SELECT deleted FROM entities WHERE id=? AND kind=1",
                [repo],
                |r| r.get(0),
            )
            .optional()?;
        if parent_deleted == Some(true) {
            let current: Option<Option<String>> = c
                .query_row(
                    "SELECT repository_id FROM entities WHERE id=?",
                    [entity],
                    |r| r.get(0),
                )
                .optional()?;
            if current.flatten().is_some_and(|v| v != *repo) {
                return Ok(());
            }
            return delete_entity(
                c,
                &json!({"formatVersion":1,"entityId":entity,"versionId":id(),"kind":"ENTITY_KIND_TASK","deleted":true}),
            );
        }
    }
    let aliases: i64 = c.query_row(
        "SELECT count(*) FROM repository_mappings WHERE local_id=?",
        [entity],
        |r| r.get(0),
    )?;
    if aliases > 0 {
        normalized = None;
    }
    c.execute(
        "INSERT OR IGNORE INTO entities(id,kind,repository_id,normalized_url) VALUES(?,?,?,?)",
        params![
            entity,
            if v["kind"] == "ENTITY_KIND_TASK" {
                2
            } else {
                1
            },
            repo,
            normalized
        ],
    )?;
    for version in std::iter::once(v).chain(array(&r["alternatives"]).iter()) {
        validate(version)?;
        if version["entityId"] != v["entityId"]
            || version["kind"] != v["kind"]
            || version["deleted"] == true
        {
            return Err(invalid("Invalid alternative."));
        }
        insert_version(c, version)?;
        c.execute(
            "INSERT OR IGNORE INTO heads VALUES(?,?)",
            params![entity, strv(version, "versionId")],
        )?;
    }
    let mut all = BTreeMap::new();
    for raw in rows(
        c,
        "SELECT envelope_json FROM versions WHERE entity_id=?",
        [entity],
    )? {
        let v: Value = serde_json::from_str(&raw)?;
        all.insert(strv(&v, "versionId").to_owned(), v);
    }
    let heads = rows(
        c,
        "SELECT version_id FROM heads WHERE entity_id=?",
        [entity],
    )?;
    let mut ancestors = HashSet::new();
    let mut pending: Vec<String> = heads
        .iter()
        .flat_map(|h| {
            array(&all[h]["parentVersionIds"])
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    while let Some(p) = pending.pop() {
        if ancestors.insert(p.clone()) {
            if let Some(v) = all.get(&p) {
                pending.extend(
                    array(&v["parentVersionIds"])
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned),
                );
            }
        }
    }
    for h in heads {
        if ancestors.contains(&h) {
            c.execute(
                "DELETE FROM heads WHERE entity_id=? AND version_id=?",
                params![entity, h],
            )?;
        }
    }
    let is_head: i64 = c.query_row(
        "SELECT count(*) FROM heads WHERE entity_id=? AND version_id=?",
        params![entity, strv(v, "versionId")],
        |r| r.get(0),
    )?;
    if is_head > 0 {
        c.execute(
            "UPDATE entities SET repository_id=?,normalized_url=? WHERE id=?",
            params![repo, normalized, entity],
        )?;
    }
    c.execute(
        "INSERT INTO changes(entity_id,change_kind) VALUES(?,1)",
        [entity],
    )?;
    Ok(())
}
pub fn delete_entity(c: &Connection, v: &Value) -> Result<()> {
    let entity = strv(v, "entityId");
    if v["kind"] == "ENTITY_KIND_REPOSITORY" {
        for child in rows(
            c,
            "SELECT id FROM entities WHERE kind=2 AND repository_id=? AND deleted=0",
            [entity],
        )? {
            delete_entity(
                c,
                &json!({"formatVersion":1,"entityId":child,"versionId":id(),"kind":"ENTITY_KIND_TASK","deleted":true}),
            )?;
        }
        delete_remote_children(c, 0, entity)?;
    }
    c.execute("INSERT INTO entities(id,kind,deleted) VALUES(?,?,1) ON CONFLICT(id) DO UPDATE SET deleted=1,repository_id=NULL,normalized_url=NULL",params![entity,if v["kind"]=="ENTITY_KIND_TASK"{2}else{1}])?;
    for sql in [
        "DELETE FROM heads WHERE entity_id=?",
        "DELETE FROM versions WHERE entity_id=?",
        "DELETE FROM outbox WHERE entity_id=?",
        "UPDATE notification_receipts SET state=3 WHERE proposal_id=?",
    ] {
        c.execute(sql, [entity])?;
    }
    c.execute(
        "UPDATE receipts SET result_json=? WHERE entity_id=?",
        params![
            json!({"result":"RESULT_KIND_DELETED","version":v}).to_string(),
            entity
        ],
    )?;
    insert_version(c, v)?;
    c.execute(
        "INSERT INTO heads VALUES(?,?)",
        params![entity, strv(v, "versionId")],
    )?;
    redact(c, entity, strv(v, "kind"))?;
    c.execute(
        "INSERT INTO changes(entity_id,change_kind) VALUES(?,1)",
        [entity],
    )?;
    Ok(())
}
pub fn change(c: &Connection, v: &Value, account: &str) -> Result<()> {
    if let Some(r) = v.get("entity") {
        return apply(c, r);
    }
    for (field, kind) in [("receiver", 1), ("event", 2), ("processing", 3)] {
        if let Some(body) = v.get(field) {
            let id = strv(body, "id");
            if id.is_empty() {
                return Err(invalid("Invalid remote identity."));
            }
            if body["deleted"] == true || body["state"] == "EVENT_STATE_DELETED" {
                return delete_remote(c, kind, id);
            }
            c.execute("INSERT INTO remote_objects(kind,id,body_json,deleted) VALUES(?,?,?,0) ON CONFLICT(kind,id) DO UPDATE SET body_json=excluded.body_json WHERE remote_objects.deleted=0",params![kind,id,body.to_string()])?;
            c.execute(
                "INSERT INTO changes(entity_id,change_kind) VALUES(?,2)",
                [id],
            )?;
            return Ok(());
        }
    }
    let d = &v["deletion"];
    let id = strv(d, "id");
    match strv(d, "kind") {
        "ENTITY_KIND_OR_REMOTE_KIND_REPOSITORY" | "ENTITY_KIND_OR_REMOTE_KIND_TASK" => {
            delete_entity(
                c,
                &json!({"formatVersion":1,"entityId":id,"versionId":d["deletionVersionId"],"kind":if d["kind"]=="ENTITY_KIND_OR_REMOTE_KIND_TASK"{"ENTITY_KIND_TASK"}else{"ENTITY_KIND_REPOSITORY"},"deleted":true}),
            )
        }
        "ENTITY_KIND_OR_REMOTE_KIND_RECEIVER" => delete_remote(c, 1, id),
        "ENTITY_KIND_OR_REMOTE_KIND_EVENT" => delete_remote(c, 2, id),
        "ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT" => {
            if id != account {
                return Err(Failure::new(
                    "FORBIDDEN",
                    "Account deletion target mismatch.",
                ));
            }
            c.execute_batch("DELETE FROM heads; DELETE FROM versions; DELETE FROM entities; DELETE FROM outbox; DELETE FROM remote_objects; DELETE FROM receipts; DELETE FROM repository_mappings; DELETE FROM notification_receipts;")?;
            put_meta(c, "account_deleted", "1")
        }
        _ => Err(invalid("Invalid synchronization change.")),
    }
}
fn delete_remote(c: &Connection, kind: i32, id: &str) -> Result<()> {
    if kind == 2 {
        delete_remote_children(c, 3, id)?;
    }
    c.execute("INSERT INTO remote_objects(kind,id,body_json,deleted) VALUES(?,?,NULL,1) ON CONFLICT(kind,id) DO UPDATE SET body_json=NULL,deleted=1",params![kind,id])?;
    Ok(())
}
fn delete_remote_children(c: &Connection, kind: i32, parent: &str) -> Result<()> {
    let mut stmt = c.prepare("SELECT kind,id,body_json FROM remote_objects WHERE deleted=0")?;
    let all = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (k, id, raw) in all {
        let v: Value = serde_json::from_str(&raw)?;
        if (kind == 0 && (k == 1 || k == 2) && v["repositoryId"] == parent)
            || (kind == 3 && k == 3 && v["eventId"] == parent)
        {
            delete_remote(c, k, &id)?;
        }
    }
    Ok(())
}
pub fn ordered(c: &Connection) -> Result<Vec<Value>> {
    let mut pending = rows(
        c,
        "SELECT envelope_json FROM versions ORDER BY created_at_us,version_id",
        [],
    )?
    .iter()
    .map(|raw| serde_json::from_str::<Value>(raw))
    .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut out = vec![];
    let mut seen = HashSet::new();
    while !pending.is_empty() {
        let count = pending.len();
        let mut next = vec![];
        for v in pending {
            if v["deleted"] == true
                || array(&v["parentVersionIds"])
                    .iter()
                    .all(|p| seen.contains(strv(&json!({"id":p}), "id")))
            {
                seen.insert(strv(&v, "versionId").to_owned());
                out.push(v);
            } else {
                next.push(v);
            }
        }
        if next.len() == count {
            return Err(invalid("Incomplete version ancestry."));
        }
        pending = next;
    }
    Ok(out)
}
