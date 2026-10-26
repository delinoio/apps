// SPDX-License-Identifier: FSL-1.1-ALv2
//! Account-scoped immutable entities. Callers own the transaction, account lock and authorization.
use crate::{Fault, Principal, Result, canonical_id, hash, id, now, string};
use connectrpc::ErrorCode;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};

pub(crate) const REPOSITORY: i16 = 1;
pub(crate) const TASK: i16 = 2;
pub(crate) const URL_CONNECTION: i16 = 3;
pub(crate) const LABEL: i16 = 4;
pub(crate) const MILESTONE: i16 = 5;
pub(crate) const CYCLE: i16 = 6;
/// Envelope format written by this server.
pub(crate) const FORMAT_VERSION: u64 = 3;
pub(crate) fn body_key(kind: i16) -> &'static str {
    match kind {
        REPOSITORY => "repository",
        URL_CONNECTION => "urlConnection",
        LABEL => "label",
        MILESTONE => "milestone",
        CYCLE => "cycle",
        _ => "task",
    }
}
pub(crate) fn kind_name(kind: i16) -> &'static str {
    match kind {
        REPOSITORY => "ENTITY_KIND_REPOSITORY",
        URL_CONNECTION => "ENTITY_KIND_URL_CONNECTION",
        LABEL => "ENTITY_KIND_LABEL",
        MILESTONE => "ENTITY_KIND_MILESTONE",
        CYCLE => "ENTITY_KIND_CYCLE",
        _ => "ENTITY_KIND_TASK",
    }
}
pub(crate) fn fault(reason: i32, message: &str) -> Fault {
    let code = match reason {
        2 => ErrorCode::NotFound,
        3 | 5 => ErrorCode::Aborted,
        4 | 7 | 8 | 17 => ErrorCode::FailedPrecondition,
        6 => ErrorCode::AlreadyExists,
        9 | 14 => ErrorCode::Unavailable,
        15 => ErrorCode::ResourceExhausted,
        13 => ErrorCode::PermissionDenied,
        _ => ErrorCode::InvalidArgument,
    };
    Fault::new(code, reason, message)
}
pub(crate) fn uuid(value: &str) -> Result<()> {
    if !canonical_id(value) {
        return Err(fault(1, "expected a lowercase UUID v7"));
    }
    Ok(())
}

/// Normalize the organizational URL without fetching the repository.
pub fn normalize_url(input: &str) -> Result<String> {
    if input.contains(['\\', '?', '#']) || input.trim() != input {
        return Err(fault(1, "invalid repository URL"));
    }
    let expanded;
    let raw = if input.starts_with("https://") || input.starts_with("ssh://") {
        input
    } else if let Some((host, path)) = input.split_once(':') {
        if !host.contains('@') || host.contains('/') {
            return Err(fault(1, "invalid repository URL"));
        }
        expanded = format!("ssh://{host}/{path}");
        &expanded
    } else {
        return Err(fault(1, "expected HTTPS, SSH or SCP repository URL"));
    };
    // Inspect the raw path before URL parsing, which otherwise removes dot segments.
    let authority_end = raw.find("://").unwrap() + 3;
    let path_start = raw[authority_end..]
        .find('/')
        .map(|n| authority_end + n)
        .ok_or_else(|| fault(1, "repository path is required"))?;
    if path_start == authority_end {
        return Err(fault(1, "repository host is required"));
    }
    let raw_path = raw[path_start..].trim_matches('/');
    let mut path = String::new();
    let bytes = raw_path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(fault(1, "invalid percent escape"));
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .map_err(|_| fault(1, "invalid percent escape"))?;
            let b = u8::from_str_radix(hex, 16).map_err(|_| fault(1, "invalid percent escape"))?;
            if b == b'/' || b == b'\\' {
                return Err(fault(1, "encoded separator is forbidden"));
            }
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                path.push(b as char);
            } else {
                path.push_str(&format!("%{b:02X}"));
            }
            i += 3;
        } else {
            let c = raw_path[i..].chars().next().unwrap();
            if c.is_control() || c.is_whitespace() {
                return Err(fault(1, "invalid repository path"));
            }
            path.push(c);
            i += c.len_utf8();
        }
    }
    if path.split('/').any(|s| s == "." || s == "..") {
        return Err(fault(1, "dot path segment is forbidden"));
    }
    if let Some(p) = path.strip_suffix(".git") {
        path = p.to_owned();
    }
    if path.is_empty() {
        return Err(fault(1, "repository path is required"));
    }
    let parsed = url::Url::parse(raw).map_err(|_| fault(1, "invalid repository URL"))?;
    let host = parsed
        .host_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fault(1, "host is required"))?;
    // The HTTPS parser performs IDNA normalization; SSH's opaque host parser does not.
    let host_url =
        url::Url::parse(&format!("https://{host}/")).map_err(|_| fault(1, "invalid host"))?;
    let host = host_url.host_str().unwrap().to_lowercase();
    let port = parsed.port().filter(|n| {
        !((parsed.scheme() == "https" && *n == 443) || (parsed.scheme() == "ssh" && *n == 22))
    });
    Ok(format!(
        "{}{}",
        match port {
            Some(n) => format!("{host}:{n}/"),
            None => format!("{host}/"),
        },
        path
    ))
}

