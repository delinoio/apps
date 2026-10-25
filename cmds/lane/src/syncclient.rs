// SPDX-License-Identifier: Apache-2.0
use crate::{
    auth::{self, Auth},
    error::{Failure, Result, invalid},
    protocol::{id, strv, valid_id},
    remote,
    store::{Identity, Store, meta, put_meta, rows, sync},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
fn busy() -> Failure {
    Failure::new(
        "STORAGE_BUSY",
        "Another process owns synchronization. Retry after it exits.",
    )
}
#[must_use]
pub fn deleted() -> Failure {
    Failure::new("DELETED", "The server directed account-data deletion.")
}
fn guard(s: &Store, c: &Connection, owner: &str) -> Result<()> {
    if meta(c, "sync_owner")? != owner
        || meta(c, "sync_owner_expires_at_us")?
            .parse::<i64>()
            .unwrap_or(0)
            <= chrono::Utc::now().timestamp_micros()
    {
        return Err(busy());
    }
    if meta(c, "account_deleted")? == "1" {
        return Err(deleted());
    }
    if s.unavailable()? {
        return Err(deleted());
    }
    Ok(())
}
fn lease(s: &Store, owner: &str, identity: Option<&Identity>) -> Result<()> {
    if let Some(expected) = identity
        && (s.identity()? != *expected || !expected.enabled)
    {
        return Err(Failure::new(
            "FORBIDDEN",
            "Synchronization was disabled or the account changed.",
        ));
    }
    s.with(true, |c| {
        if meta(c, "account_deleted")? == "1" {
            return Err(deleted());
        }
        let old = meta(c, "sync_owner")?;
        if !old.is_empty()
            && old != owner
            && meta(c, "sync_owner_expires_at_us")?
                .parse::<i64>()
                .unwrap_or(0)
                > chrono::Utc::now().timestamp_micros()
        {
            return Err(busy());
        }
        put_meta(c, "sync_owner", owner)?;
        put_meta(
            c,
            "sync_owner_expires_at_us",
            &(chrono::Utc::now().timestamp_micros() + 60_000_000).to_string(),
        )
    })
}
fn release(s: &Store, owner: &str) -> Result<()> {
    s.with(true, |c| {
        if meta(c, "sync_owner")? == owner {
            put_meta(c, "sync_owner", "")?;
            put_meta(c, "sync_owner_expires_at_us", "0")?;
        }
        Ok(())
    })
}
pub fn failure(s: &Store, e: &Failure) -> Result<()> {
    s.with(true, |c| {
        if e.reason == "ERROR_REASON_STALE_EPOCH" {
            put_meta(c, "reconnect_required", "1")?;
        }
        put_meta(c, "sync_error_reason", &e.reason)?;
        c.execute("INSERT INTO changes(change_kind) VALUES(4)", [])?;
        Ok(())
    })
}
pub fn run(auth: &Auth, s: &Store, endpoint: &str, once: bool, retry_blocked: bool) -> Result<()> {
    let identity = s.identity()?;
    if !identity.enabled {
        return Err(Failure::new(
            "ONLINE_REQUIRED",
            "Run lane sync connect and approve the connection first.",
        ));
    }
    if retry_blocked {
        s.with(true, |c| {
            c.execute(
                "UPDATE outbox SET state=1,error_reason=NULL WHERE state=2",
                [],
            )?;
            Ok(())
        })?;
    }
    let owner = id();
    lease(s, &owner, Some(&identity))?;
    let result = (|| {
        let mut delay = 1;
        let mut watch: Option<crate::watch::Watch> = None;
        loop {
            if auth.cancelled.load(Ordering::Relaxed) {
                return Ok(());
            }
            let result = cycle(auth, s, endpoint, &owner, &identity);
            if let Err(e) = &result
                && e.reason != "ERROR_REASON_DELETED"
            {
                failure(s, e)?;
            }
            if once {
                return result;
            }
            if let Err(e) = result {
                if matches!(
                    e.reason.as_str(),
                    "ERROR_REASON_STALE_EPOCH"
                        | "ERROR_REASON_DELETED"
                        | "ERROR_REASON_FORBIDDEN"
                        | "ERROR_REASON_UNAUTHENTICATED"
                        | "ERROR_REASON_UNSUPPORTED_FORMAT"
                        | "ERROR_REASON_STORAGE_BUSY"
                ) {
                    return Err(e);
                }
                tracing::warn!(reason=%e.reason,"Lane sync attempt failed");
                delay = (delay * 2).min(30);
            } else {
                delay = 1;
                let credential = auth.token(s)?;
                let (epoch, cursor) = s.with(false, |c| {
                    Ok((meta(c, "account_epoch")?, meta(c, "pull_cursor")?))
                })?;
                if let Some(watch) = &watch {
                    watch.update(credential, epoch, cursor);
                } else {
                    watch = Some(crate::watch::Watch::start(auth, credential, epoch, cursor));
                }
            }
            let until = Instant::now() + Duration::from_secs(delay);
            while Instant::now() < until && !auth.cancelled.load(Ordering::Relaxed) {
                if watch.as_ref().is_some_and(crate::watch::Watch::notified) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    })();
    let released = release(s, &owner);
    result.and(released)
}
fn info(auth: &Auth, s: &Store, endpoint: &str) -> Result<Value> {
    let info = remote::call(auth, s, endpoint, "GetSyncInfo", &json!({}))?;
    let identity = s.identity()?;
    if info["formatVersion"] != 1 {
        return Err(Failure::new(
            "UNSUPPORTED_FORMAT",
            "Update Lane before synchronizing this format.",
        ));
    }
    if info["accountId"] != identity.account
        || info["tenantId"] != identity.tenant
        || !valid_id(strv(&info, "epoch"))
    {
        return Err(Failure::new(
            "FORBIDDEN",
            "Synchronization identity mismatch.",
        ));
    }
    Ok(info)
}
fn cycle(auth: &Auth, s: &Store, endpoint: &str, owner: &str, identity: &Identity) -> Result<()> {
    if s.with(false, |c| Ok(meta(c, "reconnect_required")? == "1"))? {
        return Err(auth::reconnect_error());
    }
    lease(s, owner, Some(identity))?;
    let info = info(auth, s, endpoint)?;
    let epoch = strv(&info, "epoch");
    let stale = s.with(true, |c| {
        guard(s, c, owner)?;
        let old = meta(c, "account_epoch")?;
        if !old.is_empty() && old != epoch {
            put_meta(c, "reconnect_required", "1")?;
            return Ok(true);
        }
        put_meta(c, "account_epoch", epoch)?;
        Ok(false)
    })?;
    if stale {
        return Err(auth::reconnect_error());
    }
    let items = s.with(false, |c| {
        rows(
            c,
            "SELECT request_json FROM outbox WHERE state=1 ORDER BY created_at_us,rowid LIMIT 100",
            [],
        )?
        .into_iter()
        .map(|raw| Ok(serde_json::from_str::<Value>(&raw)?))
        .collect::<Result<Vec<_>>>()
    })?;
    if !items.is_empty() {
        lease(s, owner, Some(identity))?;
        let response = remote::call(
            auth,
            s,
            endpoint,
            "PushChanges",
            &json!({
            "formatVersion":1,"epoch":epoch,"deviceId":s.device,"mutations":items}
            ),
        )?;
        let results = response["results"]
            .as_array()
            .ok_or_else(|| invalid("Invalid push response."))?;
        if results.len() != items.len() {
            return Err(invalid("Invalid push response count."));
        }
        for (r, m) in results.iter().zip(&items) {
            if r["requestId"] != m["meta"]["requestId"]
                || r.get("result").is_some() == r.get("error").is_some()
            {
                return Err(invalid("Invalid push response identity."));
            }
        }
        s.with(true, |c| {
            guard(s, c, owner)?;
            if s.identity()? != *identity {
                return Err(Failure::new("FORBIDDEN", "Account identity changed."));
            }
            sync::mappings(c, &response["repositoryMappings"])?;
            for (r, m) in results.iter().zip(&items) {
                if let Some(e) = r.get("error") {
                    let reason = strv(e, "reason");
                    if matches!(
                        reason,
                        "ERROR_REASON_STALE_EPOCH" | "ERROR_REASON_UNSUPPORTED_FORMAT"
                    ) {
                        return Err(Failure::new(
                            reason.trim_start_matches("ERROR_REASON_"),
                            "Synchronization rejected.",
                        ));
                    }
                    if reason == "ERROR_REASON_STALE_RESOLUTION" {
                        c.execute(
                            "DELETE FROM heads WHERE entity_id=? AND version_id=?",
                            params![
                                strv(&m["version"], "entityId"),
                                strv(&m["version"], "versionId")
                            ],
                        )?;
                    }
                    c.execute(
                        "UPDATE outbox SET state=?,error_reason=? WHERE request_id=?",
                        params![
                            if e["retryable"] == true { 1 } else { 2 },
                            reason,
                            strv(r, "requestId")
                        ],
                    )?;
                } else {
                    sync::apply(c, &r["result"])?;
                    c.execute(
                        "DELETE FROM outbox WHERE request_id=?",
                        [strv(r, "requestId")],
                    )?;
                }
            }
            c.execute("INSERT INTO changes(change_kind) VALUES(4)", [])?;
            Ok(())
        })?;
    }
    loop {
        if auth.cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        lease(s, owner, Some(identity))?;
        let cursor = s.with(false, |c| meta(c, "pull_cursor"))?;
        let response = remote::call(
            auth,
            s,
            endpoint,
            "PullChanges",
            &json!({
            "formatVersion":1,"epoch":epoch,"cursor":cursor,"pageSize":100}
            ),
        )?;
        if response["hasMore"] == true && response["nextCursor"] == cursor {
            return Err(invalid("Pull cursor did not advance."));
        }
        let account_deleted = s.with(true, |c| {
            guard(s, c, owner)?;
            if s.identity()? != *identity {
                return Err(Failure::new("FORBIDDEN", "Account identity changed."));
            }
            if let Some(changes) = response["changes"].as_array() {
                for change in changes {
                    sync::change(c, change, &identity.account)?;
                }
            }
            put_meta(c, "pull_cursor", strv(&response, "nextCursor"))?;
            Ok(meta(c, "account_deleted")? == "1")
        })?;
        if account_deleted {
            return Err(deleted());
        }
        if response["hasMore"] != true {
            break;
        }
    }
    s.with(true, |c| {
        guard(s, c, owner)?;
        put_meta(c, "sync_error_reason", "")?;
        put_meta(
            c,
            "last_sync_at_us",
            &chrono::Utc::now().timestamp_micros().to_string(),
        )?;
        c.execute("INSERT INTO changes(change_kind) VALUES(4)", [])?;
        Ok(())
    })
}
pub fn reconnect(auth: &Auth, s: &Store, consent: &Value) -> Result<()> {
    let owner = id();
    lease(s, &owner, None)?;
    let result = (|| {
        let info = info(auth, s, "")?;
        let (epoch, sequence, _) = auth::approval_snapshot(s)?;
        if consent["approval_space_id"] == s.space
            && (consent["approval_epoch"] != epoch || consent["approval_sequence"] != sequence)
        {
            return Err(auth::reconnect_error());
        }
        let next = strv(&info, "epoch");
        if epoch == next {
            return Ok(());
        }
        if !epoch.is_empty() {
            let probe = remote::call(
                auth,
                s,
                "",
                "PullChanges",
                &json!({
                "formatVersion":1,"epoch":epoch,"pageSize":1}
                ),
            );
            match probe {
                Ok(response) => {
                    let changes = response["changes"]
                        .as_array()
                        .ok_or_else(|| invalid("Invalid deletion directive."))?;
                    if changes.len() != 1
                        || response["hasMore"] == true
                        || !strv(&response, "nextCursor").is_empty()
                        || changes[0]["deletion"]["kind"] != "ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT"
                        || changes[0]["deletion"]["id"] != info["accountId"]
                        || changes[0]["deletion"]["deletionVersionId"] != info["epoch"]
                    {
                        return Err(invalid("Invalid deletion directive."));
                    }
                    s.with(true, |c| {
                        guard(s, c, &owner)?;
                        sync::change(c, &changes[0], strv(&info, "accountId"))
                    })?;
                    return Err(deleted());
                }
                Err(e) if e.reason == "ERROR_REASON_STALE_EPOCH" => {}
                Err(e) => return Err(e),
            }
            let confirmed = consent["reconnect_confirmed"] == true
                && consent["approval_space_id"] == s.space
                && consent["approval_epoch"] == epoch
                && consent["account_id"] == info["accountId"]
                && consent["tenant_id"] == info["tenantId"]
                && chrono::DateTime::parse_from_rfc3339(strv(consent, "approval_expires_at"))
                    .is_ok_and(|t| t > chrono::Utc::now());
            if !confirmed {
                failure(s, &auth::reconnect_error())?;
                return Err(auth::reconnect_error());
            }
        }
        s.with(true,|c|{
guard(s,c,&owner)?;
if meta(c,"account_epoch")?!=epoch{
return Err(auth::reconnect_error());
}
if !epoch.is_empty(){
let current:String=c.query_row("SELECT CAST(coalesce(max(sequence),0) AS TEXT) FROM changes WHERE change_kind IN(1,2)",[],|r|r.get(0))?;
if current!=sequence||meta(c,"reconnect_required")?!="1"{
return Err(auth::reconnect_error());
}
let versions=sync::ordered(c)?;
c.execute("DELETE FROM outbox",[])?;
for(i,v)in versions.into_iter().enumerate(){
let rid=id();
let count=v["parentVersionIds"].as_array().map_or(0,Vec::len);
let kind=if v["deleted"]==true{
"MUTATION_KIND_DELETE"}
else if count==0{
"MUTATION_KIND_CREATE"}
else if count==1{
"MUTATION_KIND_SAVE"}
else{
"MUTATION_KIND_RESOLVE"}
;
let m=json!({
"meta":{
"requestId":rid,"origin":v["origin"]}
,"version":v,"expectedHeadIds":v["parentVersionIds"],"kind":kind}
);
c.execute("INSERT INTO outbox(request_id,entity_id,request_json,state,created_at_us) VALUES(?,?,?,1,?)",params![rid,strv(&v,"entityId"),m.to_string(),i64::try_from(i).map_err(|_| invalid("Too many retained versions."))?])?;
}
}
            put_meta(c,"account_epoch",next)?;
put_meta(c,"reconnect_required","0")?;
put_meta(c,"sync_error_reason","")?;
put_meta(c,"pull_cursor","")?;
c.execute("INSERT INTO changes(change_kind) VALUES(4)",[])?;
Ok(())}
)
    })();
    let released = release(s, &owner);
    result.and(released)
}
pub fn import(source: &Store, dest: &Store) -> Result<()> {
    if source.space == dest.space
        || source.root != dest.root
        || source.device != dest.device
        || !source.identity()?.account.is_empty()
        || dest.identity()?.account.is_empty()
    {
        return Err(Failure::new(
            "FORBIDDEN",
            "Standalone import identity mismatch.",
        ));
    }
    let key: String = dest.registry.query_row(
        "SELECT account_key FROM spaces WHERE id=?",
        [&dest.space],
        |r| r.get(0),
    )?;
    let marker = format!("standalone_import_account_{key}");
    let skip = source.with(true, |c| {
        let previous = meta(c, &marker)?;
        if !previous.is_empty() && previous != dest.space {
            return Ok(true);
        }
        put_meta(c, &marker, &dest.space)?;
        Ok(false)
    })?;
    if skip {
        return Ok(());
    }
    source.with(true,|src|dest.with(true,|dst|{
let completed=format!("standalone_import_source_{}",source.space);
if meta(dst,&completed)?=="1"{
return Ok(());
}
    let mut stmt=src.prepare("SELECT id,normalized_url FROM entities WHERE kind=1 AND deleted=0 AND normalized_url IS NOT NULL")?;
for row in stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?{
let(local,url)=row?;
let canonical:Option<String>=dst.query_row("SELECT id FROM entities WHERE kind=1 AND deleted=0 AND normalized_url=?",[url],|r|r.get(0)).optional()?;
if let Some(canonical)=canonical&& canonical!=local{
sync::mappings(dst,&json!([{
"localId":local,"canonicalId":canonical}
]))?;
}
}
    import_versions(src, dst)?;
    let mut stmt=src.prepare("SELECT method,request_id,fingerprint,entity_id,result_json FROM receipts")?;
for row in stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,String>(4)?)))?{
let(method,id,fp,entity,mut result)=row?;
let old:Option<String>=dst.query_row("SELECT fingerprint FROM receipts WHERE method=? AND request_id=?",params![method,id],|r|r.get(0)).optional()?;
if old.is_some_and(|old|old!=fp){
return Err(invalid("Request ID reused during import."));
}
let deleted:Option<bool>=dst.query_row("SELECT deleted FROM entities WHERE id=?",[&entity],|r|r.get(0)).optional()?;
if deleted==Some(true){
result=json!({
"result":"RESULT_KIND_DELETED"}
).to_string();
}
dst.execute("INSERT OR IGNORE INTO receipts VALUES(?,?,?,?,?)",params![method,id,fp,entity,result])?;
}
    let mut stmt=src.prepare("SELECT request_id,entity_id,request_json,state,error_reason,created_at_us FROM outbox ORDER BY created_at_us,rowid")?;
for row in stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i32>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,i64>(5)?)))?{
let(id,entity,raw,state,reason,created)=row?;
let mutation:Value=serde_json::from_str(&raw)?;
let deleted:bool=dst.query_row("SELECT deleted FROM entities WHERE id=?",[&entity],|r|r.get(0))?;
if deleted&&mutation["version"]["deleted"]!=true{
continue;
}
let old:Option<String>=dst.query_row("SELECT request_json FROM outbox WHERE request_id=?",[&id],|r|r.get(0)).optional()?;
if old.is_some_and(|old|old!=raw){
return Err(invalid("Outbox identity reused during import."));
}
dst.execute("INSERT OR IGNORE INTO outbox VALUES(?,?,?,?,?,?)",params![id,entity,raw,state,reason,created])?;
}
put_meta(dst,&completed,"1")
    }
))
}

