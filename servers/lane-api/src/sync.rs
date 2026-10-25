// SPDX-License-Identifier: FSL-1.1-ALv2
//! Sync calls never commit. The transport opens an authorized account-locked transaction per item.
use crate::entities::{self as e, REPOSITORY, TASK, fault};
use crate::{Principal, Result, string};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};

async fn epoch(db: &mut PgConnection, p: &Principal) -> Result<String> {
    sqlx::query_scalar("SELECT epoch FROM lane_accounts WHERE account_id=$1 AND tenant_id=$2")
        .bind(&p.account_id)
        .bind(&p.tenant_id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| {
            fault(
                17,
                "account generation is unavailable; reconnect explicitly",
            )
        })
}
async fn check_epoch(db: &mut PgConnection, p: &Principal, supplied: &str) -> Result<String> {
    let current = epoch(db, p).await?;
    if current != supplied {
        return Err(fault(
            17,
            "account generation changed; reconnect explicitly",
        ));
    }
    Ok(current)
}
fn scope(epoch: &str) -> String {
    format!("PullChanges:{epoch}")
}
/// Validate a subscription against the account's current generation and durable pull position.
/// The subscription must separately reauthorize and recheck session revocation before publishing.
pub async fn validate_watch(
    db: &mut PgConnection,
    p: &Principal,
    supplied_epoch: &str,
    cursor: &str,
) -> Result<i64> {
    let epoch = check_epoch(db, p, supplied_epoch).await?;
    if cursor.is_empty() {
        return Ok(0);
    }
    let pos = e::cursor(db, &p.account_id, &scope(&epoch), cursor).await?;
    pos["after"]
        .as_i64()
        .ok_or_else(|| fault(1, "invalid pull cursor"))
}