/// RFC 8785 canonical JSON for Lane entity requests (which contain no floating point fields).
/// Object keys sort by UTF-16 code units, including non-BMP keys in future typed fields.
pub(crate) fn canonical(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Object(m) => {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort_by_cached_key(|s| s.encode_utf16().collect::<Vec<_>>());
            let mut parts = Vec::new();
            for k in keys {
                // Protobuf emits newly added repeated fields as []; old receipts omitted them.
                // Keep historical fingerprints stable without rewriting immutable envelopes.
                if k == "captures" && m[k].as_array().is_some_and(Vec::is_empty) {
                    continue;
                }
                parts.push(format!(
                    "{}:{}",
                    serde_json::to_string(k)?,
                    canonical(&m[k])?
                ));
            }
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter()
                .map(canonical)
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        Value::Number(n) if n.is_f64() => {
            return Err(fault(
                1,
                "floating point fields are not supported in entity requests",
            ));
        }
        _ => serde_json::to_string(value)?,
    })
}
pub(crate) fn fingerprint(value: &Value) -> Result<String> {
    Ok(hash(canonical(value)?.as_bytes()))
}
pub(crate) fn set_ids(v: &Value) -> Result<Vec<String>> {
    let values = v
        .as_array()
        .ok_or_else(|| fault(1, "expected a version ID set"))?;
    let mut ids = Vec::new();
    for v in values {
        let s = v.as_str().ok_or_else(|| fault(1, "invalid version ID"))?;
        uuid(s)?;
        ids.push(s.to_owned());
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}
pub(crate) fn enum_value(value: &Value, names: &[&str], default: Option<&str>) -> Result<String> {
    let selected = if value.is_null()
        || value.as_u64() == Some(0)
        || value.as_str().is_some_and(|v| v.ends_with("_UNSPECIFIED"))
    {
        default.ok_or_else(|| fault(1, "enum value is required"))?
    } else if let Some(n) = value.as_u64() {
        names
            .get(n.saturating_sub(1) as usize)
            .copied()
            .ok_or_else(|| fault(1, "unknown enum value"))?
    } else {
        value
            .as_str()
            .filter(|s| names.contains(s))
            .ok_or_else(|| fault(1, "unknown enum value"))?
    };
    Ok(selected.to_owned())
}
pub(crate) const PROGRESS: &[&str] = &[
    "PROGRESS_TODO",
    "PROGRESS_IN_PROGRESS",
    "PROGRESS_BLOCKED",
    "PROGRESS_DONE",
    "PROGRESS_CANCELED",
];
pub(crate) const PROPOSAL: &[&str] = &[
    "PROPOSAL_REGULAR",
    "PROPOSAL_PENDING",
    "PROPOSAL_ACCEPTED",
    "PROPOSAL_REJECTED",
];
pub(crate) fn task_body(v: &Value, creation: bool) -> Result<Value> {
    let title = string(v, "title");
    if title.trim().is_empty() {
        return Err(fault(1, "title must contain non-whitespace text"));
    }
    let repository = string(v, "repositoryId");
    uuid(repository)?;
    let progress = enum_value(
        &v["progress"],
        PROGRESS,
        creation.then_some("PROGRESS_TODO"),
    )?;
    let proposal = enum_value(
        &v["proposal"],
        PROPOSAL,
        creation.then_some("PROPOSAL_REGULAR"),
    )?;
    if creation && proposal != "PROPOSAL_REGULAR" && proposal != "PROPOSAL_PENDING" {
        return Err(fault(1, "creation permits regular or pending tasks"));
    }
    let mut body = json!({"repositoryId":repository,"title":title,"description":string(v,"description"),"progress":progress,"proposal":proposal});
    crate::planning::task_planning_fields(v, body.as_object_mut().expect("task body"))?;
    if let Some(prompt) = v.get("prompt").filter(|v| !v.is_null()) {
        if !prompt.is_string() {
            return Err(fault(1, "prompt must be Markdown text"));
        }
        body["prompt"] = prompt.clone();
    }
    if let Some(captures) = v.get("captures").filter(|v| !v.is_null()) {
        let captures = crate::capture::references(captures)?;
        if !captures.as_array().unwrap().is_empty() {
            body["captures"] = captures;
        }
    }
    Ok(body)
}
/// Planning fields of a task body, in request field form.
const PLANNING_KEYS: [&str; 12] = [
    "priority",
    "type",
    "labelIds",
    "startDate",
    "dueDate",
    "storyPoints",
    "milestoneId",
    "cycleId",
    "parentTaskId",
    "dependsOnTaskIds",
    "relatedTaskIds",
    "duplicateOfTaskId",
];
/// Validate an already normalized `host[:port]/path` repository identifier.
///
/// The identifier carries no transport, so no transport default may be stripped from it.
/// `normalize_url` removes port 443 for HTTPS but keeps it for SSH, and keeps port 22 for HTTPS.
/// Validate an explicit port 443 as SSH and every other identifier as HTTPS so that no explicit
/// port is ever removed. A synthetic terminal `.git` is appended because normalization removes
/// one, and a normalized path may itself end in `.git`.
fn normalized_repository_identifier(identifier: &str) -> Result<String> {
    let authority = identifier.split('/').next().unwrap_or_default();
    let explicit_443 = authority
        .rsplit_once(':')
        .is_some_and(|(_, port)| port.parse::<u16>() == Ok(443));
    let scheme = if explicit_443 { "ssh" } else { "https" };
    normalize_url(&format!("{scheme}://{identifier}.git"))
}
pub(crate) fn repository_body(v: &Value) -> Result<Value> {
    let normalized = normalized_repository_identifier(string(v, "normalizedUrl"))?;
    let mut b = json!({"normalizedUrl":normalized});
    if let Some(name) = v.get("displayName").filter(|v| !v.is_null()) {
        if !name.is_string() {
            return Err(fault(1, "display name must be text"));
        }
        b["displayName"] = name.clone();
    }
    Ok(b)
}
pub(crate) fn version(
    entity: &str,
    kind: i16,
    parents: &[String],
    body: Option<Value>,
    p: &Principal,
) -> Value {
    let mut v = json!({"formatVersion":FORMAT_VERSION,"entityId":entity,"kind":kind_name(kind),"versionId":id(),"parentVersionIds":parents,"createdAt":now().to_rfc3339_opts(chrono::SecondsFormat::Micros,true),"origin":p.origin(),"deleted":body.is_none(),"promptRestoration":false});
    if let Some(body) = body {
        v[body_key(kind)] = body;
    }
    v
}
#[derive(Debug)]
pub(crate) struct Entity {
    pub kind: i16,
    pub deleted: bool,
    pub accepted: String,
    /// Canonical repository of a task, URL connection or planning item.
    pub repository: Option<String>,
}
pub(crate) async fn entity(
    db: &mut PgConnection,
    account: &str,
    entity: &str,
) -> Result<Option<Entity>> {
    let row = sqlx::query(
        "SELECT kind,deleted,accepted_version_id,repository_id FROM lane_entities WHERE account_id=$1 AND id=$2",
    )
    .bind(account)
    .bind(entity)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| Entity {
        kind: r.get("kind"),
        deleted: r.get("deleted"),
        accepted: r.get("accepted_version_id"),
        repository: r.get("repository_id"),
    }))
}
pub(crate) async fn canonical_repository(
    db: &mut PgConnection,
    account: &str,
    repository: &str,
) -> Result<String> {
    Ok(sqlx::query_scalar(
        "SELECT canonical_id FROM lane_repository_mappings WHERE account_id=$1 AND local_id=$2",
    )
    .bind(account)
    .bind(repository)
    .fetch_optional(db)
    .await?
    .unwrap_or_else(|| repository.to_owned()))
}
/// Every spelling of one repository inside an account: the canonical ID first, then each local
/// alias mapped to it. Alias-owned rows and canonical-owned rows are the same repository.
pub(crate) async fn repository_identities(
    db: &mut PgConnection,
    account: &str,
    canonical: &str,
) -> Result<Vec<String>> {
    let mut identities = vec![canonical.to_owned()];
    identities.extend(
        sqlx::query_scalar::<_, String>(
            "SELECT local_id FROM lane_repository_mappings WHERE account_id=$1 AND canonical_id=$2 ORDER BY local_id",
        )
        .bind(account)
        .bind(canonical)
        .fetch_all(db)
        .await?,
    );
    Ok(identities)
}
pub async fn require_repository(
    db: &mut PgConnection,
    account: &str,
    repository: &str,
) -> Result<String> {
    let canonical = canonical_repository(db, account, repository).await?;
    match entity(db, account, &canonical).await? {
        Some(e) if e.kind == REPOSITORY && !e.deleted => Ok(canonical),
        Some(e) if e.deleted => Err(fault(7, "repository was deleted")),
        _ => Err(fault(2, "repository not found")),
    }
}
pub(crate) async fn heads(
    db: &mut PgConnection,
    account: &str,
    entity: &str,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT version_id FROM lane_heads WHERE account_id=$1 AND entity_id=$2 ORDER BY version_id").bind(account).bind(entity).fetch_all(db).await?)
}
pub(crate) async fn stored_version(
    db: &mut PgConnection,
    account: &str,
    entity: &str,
    version: &str,
) -> Result<Value> {
    sqlx::query_scalar(
        "SELECT envelope FROM lane_versions WHERE account_id=$1 AND entity_id=$2 AND version_id=$3",
    )
    .bind(account)
    .bind(entity)
    .bind(version)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| fault(2, "version not found"))
}
pub(crate) async fn result(db: &mut PgConnection, account: &str, entity_id: &str) -> Result<Value> {
    let e = entity(db, account, entity_id)
        .await?
        .ok_or_else(|| fault(2, "entity not found"))?;
    let v = stored_version(db, account, entity_id, &e.accepted).await?;
    let mut alternatives = Vec::new();
    let current_heads = heads(db, account, entity_id).await?;
    if current_heads.len() > 1 {
        for h in current_heads {
            alternatives.push(stored_version(db, account, entity_id, &h).await?);
        }
    }
    Ok(
        json!({"result":if e.deleted {"RESULT_KIND_DELETED"}else if alternatives.is_empty(){"RESULT_KIND_APPLIED"}else{"RESULT_KIND_CONFLICT"},"version":v,"alternatives":alternatives,"correlationId":""}),
    )
}
/// Append a typed Change payload. The caller publishes invalidations only after commit.
pub async fn change(db: &mut PgConnection, account: &str, data: &Value) -> Result<()> {
    sqlx::query("INSERT INTO lane_changes(account_id,data) VALUES($1,$2)")
        .bind(account)
        .bind(data)
        .execute(db)
        .await?;
    Ok(())
}
pub(crate) async fn emit(db: &mut PgConnection, account: &str, entity_id: &str) -> Result<Value> {
    let r = result(db, account, entity_id).await?;
    change(db, account, &json!({"entity":r})).await?;
    Ok(r)
}
pub(crate) async fn url_owner(
    db: &mut PgConnection,
    account: &str,
    url: &str,
    except: &str,
) -> Result<Option<String>> {
    // Conflict candidate URLs are reserved until resolution, as well as the accepted projection.
    Ok(sqlx::query_scalar("SELECT DISTINCT e.id FROM lane_entities e JOIN lane_heads h ON h.account_id=e.account_id AND h.entity_id=e.id JOIN lane_versions v ON v.account_id=h.account_id AND v.version_id=h.version_id WHERE e.account_id=$1 AND e.kind=1 AND NOT e.deleted AND e.id<>$3 AND (e.normalized_url=$2 OR v.envelope->'repository'->>'normalizedUrl'=$2) ORDER BY e.id LIMIT 1").bind(account).bind(url).bind(except).fetch_optional(db).await?)
}
pub(crate) async fn validate_body(
    db: &mut PgConnection,
    account: &str,
    entity_id: &str,
    kind: i16,
    v: &Value,
) -> Result<()> {
    if kind == REPOSITORY {
        if url_owner(
            db,
            account,
            string(&v["repository"], "normalizedUrl"),
            entity_id,
        )
        .await?
        .is_some()
        {
            return Err(fault(1, "repository URL is already registered"));
        }
    } else if crate::planning::is_item(kind) {
        crate::planning::validate_item(db, account, entity_id, kind, &v[body_key(kind)]).await?;
    } else {
        require_repository(db, account, string(&v[body_key(kind)], "repositoryId")).await?;
        if kind == URL_CONNECTION {
            crate::capture::validate_rule(db, account, entity_id, &v["urlConnection"]).await?;
        }
        if kind == TASK {
            crate::capture::validate_ownership(db, account, entity_id, &v["task"]).await?;
        }
    }
    Ok(())
}
/// Prompt-history membership of a task version, shared by the interactive and sync write paths so
/// equivalent version graphs classify identically.
///
/// A version is a member when it is an explicit prompt restoration, or when its normalized prompt
/// or ordered captures differ from any of its parent snapshots. A version without parents is a
/// member when it carries a prompt. Prompt absence and the empty prompt are distinct; omitted,
/// null and empty capture arrays are equal.
pub(crate) fn prompt_version(restoration: bool, parents: &[Value], task: &Value) -> bool {
    fn prompt(task: &Value) -> Option<&Value> {
        task.get("prompt").filter(|v| !v.is_null())
    }
    fn captures(task: &Value) -> Option<&Value> {
        task.get("captures")
            .filter(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
    }
    restoration
        || if parents.is_empty() {
            prompt(task).is_some()
        } else {
            parents.iter().any(|old| {
                prompt(&old["task"]) != prompt(task) || captures(&old["task"]) != captures(task)
            })
        }
}
pub(crate) async fn insert_version(
    db: &mut PgConnection,
    account: &str,
    entity_id: &str,
    v: &Value,
    prompt: bool,
) -> Result<()> {
    let digest = fingerprint(v)?;
    if let Some(row) = sqlx::query("SELECT entity_id,fingerprint FROM lane_version_identities WHERE account_id=$1 AND version_id=$2")
        .bind(account).bind(string(v,"versionId")).fetch_optional(&mut *db).await? {
        if row.get::<String,_>("entity_id") != entity_id || row.get::<String,_>("fingerprint") != digest {
            return Err(fault(6,"version ID identifies different immutable content"));
        }
    } else {
        sqlx::query("INSERT INTO lane_version_identities(account_id,version_id,entity_id,fingerprint) VALUES($1,$2,$3,$4)")
            .bind(account).bind(string(v,"versionId")).bind(entity_id).bind(&digest).execute(&mut *db).await?;
    }
    if v.get("task").is_some() {
        crate::capture::register_manifests(db, account, entity_id, &v["task"]).await?;
    }
    sqlx::query("INSERT INTO lane_versions(account_id,version_id,entity_id,envelope,prompt_version) VALUES($1,$2,$3,$4,$5)").bind(account).bind(string(v,"versionId")).bind(entity_id).bind(v).bind(prompt).execute(db).await?;
    Ok(())
}
pub(crate) async fn add_head(
    db: &mut PgConnection,
    account: &str,
    entity_id: &str,
    version: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO lane_heads(account_id,entity_id,version_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(account).bind(entity_id).bind(version).execute(db).await?;
    Ok(())
}
pub(crate) async fn project(
    db: &mut PgConnection,
    account: &str,
    entity_id: &str,
    kind: i16,
    v: &Value,
) -> Result<()> {
    let repo = if kind != REPOSITORY {
        Some(canonical_repository(db, account, string(&v[body_key(kind)], "repositoryId")).await?)
    } else {
        None
    };
    let url = if kind == REPOSITORY {
        Some(string(&v["repository"], "normalizedUrl"))
    } else {
        None
    };
    sqlx::query("UPDATE lane_entities SET accepted_version_id=$3,repository_id=$4,normalized_url=$5 WHERE account_id=$1 AND id=$2").bind(account).bind(entity_id).bind(string(v,"versionId")).bind(repo).bind(url).execute(db).await?;
    Ok(())
}
pub(crate) async fn new_entity(
    db: &mut PgConnection,
    p: &Principal,
    entity_id: &str,
    kind: i16,
    v: &Value,
) -> Result<()> {
    validate_body(db, &p.account_id, entity_id, kind, v).await?;
    sqlx::query(
        "INSERT INTO lane_entities(account_id,id,kind,accepted_version_id) VALUES($1,$2,$3,$4)",
    )
    .bind(&p.account_id)
    .bind(entity_id)
    .bind(kind)
    .bind(string(v, "versionId"))
    .execute(&mut *db)
    .await?;
    insert_version(
        db,
        &p.account_id,
        entity_id,
        v,
        kind == TASK && (v["task"].get("prompt").is_some() || v["task"].get("captures").is_some()),
    )
    .await?;
    add_head(db, &p.account_id, entity_id, string(v, "versionId")).await?;
    project(db, &p.account_id, entity_id, kind, v).await?;
    notify_creation(db, p, entity_id, v).await
}
pub(crate) async fn notify_creation(
    db: &mut PgConnection,
    p: &Principal,
    entity_id: &str,
    v: &Value,
) -> Result<()> {
    if string(&v["task"], "proposal") == "PROPOSAL_PENDING"
        && string(&v["origin"], "kind") == "ORIGIN_KIND_MCP"
    {
        let repo =
            require_repository(db, &p.account_id, string(&v["task"], "repositoryId")).await?;
        let r = result(db, &p.account_id, &repo).await?;
        let body = &r["version"]["repository"];
        let name = body["displayName"]
            .as_str()
            .unwrap_or(string(body, "normalizedUrl"));
        let mut source = p.clone();
        source.origin_kind = string(&v["origin"], "kind").to_owned();
        source.device_id = string(&v["origin"], "deviceId").to_owned();
        crate::push::enqueue(db, &source, entity_id, name, string(&v["task"], "title")).await?;
    }
    Ok(())
}

/// Permanent deletion scrubs snapshots, receipts and prior change bodies in this transaction.
/// Synchronized deletions apply the client's own cleanup versions and never cascade.
pub(crate) async fn delete_entity(
    db: &mut PgConnection,
    p: &Principal,
    entity_id: &str,
    kind: i16,
    supplied: Option<Value>,
) -> Result<Value> {
    delete_entity_with(db, p, entity_id, kind, supplied, false).await
}
/// `cleanup` removes references to a deleted task or planning item from the other tasks of its
/// repository (new versions with the actor's origin) before the tombstone is written.
pub(crate) async fn delete_entity_with(
    db: &mut PgConnection,
    p: &Principal,
    entity_id: &str,
    kind: i16,
    supplied: Option<Value>,
    cleanup: bool,
) -> Result<Value> {
    if let Some(e) = entity(db, &p.account_id, entity_id).await? {
        if e.kind != kind {
            return Err(fault(1, "entity kind mismatch"));
        }
        if e.deleted {
            return result(db, &p.account_id, entity_id).await;
        }
        if cleanup
            && (kind == TASK || crate::planning::is_item(kind))
            && let Some(repository) = &e.repository
        {
            crate::planning::cleanup_before_delete(db, p, entity_id, kind, repository).await?;
        }
    }
    if kind == REPOSITORY {
        // The whole repository goes, so no per-entity cleanup is needed.
        let tasks:Vec<String>=sqlx::query_scalar("SELECT id FROM lane_entities WHERE account_id=$1 AND repository_id=$2 AND kind IN (2,3,4,5,6) AND NOT deleted").bind(&p.account_id).bind(entity_id).fetch_all(&mut *db).await?;
        for task in tasks {
            let child = entity(db, &p.account_id, &task).await?.unwrap();
            Box::pin(delete_entity(db, p, &task, child.kind, None)).await?;
        }
        crate::webhooks::delete_repository(db, &p.account_id, entity_id).await?;
    } else {
        crate::push::cancel(db, &p.account_id, entity_id).await?;
        crate::webhooks::redact_task(db, &p.account_id, entity_id).await?;
    }
    crate::capture::delete_task(db, &p.account_id, entity_id).await?;
    let mut tombstone = supplied.unwrap_or_else(|| version(entity_id, kind, &[], None, p));
    // Deletion keeps no old parent graph, device name or user content.
    for body in ["task", "repository", "label", "milestone", "cycle"] {
        tombstone.as_object_mut().unwrap().remove(body);
    }
    tombstone["parentVersionIds"] = json!([]);
    tombstone["origin"] = json!({});
    tombstone["deleted"] = json!(true);
    tombstone["promptRestoration"] = json!(false);
    sqlx::query("INSERT INTO lane_entities(account_id,id,kind,deleted,accepted_version_id) VALUES($1,$2,$3,true,$4) ON CONFLICT(account_id,id) DO UPDATE SET deleted=true,repository_id=NULL,normalized_url=NULL,accepted_version_id=$4").bind(&p.account_id).bind(entity_id).bind(kind).bind(string(&tombstone,"versionId")).execute(&mut *db).await?;
    sqlx::query("DELETE FROM lane_versions WHERE account_id=$1 AND entity_id=$2")
        .bind(&p.account_id)
        .bind(entity_id)
        .execute(&mut *db)
        .await?;
    insert_version(db, &p.account_id, entity_id, &tombstone, false).await?;
    add_head(
        db,
        &p.account_id,
        entity_id,
        string(&tombstone, "versionId"),
    )
    .await?;
    let deleted = result(db, &p.account_id, entity_id).await?;
    sqlx::query("UPDATE lane_entity_receipts SET response=jsonb_build_object('result',$3::jsonb) WHERE account_id=$1 AND entity_id=$2").bind(&p.account_id).bind(entity_id).bind(&deleted).execute(&mut *db).await?;
    // Task receipts can also embed other tasks they changed. Redact the deleted entity there.
    sqlx::query("UPDATE lane_entity_receipts SET response=jsonb_set(response,'{affected}',(SELECT coalesce(jsonb_agg(CASE WHEN item->'version'->>'entityId'=$2 THEN $3::jsonb ELSE item END),'[]'::jsonb) FROM jsonb_array_elements(response->'affected') item)) WHERE account_id=$1 AND jsonb_typeof(response->'affected')='array'").bind(&p.account_id).bind(entity_id).bind(&deleted).execute(&mut *db).await?;
    // A CreateTask receipt can embed a repository response. Redact that field too.
    sqlx::query("UPDATE lane_entity_receipts SET response=jsonb_set(response,'{repository}',$3) WHERE account_id=$1 AND response->'repository'->'version'->>'entityId'=$2").bind(&p.account_id).bind(entity_id).bind(&deleted).execute(&mut *db).await?;
    let aliases: Vec<String> = sqlx::query_scalar(
        "SELECT local_id FROM lane_repository_mappings WHERE account_id=$1 AND canonical_id=$2",
    )
    .bind(&p.account_id)
    .bind(entity_id)
    .fetch_all(&mut *db)
    .await?;
    let mut identities = aliases;
    identities.push(entity_id.to_owned());
    sqlx::query("UPDATE lane_changes SET data=jsonb_build_object('entity',$3::jsonb) WHERE account_id=$1 AND data->'entity'->'version'->>'entityId'=ANY($2)").bind(&p.account_id).bind(&identities).bind(&deleted).execute(&mut *db).await?;
    emit(db, &p.account_id, entity_id).await
}

pub(crate) async fn receipt(
    db: &mut PgConnection,
    account: &str,
    method: &str,
    request: &str,
    digest: &str,
) -> Result<Option<Value>> {
    if let Some(row)=sqlx::query("SELECT fingerprint,response FROM lane_entity_receipts WHERE account_id=$1 AND method=$2 AND request_id=$3").bind(account).bind(method).bind(request).fetch_optional(db).await? {
        if row.get::<String,_>("fingerprint")!=digest { return Err(fault(6,"request ID was used with different content")); }
        let mut response:Value=row.get("response");
        if response["result"]["result"]=="RESULT_KIND_APPLIED" {response["result"]["result"]=json!("RESULT_KIND_REPLAYED");}
        return Ok(Some(response));
    }
    Ok(None)
}
pub(crate) async fn save_receipt(
    db: &mut PgConnection,
    account: &str,
    method: &str,
    request: &str,
    digest: &str,
    entity_id: &str,
    response: &Value,
) -> Result<()> {
    sqlx::query("INSERT INTO lane_entity_receipts(account_id,method,request_id,fingerprint,entity_id,response) VALUES($1,$2,$3,$4,$5,$6)").bind(account).bind(method).bind(request).bind(digest).bind(entity_id).bind(response).execute(db).await?;
    Ok(())
}

fn normalize_request(p: &Principal, method: &str, q: &Value) -> Result<Value> {
    let mut q = q.clone();
    if !q.is_object() {
        return Err(fault(1, "request must be an object"));
    }
    uuid(string(&q, "id"))?;
    uuid(string(&q["meta"], "requestId"))?;
    q["meta"]["origin"] = p.origin();
    if method == "CreateRepository" || method == "SaveRepository" {
        q["url"] = json!(normalize_url(string(&q, "url"))?);
    }
    if matches!(method, "CreateTask" | "SaveTask") {
        // Planning edits are canonical sets: sorted, unique and omitted when empty.
        let edits = crate::planning::Edits::from_value(&q["relationshipEdits"])?;
        if edits.is_empty() {
            q.as_object_mut().unwrap().remove("relationshipEdits");
        } else {
            q["relationshipEdits"] = edits.to_value();
        }
    }
    if matches!(method, "CreatePlanningItem" | "SavePlanningItem") {
        let (kind, body) = crate::planning::planning_body(&q["item"])?;
        q["item"] = json!({ body_key(kind): body });
    }
    if method == "CreateTask" {
        // Planning fields normalize exactly like a stored task body so a retry with the same
        // content yields the same fingerprint however the transport spelled the sets.
        let mut normalized = serde_json::Map::new();
        crate::planning::task_planning_fields(&q, &mut normalized)?;
        let object = q.as_object_mut().unwrap();
        for key in PLANNING_KEYS {
            match normalized.remove(key) {
                Some(v) => object.insert(key.to_string(), v),
                None => object.remove(key),
            };
        }
        if q["repository"].get("url").is_some() {
            q["repository"]["url"] = json!(normalize_url(string(&q["repository"], "url"))?);
        }
        q["description"] = json!(string(&q, "description"));
        q["progress"] = json!(enum_value(&q["progress"], PROGRESS, Some("PROGRESS_TODO"))?);
        q["proposal"] = json!(enum_value(
            &q["proposal"],
            PROPOSAL,
            Some("PROPOSAL_REGULAR")
        )?);
    }
    if matches!(method, "CreateURLConnection" | "SaveURLConnection") {
        q["connection"] = crate::capture::rule_body(&q["connection"])?;
    }
    if method == "SaveTask" {
        q["task"] = task_body(&q["task"], false)?;
    }
    if method == "SetProgress" {
        q["progress"] = json!(enum_value(&q["progress"], PROGRESS, None)?);
    }
    if method == "SetProposal" {
        q["proposal"] = json!(enum_value(&q["proposal"], PROPOSAL, None)?);
    }
    if method == "ResolveConflict" {
        q["expectedHeadIds"] = json!(set_ids(&q["expectedHeadIds"])?);
    }
    Ok(q)
}

pub(crate) async fn cursor(
    db: &mut PgConnection,
    account: &str,
    scope: &str,
    token: &str,
) -> Result<Value> {
    sqlx::query_scalar(
        "SELECT position FROM lane_entity_cursors WHERE token=$1 AND account_id=$2 AND scope=$3",
    )
    .bind(token)
    .bind(account)
    .bind(scope)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| fault(1, "cursor does not belong to this account and query"))
}
pub(crate) async fn new_cursor(
    db: &mut PgConnection,
    account: &str,
    scope: &str,
    position: &Value,
) -> Result<String> {
    let token = id();
    sqlx::query(
        "INSERT INTO lane_entity_cursors(token,account_id,scope,position) VALUES($1,$2,$3,$4)",
    )
    .bind(&token)
    .bind(account)
    .bind(scope)
    .bind(position)
    .execute(db)
    .await?;
    Ok(token)
}
pub(crate) fn page_size(v: &Value) -> Result<usize> {
    let n = v.as_u64().unwrap_or(0);
    if n > 1000 {
        return Err(fault(1, "page size exceeds 1000"));
    }
    Ok(if n == 0 { 100 } else { n as usize })
}
async fn list(db: &mut PgConnection, p: &Principal, method: &str, q: &Value) -> Result<Value> {
    match method {
        "ListTasks" => return crate::planning::list_tasks(db, p, q).await,
        "ListPlanningItems" => return crate::planning::list_items(db, p, q).await,
        _ => {}
    }
    let size = page_size(&q["page"]["size"])?;
    let mut scope_q = q.clone();
    if let Some(m) = scope_q.as_object_mut() {
        m.remove("page");
    }
    let scope = format!("{method}:{}", fingerprint(&scope_q)?);
    let token = string(&q["page"], "cursor");
    let after = if token.is_empty() {
        String::new()
    } else {
        string(&cursor(db, &p.account_id, &scope, token).await?, "after").to_owned()
    };
    if method == "ListHistory" {
        let entity_id = string(q, "id");
        uuid(entity_id)?;
        let entity_id = canonical_repository(db, &p.account_id, entity_id).await?;
        let e = entity(db, &p.account_id, &entity_id)
            .await?
            .ok_or_else(|| fault(2, "entity not found"))?;
        if e.deleted {
            return Ok(json!({"versions":[],"nextCursor":""}));
        }
        let rows:Vec<Value>=sqlx::query_scalar("SELECT envelope FROM lane_versions WHERE account_id=$1 AND entity_id=$2 AND (NOT $3 OR prompt_version) ORDER BY envelope->>'createdAt',version_id").bind(&p.account_id).bind(&entity_id).bind(q["promptsOnly"].as_bool().unwrap_or(false)).fetch_all(&mut *db).await?;
        let key = |v: &Value| format!("{}|{}", string(v, "createdAt"), string(v, "versionId"));
        let filtered: Vec<_> = rows
            .into_iter()
            .filter(|v| key(v) > after)
            .take(size + 1)
            .collect();
        let more = filtered.len() > size;
        let rows: Vec<_> = filtered.into_iter().take(size).collect();
        let next = if more {
            new_cursor(
                db,
                &p.account_id,
                &scope,
                &json!({"after":key(rows.last().unwrap())}),
            )
            .await?
        } else {
            String::new()
        };
        return Ok(json!({"versions":rows,"nextCursor":next}));
    }
    let kind = match method {
        "ListRepositories" => REPOSITORY,
        _ => URL_CONNECTION,
    };
    let ids:Vec<String>=sqlx::query_scalar("SELECT id FROM lane_entities WHERE account_id=$1 AND kind=$2 AND NOT deleted AND id>$3 ORDER BY id").bind(&p.account_id).bind(kind).bind(&after).fetch_all(&mut *db).await?;
    let mut items = Vec::new();
    let mut last = String::new();
    for entity_id in ids {
        let r = result(db, &p.account_id, &entity_id).await?;
        if items.len() == size {
            let next = new_cursor(db, &p.account_id, &scope, &json!({"after":last})).await?;
            return Ok(json!({"items":items,"nextCursor":next}));
        }
        last = entity_id;
        items.push(r);
    }
    Ok(json!({"items":items,"nextCursor":""}))
}

/// Create a webhook-generated task using exactly the interactive creation transaction rules.
pub async fn create_task(
    db: &mut PgConnection,
    p: &Principal,
    q: &Value,
    max_item_bytes: usize,
) -> Result<Value> {
    dispatch(db, p, "CreateTask", q, max_item_bytes).await
}

/// A preserved stale save returns Ok with RESULT_KIND_CONFLICT. Commit it before the adapter
/// sends STALE_VERSION; turning that result into an error inside the transaction loses history.
pub async fn dispatch(
    db: &mut PgConnection,
    p: &Principal,
    method: &str,
    q: &Value,
    max_item_bytes: usize,
) -> Result<Value> {
    if matches!(
        method,
        "ListRepositories"
            | "ListTasks"
            | "ListHistory"
            | "ListURLConnections"
            | "ListPlanningItems"
    ) {
        return list(db, p, method, q).await;
    }
    if matches!(method, "GetRepository" | "GetTask" | "GetPlanningItem") {
        let entity_id = canonical_repository(db, &p.account_id, string(q, "id")).await?;
        let e = entity(db, &p.account_id, &entity_id)
            .await?
            .ok_or_else(|| fault(2, "entity not found"))?;
        let wanted = match method {
            "GetRepository" => e.kind == REPOSITORY,
            "GetTask" => e.kind == TASK,
            _ => crate::planning::is_item(e.kind),
        };
        if !wanted {
            return Err(fault(2, "entity not found"));
        }
        return Ok(json!({"result":result(db,&p.account_id,&entity_id).await?}));
    }
    if !matches!(
        method,
        "CreateRepository"
            | "SaveRepository"
            | "DeleteRepository"
            | "CreateTask"
            | "SaveTask"
            | "SetProgress"
            | "SetProposal"
            | "RestorePrompt"
            | "DeleteTask"
            | "ResolveConflict"
            | "CreateURLConnection"
            | "SaveURLConnection"
            | "DeleteURLConnection"
            | "CreatePlanningItem"
            | "SavePlanningItem"
            | "DeletePlanningItem"
    ) {
        return Err(fault(1, "unknown Lane method"));
    }
    let q = normalize_request(p, method, q)?;
    let request = string(&q["meta"], "requestId");
    let digest = fingerprint(&q)?;
    if let Some(response) = receipt(db, &p.account_id, method, request, &digest).await? {
        return Ok(response);
    }
    let entity_id = canonical_repository(db, &p.account_id, string(&q, "id")).await?;
    let existing = entity(db, &p.account_id, &entity_id).await?;
    if existing.as_ref().is_some_and(|e| e.deleted) {
        let response = json!({"result":result(db,&p.account_id,&entity_id).await?});
        save_receipt(
            db,
            &p.account_id,
            method,
            request,
            &digest,
            &entity_id,
            &response,
        )
        .await?;
        return Ok(response);
    }
    let mut response;
    // Other tasks the write changes (relationship edits, related links, moves).
    let mut cascade = std::collections::BTreeMap::new();
    if matches!(
        method,
        "DeleteRepository" | "DeleteTask" | "DeleteURLConnection" | "DeletePlanningItem"
    ) {
        let kind = match method {
            "DeleteRepository" => REPOSITORY,
            "DeleteURLConnection" => URL_CONNECTION,
            "DeletePlanningItem" => match &existing {
                Some(e) if crate::planning::is_item(e.kind) => e.kind,
                _ => return Err(fault(2, "planning item not found")),
            },
            _ => TASK,
        };
        response = json!({"result":delete_entity_with(db,p,&entity_id,kind,None,true).await?});
    } else {
        if canonical(&q)?.len() > max_item_bytes {
            return Err(fault(15, "item exceeds configured byte limit"));
        }
        if matches!(
            method,
            "CreateRepository" | "CreateTask" | "CreateURLConnection" | "CreatePlanningItem"
        ) {
            if existing.is_some() {
                return Err(fault(6, "entity ID is already in use"));
            }
            let item_kind = crate::planning::planning_body(&q["item"]).map(|(k, _)| k);
            let kind = match method {
                "CreateRepository" => REPOSITORY,
                "CreateURLConnection" => URL_CONNECTION,
                "CreatePlanningItem" => item_kind?,
                _ => TASK,
            };
            let mut repo_result = None;
            let body = if crate::planning::is_item(kind) {
                q["item"][body_key(kind)].clone()
            } else if kind == URL_CONNECTION {
                q["connection"].clone()
            } else if kind == REPOSITORY {
                let mut b = json!({"normalizedUrl":q["url"]});
                if let Some(n) = q.get("displayName") {
                    b["displayName"] = n.clone();
                }
                b
            } else {
                let target = &q["repository"];
                if target.get("id").is_some() == target.get("url").is_some() {
                    return Err(fault(1, "choose exactly one repository target"));
                }
                let repository = if let Some(r) = target["id"].as_str() {
                    require_repository(db, &p.account_id, r).await?
                } else {
                    let url = string(target, "url");
                    if let Some(r) = url_owner(db, &p.account_id, url, "").await? {
                        r
                    } else {
                        let r = id();
                        let v = version(&r, REPOSITORY, &[], Some(json!({"normalizedUrl":url})), p);
                        new_entity(db, p, &r, REPOSITORY, &v).await?;
                        emit(db, &p.account_id, &r).await?;
                        r
                    }
                };
                repo_result = Some(result(db, &p.account_id, &repository).await?);
                let mut task = q.clone();
                task["repositoryId"] = json!(repository);
                let body = task_body(&task, true)?;
                let edits = crate::planning::Edits::from_value(&q["relationshipEdits"])?;
                let plan = crate::planning::plan_task(
                    db,
                    &p.account_id,
                    crate::planning::TaskPlan {
                        id: &entity_id,
                        current_repo: None,
                        body,
                        edits: &edits,
                        mode: crate::planning::Mode::Create,
                        supersede: true,
                    },
                )
                .await?;
                cascade = plan.others;
                plan.body
            };
            let v = version(&entity_id, kind, &[], Some(body), p);
            new_entity(db, p, &entity_id, kind, &v).await?;
            let mut r = json!({"result":emit(db,&p.account_id,&entity_id).await?});
            if let Some(repo) = repo_result {
                r["repository"] = repo;
            }
            response = r;
        } else {
            let e = existing.ok_or_else(|| fault(2, "entity not found"))?;
            let hs = heads(db, &p.account_id, &entity_id).await?;
            let resolving = method == "ResolveConflict";
            if !resolving && hs.len() != 1 {
                return Err(fault(4, "conflicted entity is read-only"));
            }
            let parents = if resolving {
                let expected = set_ids(&q["expectedHeadIds"])?;
                if hs.len() < 2 || expected != hs {
                    return Err(fault(5, "conflict heads changed; read current state"));
                }
                expected
            } else {
                let expected = string(&q, "expectedVersionId");
                uuid(expected)?;
                stored_version(db, &p.account_id, &entity_id, expected).await?;
                vec![expected.to_owned()]
            };
            if (method == "SaveRepository" && e.kind != REPOSITORY)
                || (method == "SaveURLConnection" && e.kind != URL_CONNECTION)
                || (method == "SavePlanningItem" && !crate::planning::is_item(e.kind))
                || (!resolving
                    && !matches!(
                        method,
                        "SaveRepository" | "SaveURLConnection" | "SavePlanningItem"
                    )
                    && e.kind != TASK)
            {
                return Err(fault(1, "entity kind mismatch"));
            }
            let body = if resolving {
                if q["resolved"]["deleted"].as_bool() == Some(true) {
                    return Err(fault(1, "use deletion to delete an entity"));
                }
                if crate::planning::is_item(e.kind) {
                    crate::planning::item_body(e.kind, &q["resolved"][body_key(e.kind)])?
                } else if e.kind == URL_CONNECTION {
                    crate::capture::rule_body(&q["resolved"]["urlConnection"])?
                } else if e.kind == TASK {
                    task_body(&q["resolved"]["task"], false)?
                } else {
                    repository_body(&q["resolved"]["repository"])?
                }
            } else if crate::planning::is_item(e.kind) {
                // Kind and repository are immutable; validate_body checks the repository.
                let (kind, body) = crate::planning::planning_body(&q["item"])?;
                if kind != e.kind {
                    return Err(fault(1, "a planning item's kind is immutable"));
                }
                body
            } else if e.kind == URL_CONNECTION {
                q["connection"].clone()
            } else if e.kind == REPOSITORY {
                let mut b = json!({"normalizedUrl":q["url"]});
                if let Some(n) = q.get("displayName") {
                    b["displayName"] = n.clone();
                }
                b
            } else {
                // Specialized edits use the submitted base snapshot, not a newer accepted head.
                let base = stored_version(db, &p.account_id, &entity_id, &parents[0]).await?;
                let mut b = base["task"].clone();
                match method {
                    "SaveTask" => {
                        b = q["task"].clone();
                        if b["proposal"] != base["task"]["proposal"] {
                            return Err(fault(1, "use SetProposal to change proposal disposition"));
                        }
                    }
                    "SetProgress" => b["progress"] = q["progress"].clone(),
                    "SetProposal" => {
                        if base["task"]["proposal"] == "PROPOSAL_REGULAR"
                            || !matches!(
                                string(&q, "proposal"),
                                "PROPOSAL_ACCEPTED" | "PROPOSAL_REJECTED"
                            )
                        {
                            return Err(fault(1, "invalid proposal transition"));
                        }
                        b["proposal"] = q["proposal"].clone();
                    }
                    "RestorePrompt" => {
                        let source = stored_version(
                            db,
                            &p.account_id,
                            &entity_id,
                            string(&q, "sourceVersionId"),
                        )
                        .await?;
                        b.as_object_mut().unwrap().remove("captures");
                        if let Some(captures) = source["task"].get("captures") {
                            b["captures"] = captures.clone();
                        }
                        b.as_object_mut().unwrap().remove("prompt");
                        if let Some(prompt) = source["task"].get("prompt") {
                            b["prompt"] = prompt.clone();
                        }
                    }
                    _ => return Err(fault(1, "invalid task operation")),
                }
                b
            };
            let mut body = body;
            if e.kind == TASK && matches!(method, "SaveTask" | "ResolveConflict") {
                // A save that is not based on the only head becomes a conflict alternative: it
                // is validated and preserved, but it changes no other task.
                let supersede = resolving || parents == hs;
                let edits = crate::planning::Edits::from_value(&q["relationshipEdits"])?;
                let plan = crate::planning::plan_task(
                    db,
                    &p.account_id,
                    crate::planning::TaskPlan {
                        id: &entity_id,
                        current_repo: e.repository.as_deref(),
                        body,
                        edits: &edits,
                        mode: crate::planning::Mode::Save,
                        supersede,
                    },
                )
                .await?;
                body = plan.body;
                if supersede {
                    cascade = plan.others;
                }
            } else if e.kind == TASK {
                // SetProgress, SetProposal and RestorePrompt copy the planning fields of the
                // submitted base snapshot. When that base is stale, the copy becomes a conflict
                // alternative whose relationships may no longer be valid against the current
                // heads (for example a dependency that would now close a cycle), so validate it
                // like a synchronized version: no reinterpretation, no cascade, and the union of
                // every head's references for the cycle checks.
                crate::planning::plan_task(
                    db,
                    &p.account_id,
                    crate::planning::TaskPlan {
                        id: &entity_id,
                        current_repo: e.repository.as_deref(),
                        body: body.clone(),
                        edits: &crate::planning::Edits::default(),
                        mode: crate::planning::Mode::Push,
                        supersede: parents == hs,
                    },
                )
                .await?;
            }
            let mut v = version(&entity_id, e.kind, &parents, Some(body), p);
            v["promptRestoration"] = json!(method == "RestorePrompt");
            validate_body(db, &p.account_id, &entity_id, e.kind, &v).await?;
            // Membership is a property of the version graph: compare with the submitted parent
            // snapshots, never with the accepted head, which a stale save or a resolution need
            // not have as its parent.
            let prompt = if e.kind == TASK {
                let mut snapshots = Vec::with_capacity(parents.len());
                for parent in &parents {
                    snapshots.push(stored_version(db, &p.account_id, &entity_id, parent).await?);
                }
                prompt_version(method == "RestorePrompt", &snapshots, &v["task"])
            } else {
                false
            };
            insert_version(db, &p.account_id, &entity_id, &v, prompt).await?;
            if resolving || parents == hs {
                sqlx::query("DELETE FROM lane_heads WHERE account_id=$1 AND entity_id=$2")
                    .bind(&p.account_id)
                    .bind(&entity_id)
                    .execute(&mut *db)
                    .await?;
                project(db, &p.account_id, &entity_id, e.kind, &v).await?;
                if e.kind == TASK && v["task"]["proposal"] != "PROPOSAL_PENDING" {
                    crate::push::cancel(db, &p.account_id, &entity_id).await?;
                }
            }
            add_head(db, &p.account_id, &entity_id, string(&v, "versionId")).await?;
            response = json!({"result":emit(db,&p.account_id,&entity_id).await?});
        }
    }
    if !cascade.is_empty() {
        let affected = crate::planning::write_others(db, p, cascade).await?;
        // The cascade can change the receipt of this very request, so write it afterwards.
        response["affected"] = json!(affected);
    }
    save_receipt(
        db,
        &p.account_id,
        method,
        request,
        &digest,
        &entity_id,
        &response,
    )
    .await?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixtures(name: &str) -> Value {
        let root = std::env::var("LANE_CONFORMANCE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../protos/lane/v1/fixtures")
            });
        serde_json::from_str(&std::fs::read_to_string(root.join(format!("{name}.json"))).unwrap())
            .unwrap()
    }
    #[test]
    fn shared_normalization_vectors() {
        let fixtures = fixtures("normalization");
        assert_eq!(fixtures["formatVersion"], 1);
        for case in fixtures["cases"].as_array().unwrap() {
            let got = normalize_url(string(&case["input"], "url"));
            if case["expected"].get("reason").is_some() {
                assert_eq!(got.unwrap_err().reason, 1, "{}", case["id"]);
            } else {
                assert_eq!(
                    got.unwrap(),
                    case["expected"]["normalizedUrl"],
                    "{}",
                    case["id"]
                );
            }
        }
    }
    #[test]
    fn shared_fingerprint_vectors() {
        let fixtures = fixtures("fingerprints");
        assert_eq!(fixtures["formatVersion"], 1);
        for case in fixtures["cases"].as_array().unwrap() {
            let q = &case["input"]["normalizedRequest"];
            assert_eq!(canonical(q).unwrap(), case["expected"]["canonicalJson"]);
            assert_eq!(fingerprint(q).unwrap(), case["expected"]["sha256"]);
        }
    }
    #[test]
    fn normalized_repository_body_does_not_strip_another_suffix() {
        let v = json!({"normalizedUrl":"example.com/a.git"});
        assert_eq!(repository_body(&v).unwrap(), v);
    }
    #[test]
    fn normalized_repository_body_preserves_explicit_ports() {
        for url in [
            "example.com:443/a/b",
            "example.com:22/a/b",
            "example.com:2222/a/b",
            "example.com:8443/a/b",
            "example.com/a/b",
            "example.com:443/a/b.git",
            "example.com/Mixed/Case",
            "[::1]:443/a/b",
        ] {
            let v = json!({"normalizedUrl":url});
            assert_eq!(repository_body(&v).unwrap(), v, "{url}");
            let named = json!({"normalizedUrl":url,"displayName":"Name"});
            assert_eq!(repository_body(&named).unwrap(), named, "{url}");
        }
    }
    #[test]
    fn raw_input_normalization_removes_only_transport_defaults() {
        for (raw, expected) in [
            ("ssh://git@example.com:443/a/b.git", "example.com:443/a/b"),
            ("git@example.com:a/b.git", "example.com/a/b"),
            ("ssh://git@example.com:22/a/b.git", "example.com/a/b"),
            ("https://example.com:443/a/b.git", "example.com/a/b"),
            ("ssh://git@example.com:2222/a/b", "example.com:2222/a/b"),
            ("https://example.com:22/a/b", "example.com:22/a/b"),
            ("https://example.com:8443/a/b", "example.com:8443/a/b"),
        ] {
            assert_eq!(normalize_url(raw).unwrap(), expected, "{raw}");
            let v = json!({"normalizedUrl":expected});
            assert_eq!(repository_body(&v).unwrap(), v, "{raw}");
        }
    }
    #[test]
    fn normalized_repository_body_rejects_malformed_identifiers() {
        for url in [
            "",
            "example.com",
            "example.com/",
            "/a/b",
            "example.com/a/../b",
            "example.com/a/./b",
            "example.com:443/a/%2Fb",
            "example.com:443/a/%5Cb",
            "example.com:443/a?x=1",
            "example.com:443/a#frag",
            "example.com:443\\a",
            "example.com:443/a/%zz",
            "example.com:abc/a/b",
        ] {
            assert!(
                repository_body(&json!({"normalizedUrl":url})).is_err(),
                "{url:?}"
            );
        }
    }
}