// History must not become a live projection during import: a repository may
// have released an old URL that another account repository now owns.
fn import_versions(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt =
        src.prepare("SELECT id,kind,repository_id,normalized_url,deleted FROM entities")?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i32>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, bool>(4)?,
        ))
    })? {
        let (entity, kind, repository, mut url, deleted) = row?;
        let mapped: bool = dst.query_row(
            "SELECT EXISTS(SELECT 1 FROM repository_mappings WHERE local_id=?)",
            [&entity],
            |r| r.get(0),
        )?;
        if mapped {
            url = None;
        }
        dst.execute("INSERT OR IGNORE INTO entities(id,kind,repository_id,normalized_url,deleted) VALUES(?,?,?,?,?)", params![entity,kind,repository,url,deleted])?;
    }
    for v in sync::ordered(src)? {
        let deleted: bool = dst.query_row(
            "SELECT deleted FROM entities WHERE id=?",
            [strv(&v, "entityId")],
            |r| r.get(0),
        )?;
        if deleted && v["deleted"] != true {
            continue;
        }
        sync::validate(&v)?;
        sync::insert_version(dst, &v)?;
        let prompt: bool = src.query_row(
            "SELECT prompt_version FROM versions WHERE version_id=?",
            [strv(&v, "versionId")],
            |r| r.get(0),
        )?;
        if prompt {
            dst.execute(
                "UPDATE versions SET prompt_version=1 WHERE version_id=?",
                [strv(&v, "versionId")],
            )?;
        }
    }
    for raw in rows(
        src,
        "SELECT v.envelope_json FROM heads h JOIN versions v ON v.version_id=h.version_id ORDER BY v.entity_id,v.version_id",
        [],
    )? {
        let version: Value = serde_json::from_str(&raw)?;
        sync::apply(dst, &json!({"version":version}))?;
    }
    Ok(())
}
