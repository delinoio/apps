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
    if info["formatVersion"] != 2 {
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
            "formatVersion":2,"epoch":epoch,"deviceId":s.device,"mutations":items}
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
            "formatVersion":2,"epoch":epoch,"cursor":cursor,"pageSize":100}
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
    attachments(auth, s, endpoint, owner, identity, epoch)?;
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
                "formatVersion":2,"epoch":epoch,"pageSize":1}
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
c.execute("UPDATE attachments SET synchronized=0", [])?;
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
    import_attachments(src, dst)?;
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
if let Some(old)=old {
let previous:Value=serde_json::from_str(&old)?;
if crate::protocol::fingerprint(&crate::protocol::message("SyncMutation",&previous)?) != crate::protocol::fingerprint(&crate::protocol::message("SyncMutation",&mutation)?) {
return Err(invalid("Outbox identity reused during import."));
}
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

fn import_attachments(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt =
        src.prepare("SELECT id,task_id,sha256,width,height,byte_size,png_base64 FROM attachments")?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, Option<String>>(6)?,
        ))
    })? {
        let (id, task, hash, width, height, size, png) = row?;
        let alive: bool = dst.query_row(
            "SELECT EXISTS(SELECT 1 FROM entities WHERE id=? AND kind=2 AND deleted=0)",
            [&task],
            |r| r.get(0),
        )?;
        if !alive {
            continue;
        }
        let old: Option<(String, String, i64, i64, i64)> = dst
            .query_row(
                "SELECT task_id,sha256,width,height,byte_size FROM attachments WHERE id=?",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        if old.is_some_and(|old| old != (task.clone(), hash.clone(), width, height, size)) {
            return Err(invalid("Immutable attachment changed during import."));
        }
        dst.execute("INSERT INTO attachments(id,task_id,sha256,width,height,byte_size,png_base64,synchronized) VALUES(?,?,?,?,?,?,?,0) ON CONFLICT(id) DO UPDATE SET png_base64=coalesce(attachments.png_base64,excluded.png_base64)", params![id,task,hash,width,height,size,png])?;
    }
    Ok(())
}