fn normalize_version(v: &Value) -> Result<Value> {
    if v["formatVersion"].as_u64() != Some(1) {
        return Err(fault(8, "unsupported synchronization format; update Lane"));
    }
    e::uuid(string(v, "entityId"))?;
    e::uuid(string(v, "versionId"))?;
    let kind = match v["kind"].as_str() {
        Some("ENTITY_KIND_REPOSITORY") => REPOSITORY,
        Some("ENTITY_KIND_TASK") => TASK,
        _ => match v["kind"].as_i64() {
            Some(1) => REPOSITORY,
            Some(2) => TASK,
            _ => return Err(fault(1, "invalid entity kind")),
        },
    };
    let timestamp = chrono::DateTime::parse_from_rfc3339(string(v, "createdAt"))
        .map_err(|_| fault(1, "invalid version timestamp"))?;
    let timestamp = chrono::DateTime::from_timestamp_micros(timestamp.timestamp_micros())
        .ok_or_else(|| fault(1, "timestamp out of range"))?;
    let parents = e::set_ids(v.get("parentVersionIds").unwrap_or(&json!([])))?;
    if parents.iter().any(|s| s == string(v, "versionId")) {
        return Err(fault(1, "a version cannot parent itself"));
    }
    let deleted = v["deleted"].as_bool().unwrap_or(false);
    let mut origin = v["origin"].clone();
    let origin_kind = match origin["kind"].as_str() {
        Some("ORIGIN_KIND_APP") => "ORIGIN_KIND_APP",
        Some("ORIGIN_KIND_CLI") => "ORIGIN_KIND_CLI",
        Some("ORIGIN_KIND_MCP") => "ORIGIN_KIND_MCP",
        _ => match origin["kind"].as_u64() {
            Some(1) => "ORIGIN_KIND_APP",
            Some(2) => "ORIGIN_KIND_CLI",
            Some(3) => "ORIGIN_KIND_MCP",
            _ => return Err(fault(1, "invalid origin kind")),
        },
    };
    origin = json!({"kind":origin_kind,"deviceId":string(&origin,"deviceId"),"deviceName":string(&origin,"deviceName"),"accountId":string(&origin,"accountId")});
    if !string(&origin, "deviceId").is_empty() {
        e::uuid(string(&origin, "deviceId"))?;
    }
    let mut out = json!({"formatVersion":1,"entityId":v["entityId"],"kind":if kind==REPOSITORY{"ENTITY_KIND_REPOSITORY"}else{"ENTITY_KIND_TASK"},"versionId":v["versionId"],"parentVersionIds":parents,"createdAt":timestamp.to_rfc3339_opts(chrono::SecondsFormat::Micros,true),"origin":origin,"deleted":deleted,"promptRestoration":v["promptRestoration"].as_bool().unwrap_or(false)});
    if out["promptRestoration"] == true && (deleted || kind != TASK || parents.len() != 1) {
        return Err(fault(
            1,
            "prompt restoration requires a live task with one parent",
        ));
    }
    if deleted {
        if v.get("repository").is_some() || v.get("task").is_some() {
            return Err(fault(1, "deleted versions cannot contain a body"));
        }
    } else if kind == REPOSITORY {
        out["repository"] = e::repository_body(&v["repository"])?;
    } else {
        out["task"] = e::task_body(&v["task"], false)?;
    }
    Ok(out)
}
fn mutation_kind(v: &Value) -> Result<&'static str> {
    match v.as_str() {
        Some("MUTATION_KIND_CREATE") => Ok("CREATE"),
        Some("MUTATION_KIND_SAVE") => Ok("SAVE"),
        Some("MUTATION_KIND_RESOLVE") => Ok("RESOLVE"),
        Some("MUTATION_KIND_DELETE") => Ok("DELETE"),
        _ => match v.as_u64() {
            Some(1) => Ok("CREATE"),
            Some(2) => Ok("SAVE"),
            Some(3) => Ok("RESOLVE"),
            Some(4) => Ok("DELETE"),
            _ => Err(fault(1, "invalid mutation kind")),
        },
    }
}
/// Apply one immutable mutation in the caller's transaction. The returned object has the
/// PushChanges response layout so adapters can concatenate results and mappings in input order.
pub async fn push_item(
    db: &mut PgConnection,
    p: &Principal,
    q: &Value,
    max_item_bytes: usize,
) -> Result<Value> {
    if q["formatVersion"].as_u64() != Some(1) {
        return Err(fault(8, "unsupported synchronization format; update Lane"));
    }
    check_epoch(db, p, string(q, "epoch")).await?;
    if string(q, "deviceId") != p.device_id || p.device_id.is_empty() {
        return Err(fault(
            13,
            "sync device does not match authenticated session",
        ));
    }
    let items = q["mutations"]
        .as_array()
        .ok_or_else(|| fault(1, "mutations are required"))?;
    if items.len() != 1 {
        return Err(fault(1, "push_item requires exactly one mutation"));
    }
    let m = &items[0];
    let request = string(&m["meta"], "requestId");
    e::uuid(request)?;
    let v = normalize_version(&m["version"])?;
    if !string(&v["origin"], "accountId").is_empty()
        && string(&v["origin"], "accountId") != p.account_id
    {
        return Err(fault(13, "version belongs to another account"));
    }
    let origin_device = string(&v["origin"], "deviceId");
    if !origin_device.is_empty() && origin_device != p.device_id {
        let associated: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM lane_devices WHERE account_id=$1 AND id=$2)",
        )
        .bind(&p.account_id)
        .bind(origin_device)
        .fetch_one(&mut *db)
        .await?;
        if !associated {
            return Err(fault(
                13,
                "version origin device does not belong to this account",
            ));
        }
    }
    let kind = if v["kind"] == "ENTITY_KIND_REPOSITORY" {
        REPOSITORY
    } else {
        TASK
    };
    let action = mutation_kind(&m["kind"])?;
    if v["promptRestoration"] == true && action != "SAVE" {
        return Err(fault(1, "prompt restoration must be a SAVE mutation"));
    }
    let parents = e::set_ids(&v["parentVersionIds"])?;
    let expected = e::set_ids(m.get("expectedHeadIds").unwrap_or(&json!([])))?;
    let normalized = json!({"meta":{"requestId":request,"origin":v["origin"]},"version":v,"expectedHeadIds":expected,"kind":format!("MUTATION_KIND_{action}")});
    let digest = e::fingerprint(&normalized)?;
    if let Some(r) = e::receipt(db, &p.account_id, "PushChanges", request, &digest).await? {
        return Ok(
            json!({"results":[{"requestId":request,"result":r["result"]}],"repositoryMappings":mappings(db,&p.account_id).await?}),
        );
    }
    let original = string(&v, "entityId");
    let mut entity_id = e::canonical_repository(db, &p.account_id, original).await?;
    let mut existing = e::entity(db, &p.account_id, &entity_id).await?;
    if existing.as_ref().is_some_and(|x| x.kind != kind) {
        return Err(fault(1, "entity kind mismatch"));
    }
    let response;
    if existing.as_ref().is_some_and(|x| x.deleted) {
        response = e::result(db, &p.account_id, &entity_id).await?;
    } else if action == "DELETE" {
        if v["deleted"] != true {
            return Err(fault(1, "delete mutation requires a deleted version"));
        }
        response = e::delete_entity(db, p, &entity_id, kind, Some(v.clone())).await?;
    } else {
        if v["deleted"] == true {
            return Err(fault(1, "deleted version requires delete mutation"));
        }
        // Parent repository deletion dominates even a delayed first upload of a task.
        if kind == TASK {
            let repo =
                e::canonical_repository(db, &p.account_id, string(&v["task"], "repositoryId"))
                    .await?;
            if e::entity(db, &p.account_id, &repo)
                .await?
                .is_some_and(|r| r.kind == REPOSITORY && r.deleted)
            {
                response = e::delete_entity(db, p, &entity_id, TASK, None).await?;
                return finish(db, p, request, &digest, &entity_id, &response).await;
            }
        }
        if let Some(row) = sqlx::query(
            "SELECT entity_id,envelope FROM lane_versions WHERE account_id=$1 AND version_id=$2",
        )
        .bind(&p.account_id)
        .bind(string(&v, "versionId"))
        .fetch_optional(&mut *db)
        .await?
        {
            let stored: Value = row.get("envelope");
            if row.get::<String, _>("entity_id") != entity_id || stored != v {
                return Err(fault(
                    6,
                    "version ID identifies different immutable content",
                ));
            }
            response = json!({"result":"RESULT_KIND_REPLAYED","version":v,"alternatives":[],"correlationId":""});
            return finish(db, p, request, &digest, &entity_id, &response).await;
        }
        if e::canonical(&normalized)?.len() > max_item_bytes {
            return Err(fault(15, "item exceeds configured byte limit"));
        }
        if action == "CREATE" && !parents.is_empty() {
            return Err(fault(1, "creation cannot have parents"));
        }
        if action == "SAVE" && parents.len() != 1 {
            return Err(fault(1, "save requires exactly one parent"));
        }
        if action == "RESOLVE" && (parents.len() < 2 || parents != expected) {
            return Err(fault(
                1,
                "resolution requires all expected heads as parents",
            ));
        }
        if kind == REPOSITORY
            && action == "CREATE"
            && existing.is_none()
            && let Some(canonical) = e::url_owner(
                db,
                &p.account_id,
                string(&v["repository"], "normalizedUrl"),
                &entity_id,
            )
            .await?
        {
            sqlx::query("INSERT INTO lane_repository_mappings(account_id,local_id,canonical_id) VALUES($1,$2,$3)").bind(&p.account_id).bind(original).bind(&canonical).execute(&mut *db).await?;
            entity_id = canonical;
            existing = e::entity(db, &p.account_id, &entity_id).await?;
        }
        match existing {
            None => {
                if action != "CREATE" {
                    return Err(fault(
                        9,
                        "parent entity is missing; retry after uploading parents",
                    ));
                }
                if kind == TASK {
                    e::task_body(&v["task"], true)?;
                }
                e::new_entity(db, p, &entity_id, kind, &v).await?;
                response = e::emit(db, &p.account_id, &entity_id).await?;
            }
            Some(current) => {
                let hs = e::heads(db, &p.account_id, &entity_id).await?;
                let accepted =
                    e::stored_version(db, &p.account_id, &entity_id, &current.accepted).await?;
                if action == "RESOLVE" && expected != hs {
                    return Err(fault(5, "resolution is stale; read current heads"));
                }
                if action != "RESOLVE" && hs.len() > 1 {
                    return Err(fault(4, "conflicted entity is read-only"));
                }
                let mut parent_versions = Vec::new();
                for parent in &parents {
                    match e::stored_version(db, &p.account_id, &entity_id, parent).await {
                        Ok(v) => parent_versions.push(v),
                        Err(f) if f.reason == 2 => {
                            return Err(fault(
                                9,
                                "parent version is missing; upload parents first",
                            ));
                        }
                        Err(f) => return Err(f),
                    }
                }
                if kind == TASK && action == "SAVE" {
                    let base = &parent_versions[0]["task"];
                    if base["proposal"] != v["task"]["proposal"]
                        && (base["proposal"] == "PROPOSAL_REGULAR"
                            || !matches!(
                                string(&v["task"], "proposal"),
                                "PROPOSAL_ACCEPTED" | "PROPOSAL_REJECTED"
                            ))
                    {
                        return Err(fault(1, "invalid proposal transition"));
                    }
                }
                e::validate_body(db, &p.account_id, &entity_id, kind, &v).await?;
                let prompt = kind == TASK
                    && (v["promptRestoration"] == true
                        || parent_versions.is_empty() && v["task"].get("prompt").is_some()
                        || parent_versions
                            .iter()
                            .any(|old| old["task"].get("prompt") != v["task"].get("prompt")));
                e::insert_version(db, &p.account_id, &entity_id, &v, prompt).await?;
                let same_repository = kind == REPOSITORY
                    && action == "CREATE"
                    && v["repository"] == accepted["repository"];
                if !same_repository {
                    if action == "RESOLVE" || parents == hs {
                        sqlx::query("DELETE FROM lane_heads WHERE account_id=$1 AND entity_id=$2")
                            .bind(&p.account_id)
                            .bind(&entity_id)
                            .execute(&mut *db)
                            .await?;
                        e::project(db, &p.account_id, &entity_id, kind, &v).await?;
                        if kind == TASK && v["task"]["proposal"] != "PROPOSAL_PENDING" {
                            crate::push::cancel(db, &p.account_id, &entity_id).await?;
                        }
                    }
                    e::add_head(db, &p.account_id, &entity_id, string(&v, "versionId")).await?;
                }
                response = e::emit(db, &p.account_id, &entity_id).await?;
            }
        }
    }
    finish(db, p, request, &digest, &entity_id, &response).await
}
async fn mappings(db: &mut PgConnection, account: &str) -> Result<Vec<Value>> {
    let rows=sqlx::query("SELECT local_id,canonical_id FROM lane_repository_mappings WHERE account_id=$1 ORDER BY local_id").bind(account).fetch_all(db).await?;
    Ok(rows.iter().map(|r|json!({"localId":r.get::<String,_>("local_id"),"canonicalId":r.get::<String,_>("canonical_id")})).collect())
}
async fn finish(
    db: &mut PgConnection,
    p: &Principal,
    request: &str,
    digest: &str,
    entity_id: &str,
    result: &Value,
) -> Result<Value> {
    e::save_receipt(
        db,
        &p.account_id,
        "PushChanges",
        request,
        digest,
        entity_id,
        &json!({"result":result}),
    )
    .await?;
    Ok(
        json!({"results":[{"requestId":request,"result":result}],"repositoryMappings":mappings(db,&p.account_id).await?}),
    )
}
async fn pull(db: &mut PgConnection, p: &Principal, q: &Value) -> Result<Value> {
    let epoch = check_epoch(db, p, string(q, "epoch")).await?;
    let size = e::page_size(&q["pageSize"])?;
    let token = string(q, "cursor");
    let (after, high) = if token.is_empty() {
        (0, None)
    } else {
        let pos = e::cursor(db, &p.account_id, &scope(&epoch), token).await?;
        (
            pos["after"]
                .as_i64()
                .ok_or_else(|| fault(1, "invalid cursor"))?,
            pos["high"].as_i64(),
        )
    };
    let high = match high {
        Some(h) => h,
        None => sqlx::query_scalar::<_, Option<i64>>(
            "SELECT max(sequence) FROM lane_changes WHERE account_id=$1",
        )
        .bind(&p.account_id)
        .fetch_one(&mut *db)
        .await?
        .unwrap_or(0),
    };
    let rows=sqlx::query("SELECT sequence,data FROM lane_changes WHERE account_id=$1 AND sequence>$2 AND sequence<=$3 ORDER BY sequence LIMIT $4").bind(&p.account_id).bind(after).bind(high).bind((size+1) as i64).fetch_all(&mut *db).await?;
    let more = rows.len() > size;
    let mut changes = Vec::new();
    let mut last = after;
    for row in rows.iter().take(size) {
        last = row.get("sequence");
        let mut data: Value = row.get("data");
        data["sequence"] = json!(last.to_string());
        changes.push(data);
    }
    let next = e::new_cursor(
        db,
        &p.account_id,
        &scope(&epoch),
        &json!({"after":last,"high":if more{Some(high)}else{None}}),
    )
    .await?;
    Ok(json!({"changes":changes,"nextCursor":next,"hasMore":more}))
}
pub async fn dispatch(
    db: &mut PgConnection,
    p: &Principal,
    method: &str,
    q: &Value,
    max_item_bytes: usize,
) -> Result<Value> {
    match method {
        "GetSyncInfo" => Ok(
            json!({"formatVersion":1,"accountId":p.account_id,"tenantId":p.tenant_id,"epoch":epoch(db,p).await?,"maxItemBytes":max_item_bytes.to_string()}),
        ),
        "PushChanges" => push_item(db, p, q, max_item_bytes).await,
        "PullChanges" => {
            if q["formatVersion"].as_u64() != Some(1) {
                return Err(fault(8, "unsupported synchronization format; update Lane"));
            }
            pull(db, p, q).await
        }
        "WatchChanges" => {
            let after = validate_watch(db, p, string(q, "epoch"), string(q, "cursor")).await?;
            let latest: i64 = sqlx::query_scalar::<_, Option<i64>>(
                "SELECT max(sequence) FROM lane_changes WHERE account_id=$1",
            )
            .bind(&p.account_id)
            .fetch_one(&mut *db)
            .await?
            .unwrap_or(0);
            let token = if latest > after {
                e::new_cursor(
                    db,
                    &p.account_id,
                    &scope(string(q, "epoch")),
                    &json!({"after":latest,"high":null}),
                )
                .await?
            } else {
                string(q, "cursor").to_owned()
            };
            Ok(json!({"cursor":token}))
        }
        _ => Err(fault(1, "unknown Sync method")),
    }
}