// Transfer URLs are transient capabilities. Use a separate client with no Lane
// bearer token, cookies, or redirects, and never store or log these URLs.
fn object_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|_| {
            Failure::new(
                "DEPENDENCY_UNAVAILABLE",
                "Attachment transport is unavailable.",
            )
        })
}
fn signed_url(value: &Value, field: &str) -> Result<url::Url> {
    let url = url::Url::parse(strv(value, field))
        .map_err(|_| invalid("Invalid signed attachment access."))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "Signed attachment access requires HTTPS without credentials.",
        ));
    }
    if !chrono::DateTime::parse_from_rfc3339(strv(value, "expiresAt"))
        .is_ok_and(|t| t > chrono::Utc::now())
    {
        return Err(Failure::new(
            "DEPENDENCY_UNAVAILABLE",
            "Attachment access expired; retry synchronization.",
        ));
    }
    Ok(url)
}
fn attachments(
    auth: &Auth,
    s: &Store,
    endpoint: &str,
    owner: &str,
    identity: &Identity,
    epoch: &str,
) -> Result<()> {
    let pending = s.with(false, |c| {
        let mut stmt = c.prepare("SELECT a.id,a.task_id,a.sha256,a.width,a.height,a.byte_size,a.png_base64 FROM attachments a JOIN entities e ON e.id=a.task_id WHERE e.deleted=0 AND (a.synchronized=0 OR a.png_base64 IS NULL) ORDER BY a.id")?;
        let items = stmt.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,Option<String>>(6)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(items)
    })?;
    let client = object_client()?;
    let mut failure = None;
    for (attachment, task, hash, width, height, size, png) in pending {
        lease(s, owner, Some(identity))?;
        let reference = json!({"attachmentId":attachment,"sha256":hash,"width":width,"height":height,"byteSize":size.to_string()});
        let result = transfer_attachment(
            auth,
            s,
            endpoint,
            &client,
            epoch,
            &task,
            &reference,
            png.as_deref(),
        );
        match result {
            Ok(bytes) => {
                use base64::Engine;
                lease(s, owner, Some(identity))?;
                s.with(true, |c| {
                    guard(s,c,owner)?;
                    if meta(c,"account_epoch")? != epoch || meta(c,"reconnect_required")? == "1" { return Err(auth::reconnect_error()); }
                    // Owner deletion or replacement during I/O must never recreate content.
                    c.execute("UPDATE attachments SET png_base64=?,synchronized=1 WHERE id=? AND task_id=? AND sha256=? AND width=? AND height=? AND byte_size=? AND EXISTS(SELECT 1 FROM entities WHERE id=? AND deleted=0)", params![base64::engine::general_purpose::STANDARD.encode(bytes),attachment,task,hash,width,height,size,task])?;
                    c.execute("INSERT INTO changes(entity_id,change_kind) VALUES(?,1)", [&task])?;
                    Ok(())
                })?;
            }
            Err(e) => {
                if matches!(
                    e.reason.as_str(),
                    "ERROR_REASON_FORBIDDEN"
                        | "ERROR_REASON_UNAUTHENTICATED"
                        | "ERROR_REASON_STALE_EPOCH"
                        | "ERROR_REASON_UNSUPPORTED_FORMAT"
                ) {
                    return Err(e);
                }
                tracing::warn!(reason=%e.reason, "Lane attachment synchronization failed");
                failure = Some(e);
            }
        }
    }
    failure.map_or(Ok(()), Err)
}
#[expect(
    clippy::too_many_arguments,
    reason = "The transfer keeps account, epoch, object transport and immutable owner explicit."
)]
fn transfer_attachment(
    auth: &Auth,
    s: &Store,
    endpoint: &str,
    client: &reqwest::Client,
    epoch: &str,
    task: &str,
    reference: &Value,
    png: Option<&str>,
) -> Result<Vec<u8>> {
    use crate::network::SendRequest;
    use base64::{Engine, engine::general_purpose::STANDARD};
    let request = json!({"taskId":task,"attachmentId":reference["attachmentId"],"epoch":epoch});
    let unavailable = || {
        Failure::new(
            "DEPENDENCY_UNAVAILABLE",
            "Attachment is waiting for synchronization; retry.",
        )
    };
    let bytes = if let Some(png) = png {
        let bytes = STANDARD
            .decode(png)
            .map_err(|_| invalid("Invalid stored attachment encoding."))?;
        crate::capture::validate_png(reference, &bytes)?;
        let begin = remote::call(auth, s, endpoint, "BeginAttachmentUpload", &request)?;
        if begin["state"] != "ATTACHMENT_STATE_AVAILABLE" {
            let url = signed_url(&begin, "uploadUrl")?;
            let mut put = client.put(url).body(bytes.clone());
            if let Some(headers) = begin["headers"].as_object() {
                for (name, value) in headers {
                    if name.eq_ignore_ascii_case("authorization")
                        || name.eq_ignore_ascii_case("cookie")
                        || name.eq_ignore_ascii_case("host")
                    {
                        return Err(invalid("Invalid attachment upload header."));
                    }
                    put = put.header(
                        name,
                        value
                            .as_str()
                            .ok_or_else(|| invalid("Invalid attachment upload header."))?,
                    );
                }
            }
            if !put.send_cancellable(auth)?.status().is_success() {
                return Err(unavailable());
            }
            let mut complete = request;
            complete["uploadId"] = begin["uploadId"].clone();
            if remote::call(auth, s, endpoint, "CompleteAttachmentUpload", &complete)?["state"]
                != "ATTACHMENT_STATE_AVAILABLE"
            {
                return Err(unavailable());
            }
        }
        bytes
    } else {
        let response = remote::call(
            auth,
            s,
            endpoint,
            "GetAttachment",
            &json!({"taskId":task,"attachmentId":reference["attachmentId"]}),
        )?;
        if response["state"] != "ATTACHMENT_STATE_AVAILABLE" {
            return Err(unavailable());
        }
        if strv(&response, "png").is_empty() {
            let size = strv(reference, "byteSize")
                .parse::<usize>()
                .map_err(|_| invalid("Invalid attachment byte count."))?;
            let response = client
                .get(signed_url(&response, "downloadUrl")?)
                .send_cancellable_limit(auth, size)?;
            if !response.status().is_success() {
                return Err(unavailable());
            }
            response.body
        } else {
            STANDARD
                .decode(strv(&response, "png"))
                .map_err(|_| invalid("Invalid attachment encoding."))?
        }
    };
    crate::capture::validate_png(reference, &bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    #[test]
    fn signed_access_rejects_credentials_insecure_transport_and_expiry() {
        let expires = (chrono::Utc::now() + chrono::Duration::minutes(1)).to_rfc3339();
        for url in [
            "http://example.com/image",
            "https://user:secret@example.com/image",
            "https://example.com/image#fragment",
            "file:///image",
        ] {
            assert!(
                signed_url(
                    &json!({"downloadUrl":url,"expiresAt":expires}),
                    "downloadUrl"
                )
                .is_err()
            );
        }
        assert!(signed_url(&json!({"downloadUrl":"https://example.com/image?signature=transient","expiresAt":expires}),"downloadUrl").is_ok());
        assert!(signed_url(&json!({"downloadUrl":"https://example.com/image","expiresAt":"2000-01-01T00:00:00Z"}),"downloadUrl").is_err());
    }
}
