// SPDX-License-Identifier: FSL-1.1-ALv2
//! Transactional webhook domain. Callers authorize the current session and hold the account
//! advisory lock. The caller owns commit/rollback; intake acquires the lock itself.
use crate::{Fault, Principal, Result, account_lock, canonical_id, hash, id, string};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::{PgConnection, Row, postgres::PgRow};

/// Validate once at startup and retain in adapter state. Do not log this structure.
#[derive(Clone)]
pub struct WebhookConfig {
    encryption_key: [u8; 32],
    /// Complete public intake base without a trailing slash. The receiver ID is appended as the
    /// final path segment. See `intake_base` for how the configured URL maps to this value.
    pub base_url: String,
    pub max_webhook_bytes: usize,
}
/// Internal route that serves webhook intake. It is mounted by the HTTP adapter.
const INTAKE_ROUTE: &str = "/webhooks";
/// Map the configured URL to the complete public intake base.
///
/// An empty or root path means the value is an API origin, so the internal intake route is
/// appended. Any explicit non-root path is the complete public intake base and is kept as
/// configured; a custom path therefore needs an operator-managed rewrite to the internal route.
fn intake_base(url: &url::Url, configured: &str) -> String {
    let configured = configured.trim_end_matches('/');
    if url.path().trim_matches('/').is_empty() {
        format!("{configured}{INTAKE_ROUTE}")
    } else {
        configured.into()
    }
}
impl WebhookConfig {
    pub fn new(key: &str, base_url: &str, max_webhook_bytes: usize) -> Result<Self> {
        let key: [u8; 32] = URL_SAFE_NO_PAD
            .decode(key)
            .map_err(|_| Fault::invalid("invalid webhook encryption key"))?
            .try_into()
            .map_err(|_| Fault::invalid("webhook encryption key must contain 32 bytes"))?;
        let url =
            url::Url::parse(base_url).map_err(|_| Fault::invalid("invalid webhook base URL"))?;
        if max_webhook_bytes == 0
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        {
            return Err(Fault::invalid("invalid webhook configuration"));
        }
        Ok(Self {
            encryption_key: key,
            base_url: intake_base(&url, base_url),
            max_webhook_bytes,
        })
    }
    pub fn from_env() -> Result<Self> {
        let read = |name| {
            std::env::var(name).map_err(|_| Fault::precondition("webhook configuration missing"))
        };
        Self::new(
            &read("LANE_WEBHOOK_ENCRYPTION_KEY")?,
            &read("LANE_WEBHOOK_BASE_URL")?,
            read("MAX_WEBHOOK_BYTES")?
                .parse()
                .map_err(|_| Fault::invalid("invalid MAX_WEBHOOK_BYTES"))?,
        )
    }
    /// Public intake endpoint returned for a receiver.
    pub fn endpoint(&self, receiver: &str) -> String {
        format!("{}/{receiver}", self.base_url)
    }
    fn encrypt(&self, account: &str, receiver: &str, key: &[u8]) -> Result<Vec<u8>> {
        let cipher =
            Aes256Gcm::new_from_slice(&self.encryption_key).map_err(|_| Fault::unavailable())?;
        let mut nonce = [0u8; 12];
        rand::rng().fill_bytes(&mut nonce);
        let aad = format!("lane-webhook-key:{account}:{receiver}");
        let encrypted = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: key,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| Fault::unavailable())?;
        Ok([nonce.as_slice(), encrypted.as_slice()].concat())
    }
    fn decrypt(&self, account: &str, receiver: &str, encrypted: &[u8]) -> Result<Vec<u8>> {
        if encrypted.len() != 60 {
            return Err(Fault::unavailable());
        }
        let cipher =
            Aes256Gcm::new_from_slice(&self.encryption_key).map_err(|_| Fault::unavailable())?;
        let aad = format!("lane-webhook-key:{account}:{receiver}");
        cipher
            .decrypt(
                Nonce::from_slice(&encrypted[..12]),
                Payload {
                    msg: &encrypted[12..],
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| Fault::unavailable())
    }
}
fn stale() -> Fault {
    Fault::new(connectrpc::ErrorCode::Aborted, 3, "revision is stale")
}
fn too_large() -> Fault {
    Fault::new(
        connectrpc::ErrorCode::ResourceExhausted,
        15,
        "item exceeds configured byte limit",
    )
}
fn lease_invalid(message: &str) -> Fault {
    Fault::new(connectrpc::ErrorCode::FailedPrecondition, 18, message)
}
fn deleted(message: &str) -> Fault {
    Fault::new(connectrpc::ErrorCode::FailedPrecondition, 7, message)
}
fn random_secret() -> [u8; 32] {
    let mut key = [0; 32];
    rand::rng().fill_bytes(&mut key);
    key
}
fn required<'a>(q: &'a Value, field: &str) -> Result<&'a str> {
    let s = string(q, field);
    if s.is_empty() {
        Err(Fault::invalid(format!("{field} is required")))
    } else {
        Ok(s)
    }
}
fn identity<'a>(q: &'a Value, field: &str) -> Result<&'a str> {
    let s = required(q, field)?;
    if canonical_id(s) {
        Ok(s)
    } else {
        Err(Fault::invalid("invalid UUID v7"))
    }
}
fn revision(q: &Value) -> Result<i64> {
    q["expectedRevision"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| q["expectedRevision"].as_i64())
        .filter(|v| *v > 0)
        .ok_or_else(|| Fault::invalid("expected revision is required"))
}
fn fingerprint(q: &Value) -> Result<String> {
    Ok(hash(&serde_jcs::to_vec(q)?))
}
fn stamp(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Micros, true)
}
async fn clock(db: &mut PgConnection) -> Result<DateTime<Utc>> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(db)
        .await?)
}
fn receiver(row: &PgRow) -> Value {
    json!({"id":row.get::<String,_>("id"),"repositoryId":row.get::<String,_>("repository_id"),
        "name":row.get::<String,_>("name"),"revision":row.get::<i64,_>("revision").to_string(),
        "deleted":row.get::<Option<String>,_>("deletion_version_id").is_some()})
}
fn event(row: &PgRow) -> Value {
    let data: Option<Value> = row.get("data");
    let mut data = data.unwrap_or_else(|| json!({}));
    data["id"] = json!(row.get::<String, _>("id"));
    data["receiverId"] = json!(row.get::<String, _>("receiver_id"));
    data["repositoryId"] = json!(row.get::<String, _>("repository_id"));
    data["senderEventId"] = json!(row.get::<String, _>("sender_event_id"));
    data["state"] = json!(format!("EVENT_STATE_{}", row.get::<String, _>("state")));
    data["revision"] = json!(row.get::<i64, _>("revision").to_string());
    data
}
fn deletion(kind: &str, id: &str, version: &str) -> Value {
    json!({"kind":format!("ENTITY_KIND_OR_REMOTE_KIND_{kind}"),"id":id,"deletionVersionId":version})
}
async fn receiver_row(db: &mut PgConnection, account: &str, id: &str) -> Result<PgRow> {
    Ok(
        sqlx::query(
            "SELECT * FROM lane_webhook_receivers WHERE account_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(account)
        .bind(id)
        .fetch_one(db)
        .await?,
    )
}
async fn event_row(db: &mut PgConnection, account: &str, id: &str) -> Result<PgRow> {
    Ok(
        sqlx::query("SELECT * FROM lane_webhook_events WHERE account_id=$1 AND id=$2 FOR UPDATE")
            .bind(account)
            .bind(id)
            .fetch_one(db)
            .await?,
    )
}
fn live_event(row: &PgRow) -> Result<()> {
    if row.get::<String, _>("state") == "DELETED" {
        Err(deleted("event is deleted"))
    } else {
        Ok(())
    }
}
async fn live_repository(db: &mut PgConnection, p: &Principal, repository: &str) -> Result<()> {
    let result = crate::entities::dispatch(
        db,
        p,
        "GetRepository",
        &json!({"id":repository}),
        usize::MAX,
    )
    .await?;
    if result["result"]["version"]["deleted"]
        .as_bool()
        .unwrap_or(false)
        || result["result"]["version"].is_null()
    {
        Err(deleted("repository is deleted"))
    } else {
        Ok(())
    }
}

/// Exact common dispatch interface. Prefer `dispatch_with_config` in adapters with immutable state.
pub async fn dispatch(
    db: &mut PgConnection,
    p: &Principal,
    method: &str,
    q: &Value,
    max_item_bytes: usize,
) -> Result<Value> {
    let config = if matches!(method, "CreateReceiver" | "RotateReceiverKey") {
        Some(WebhookConfig::from_env()?)
    } else {
        None
    };
    dispatch_with_config(db, p, method, q, max_item_bytes, config.as_ref()).await
}
/// Does not commit. On any error the caller must roll back its transaction.
pub async fn dispatch_with_config(
    db: &mut PgConnection,
    p: &Principal,
    method: &str,
    q: &Value,
    max_item_bytes: usize,
    config: Option<&WebhookConfig>,
) -> Result<Value> {
    if matches!(method, "ListReceivers" | "ListEvents") {
        return list(db, p, method, q).await;
    }
    let resource = identity(q, "id")?;
    if method == "GetEvent" {
        let row = event_row(db, &p.account_id, resource).await?;
        live_event(&row)?;
        let processing:Vec<Value> = sqlx::query_scalar("SELECT data FROM lane_webhook_processing WHERE account_id=$1 AND event_id=$2 ORDER BY id")
            .bind(&p.account_id).bind(resource).fetch_all(db).await?;
        return Ok(json!({"event":event(&row),"processing":processing}));
    }
    let request_id = identity(&q["meta"], "requestId")?;
    let mut normalized = q.clone();
    normalized["meta"] = json!({"requestId":request_id,"origin":p.origin()});
    // A lease token is a credential, not durable replay content. Hash before fingerprinting.
    if let Some(token) = q["leaseToken"].as_str() {
        normalized["leaseToken"] = json!(hash(token.as_bytes()));
    }
    match method {
        "CreateReceiver" | "SaveReceiver" => normalized["name"] = json!(string(q, "name")),
        "ReleaseEventLease" => normalized["errorReason"] = json!(string(q, "errorReason")),
        "CompleteEvent" => {
            if normalized.get("tasks").is_none() {
                normalized["tasks"] = json!([]);
            }
            if let Some(tasks) = normalized["tasks"].as_array_mut() {
                for task in tasks {
                    let request_id = string(&task["meta"], "requestId").to_owned();
                    task["meta"] = json!({"requestId":request_id,"origin":p.origin()});
                    task["description"] = json!(string(task, "description"));
                    if task.get("progress").is_none() || task["progress"] == "PROGRESS_UNSPECIFIED"
                    {
                        task["progress"] = json!("PROGRESS_TODO");
                    }
                    if task.get("proposal").is_none() || task["proposal"] == "PROPOSAL_UNSPECIFIED"
                    {
                        task["proposal"] = json!("PROPOSAL_REGULAR");
                    }
                    if let Some(url) = task["repository"]["url"].as_str() {
                        task["repository"]["url"] = json!(crate::entities::normalize_url(url)?);
                    }
                }
            }
        }
        _ => {}
    }
    if matches!(method, "SaveReceiver" | "RotateReceiverKey" | "ReopenEvent") {
        normalized["expectedRevision"] = json!(revision(q)?.to_string());
    }
    let digest = fingerprint(&json!({"method":method,"request":normalized}))?;
    if let Some(receipt) = sqlx::query("SELECT fingerprint,response FROM lane_webhook_receipts WHERE account_id=$1 AND method=$2 AND request_id=$3")
        .bind(&p.account_id).bind(method).bind(request_id).fetch_optional(&mut *db).await? {
        if receipt.get::<String,_>("fingerprint") != digest { return Err(Fault::conflict()); }
        if method == "ClaimEvent" { return Err(Fault::precondition("lease token was already issued; release or wait for expiry")); }
        return Ok(receipt.get("response"));
    }
    if !matches!(method, "DeleteReceiver" | "DeleteEvent" | "CompleteEvent")
        && serde_json::to_vec(q)?.len() > max_item_bytes
    {
        return Err(too_large());
    }
    let response = match method {
        "CreateReceiver" => {
            let repository = identity(q, "repositoryId")?;
            live_repository(db, p, repository).await?;
            let config =
                config.ok_or_else(|| Fault::precondition("webhook configuration missing"))?;
            let key = random_secret();
            let encrypted = config.encrypt(&p.account_id, resource, &key)?;
            let row = sqlx::query("INSERT INTO lane_webhook_receivers(account_id,id,repository_id,name,encrypted_key) VALUES($1,$2,$3,$4,$5) RETURNING *")
                .bind(&p.account_id).bind(resource).bind(repository).bind(string(q,"name")).bind(encrypted).fetch_one(&mut *db).await?;
            let receiver = receiver(&row);
            crate::entities::change(db, &p.account_id, &json!({"receiver":receiver})).await?;
            json!({"receiver":receiver,"secret":URL_SAFE_NO_PAD.encode(key),"endpoint":config.endpoint(resource)})
        }
        "SaveReceiver" | "RotateReceiverKey" => {
            let row = receiver_row(db, &p.account_id, resource).await?;
            if row
                .get::<Option<String>, _>("deletion_version_id")
                .is_some()
            {
                return Err(deleted("receiver is deleted"));
            }
            if row.get::<i64, _>("revision") != revision(q)? {
                return Err(stale());
            }
            let secret = if method == "RotateReceiverKey" {
                let config =
                    config.ok_or_else(|| Fault::precondition("webhook configuration missing"))?;
                let key = random_secret();
                let encrypted = config.encrypt(&p.account_id, resource, &key)?;
                sqlx::query("UPDATE lane_webhook_receivers SET encrypted_key=$3,revision=revision+1 WHERE account_id=$1 AND id=$2")
                    .bind(&p.account_id).bind(resource).bind(encrypted).execute(&mut *db).await?;
                Some(URL_SAFE_NO_PAD.encode(key))
            } else {
                let repository = identity(q, "repositoryId")?;
                live_repository(db, p, repository).await?;
                sqlx::query("UPDATE lane_webhook_receivers SET repository_id=$3,name=$4,revision=revision+1 WHERE account_id=$1 AND id=$2")
                    .bind(&p.account_id).bind(resource).bind(repository).bind(string(q,"name")).execute(&mut *db).await?;
                None
            };
            let receiver = receiver(&receiver_row(db, &p.account_id, resource).await?);
            crate::entities::change(db, &p.account_id, &json!({"receiver":receiver})).await?;
            let mut response = json!({"receiver":receiver});
            if let Some(secret) = secret {
                response["secret"] = json!(secret);
            }
            response
        }
        "DeleteReceiver" => json!({"deletion":delete_receiver(db,&p.account_id,resource).await?}),
        "DeleteEvent" => json!({"deletion":delete_event(db,&p.account_id,resource).await?}),
        "ClaimEvent" | "RenewEventLease" | "ReleaseEventLease" | "ReopenEvent"
        | "CompleteEvent" => process(db, p, method, q, max_item_bytes).await?,
        _ => return Err(Fault::invalid("unknown webhook method")),
    };
    let mut saved = response.clone();
    if let Some(object) = saved.as_object_mut() {
        object.remove("secret");
        object.remove("leaseToken");
    }
    sqlx::query("INSERT INTO lane_webhook_receipts(account_id,method,request_id,resource_id,fingerprint,response) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(&p.account_id).bind(method).bind(request_id).bind(resource).bind(digest).bind(saved).execute(db).await?;
    Ok(response)
}

async fn process(
    db: &mut PgConnection,
    p: &Principal,
    method: &str,
    q: &Value,
    max_item_bytes: usize,
) -> Result<Value> {
    let resource = identity(q, "id")?;
    let row = event_row(db, &p.account_id, resource).await?;
    live_event(&row)?;
    live_repository(db, p, &row.get::<String, _>("repository_id")).await?;
    let now = clock(db).await?;
    let state: String = row.get("state");
    let mut extra = json!({});
    match method {
        "ClaimEvent" => {
            let expiry: Option<DateTime<Utc>> = row.get("expires_at");
            if state != "PENDING" && !(state == "LEASED" && expiry.is_some_and(|t| t <= now)) {
                return Err(lease_invalid("event cannot be claimed"));
            }
            let token = URL_SAFE_NO_PAD.encode(random_secret());
            let expires = now + chrono::Duration::minutes(5);
            sqlx::query("UPDATE lane_webhook_events SET state='LEASED',lease_hash=$3,claimant=$4,expires_at=$5,last_error=NULL,revision=revision+1 WHERE account_id=$1 AND id=$2")
                .bind(&p.account_id).bind(resource).bind(hash(token.as_bytes())).bind(&p.session_id).bind(expires).execute(&mut *db).await?;
            extra = json!({"leaseToken":token,"expiresAt":stamp(expires)});
        }
        "ReopenEvent" => {
            if state != "COMPLETED" {
                return Err(Fault::precondition("only completed events can be reopened"));
            }
            if revision(q)? != row.get::<i64, _>("revision") {
                return Err(stale());
            }
            sqlx::query("UPDATE lane_webhook_events SET state='PENDING',revision=revision+1 WHERE account_id=$1 AND id=$2")
                .bind(&p.account_id).bind(resource).execute(&mut *db).await?;
        }
        _ => {
            validate_lease(&row, p, required(q, "leaseToken")?, now)?;
            match method {
                "RenewEventLease" => {
                    let expires = now + chrono::Duration::minutes(5);
                    sqlx::query("UPDATE lane_webhook_events SET expires_at=$3 WHERE account_id=$1 AND id=$2")
                        .bind(&p.account_id).bind(resource).bind(expires).execute(db).await?;
                    return Ok(json!({"expiresAt":stamp(expires)}));
                }
                "ReleaseEventLease" => {
                    // Retained reason is user-controlled text; never copy it into logs.
                    sqlx::query("UPDATE lane_webhook_events SET state='PENDING',lease_hash=NULL,claimant=NULL,expires_at=NULL,last_error=$3,revision=revision+1 WHERE account_id=$1 AND id=$2")
                        .bind(&p.account_id).bind(resource).bind(string(q,"errorReason")).execute(&mut *db).await?;
                }
                "CompleteEvent" => {
                    // The savepoint also protects callers that catch a domain error and continue
                    // their outer transaction. It never commits the caller's transaction.
                    sqlx::query("SAVEPOINT lane_webhook_completion")
                        .execute(&mut *db)
                        .await?;
                    let completion = complete(db, p, q, max_item_bytes, now).await;
                    match completion {
                        Ok(value) => {
                            sqlx::query("RELEASE SAVEPOINT lane_webhook_completion")
                                .execute(&mut *db)
                                .await?;
                            extra = value;
                        }
                        Err(error) => {
                            sqlx::query("ROLLBACK TO SAVEPOINT lane_webhook_completion")
                                .execute(&mut *db)
                                .await?;
                            sqlx::query("RELEASE SAVEPOINT lane_webhook_completion")
                                .execute(db)
                                .await?;
                            return Err(error);
                        }
                    }
                }
                _ => return Err(Fault::invalid("unknown lease operation")),
            }
        }
    }
    let value = event(&event_row(db, &p.account_id, resource).await?);
    crate::entities::change(db, &p.account_id, &json!({"event":value})).await?;
    extra["event"] = value;
    Ok(extra)
}
fn validate_lease(row: &PgRow, p: &Principal, token: &str, now: DateTime<Utc>) -> Result<()> {
    if row.get::<String, _>("state") != "LEASED"
        || row.get::<Option<String>, _>("claimant").as_deref() != Some(&p.session_id)
        || row.get::<Option<String>, _>("lease_hash").as_deref() != Some(&hash(token.as_bytes()))
        || !row
            .get::<Option<DateTime<Utc>>, _>("expires_at")
            .is_some_and(|t| now < t)
    {
        return Err(lease_invalid("lease is invalid or expired"));
    }
    Ok(())
}
async fn complete(
    db: &mut PgConnection,
    p: &Principal,
    q: &Value,
    max_item_bytes: usize,
    now: DateTime<Utc>,
) -> Result<Value> {
    let empty = Vec::new();
    let tasks = match q.get("tasks") {
        None => &empty,
        Some(v) => v
            .as_array()
            .ok_or_else(|| Fault::invalid("tasks must be an array"))?,
    };
    let mut results = Vec::with_capacity(tasks.len());
    let mut ids = Vec::with_capacity(tasks.len());
    for task in tasks {
        let result = crate::entities::dispatch(db, p, "CreateTask", task, max_item_bytes).await?;
        let entity = &result["result"];
        if entity["version"].is_null() || entity["version"]["deleted"] == true {
            return Err(Fault::precondition(
                "completion task was deleted or could not be created",
            ));
        }
        ids.push(identity(task, "id")?.to_owned());
        results.push(entity.clone());
    }
    let event = identity(q, "id")?;
    let processing = json!({"id":id(),"eventId":event,"taskIds":ids,"completedAt":stamp(now),"origin":p.origin()});
    sqlx::query(
        "INSERT INTO lane_webhook_processing(account_id,id,event_id,data) VALUES($1,$2,$3,$4)",
    )
    .bind(&p.account_id)
    .bind(string(&processing, "id"))
    .bind(event)
    .bind(&processing)
    .execute(&mut *db)
    .await?;
    sqlx::query("UPDATE lane_webhook_events SET state='COMPLETED',lease_hash=NULL,claimant=NULL,expires_at=NULL,last_error=NULL,revision=revision+1 WHERE account_id=$1 AND id=$2")
        .bind(&p.account_id).bind(event).execute(&mut *db).await?;
    crate::entities::change(db, &p.account_id, &json!({"processing":processing})).await?;
    Ok(json!({"processing":processing,"tasks":results}))
}

async fn list(db: &mut PgConnection, p: &Principal, method: &str, q: &Value) -> Result<Value> {
    let size = q["page"]["size"].as_u64().unwrap_or(0);
    let size = if size == 0 { 100 } else { size.min(1000) };
    let repo = q["repositoryId"].as_str();
    let mut states = Vec::<String>::new();
    if let Some(values) = q["states"].as_array() {
        for value in values {
            states.push(
                match value.as_str() {
                    Some("EVENT_STATE_PENDING") => "PENDING",
                    Some("EVENT_STATE_LEASED") => "LEASED",
                    Some("EVENT_STATE_COMPLETED") => "COMPLETED",
                    Some("EVENT_STATE_DELETED") => "DELETED",
                    _ => return Err(Fault::invalid("invalid event state")),
                }
                .into(),
            );
        }
        states.sort();
        states.dedup();
    }
    let scope = fingerprint(
        &json!({"account":p.account_id,"method":method,"repository":repo,"states":states}),
    )?;
    let cursor = string(&q["page"], "cursor");
    let after = if cursor.is_empty() {
        String::new()
    } else {
        let bytes = URL_SAFE_NO_PAD
            .decode(cursor)
            .map_err(|_| Fault::invalid("invalid cursor"))?;
        let value: Value = serde_json::from_slice(&bytes)?;
        if value["scope"] != scope {
            return Err(Fault::invalid("cursor belongs to another query"));
        }
        identity(&value, "id")?.to_owned()
    };
    let rows = if method == "ListReceivers" {
        sqlx::query("SELECT * FROM lane_webhook_receivers WHERE account_id=$1 AND id>$2 AND ($3::text IS NULL OR repository_id=$3) AND deletion_version_id IS NULL ORDER BY id LIMIT $4")
            .bind(&p.account_id).bind(&after).bind(repo).bind(size as i64+1).fetch_all(db).await?
    } else {
        sqlx::query("SELECT * FROM lane_webhook_events WHERE account_id=$1 AND id>$2 AND ($3::text IS NULL OR repository_id=$3) AND state<>'DELETED' AND (cardinality($5::text[])=0 OR state=ANY($5)) ORDER BY id LIMIT $4")
            .bind(&p.account_id).bind(&after).bind(repo).bind(size as i64+1).bind(states).fetch_all(db).await?
    };
    let next = if rows.len() > size as usize {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(
            &json!({"scope":scope,"id":rows[size as usize-1].get::<String,_>("id")}),
        )?)
    } else {
        String::new()
    };
    let items: Vec<Value> = rows
        .iter()
        .take(size as usize)
        .map(if method == "ListReceivers" {
            receiver
        } else {
            event
        })
        .collect();
    Ok(json!({"items":items,"nextCursor":next}))
}

/// Receiver deletion preserves its event data, processing records and active leases.
pub async fn delete_receiver(
    db: &mut PgConnection,
    account: &str,
    receiver_id: &str,
) -> Result<Value> {
    let row = receiver_row(db, account, receiver_id).await?;
    let version = row
        .get::<Option<String>, _>("deletion_version_id")
        .unwrap_or_else(id);
    let deleted = deletion("RECEIVER", receiver_id, &version);
    sqlx::query("UPDATE lane_webhook_receivers SET encrypted_key=NULL,name='',deletion_version_id=$3,revision=revision+CASE WHEN deletion_version_id IS NULL THEN 1 ELSE 0 END WHERE account_id=$1 AND id=$2")
        .bind(account).bind(receiver_id).bind(version).execute(&mut *db).await?;
    redact_resource(db, account, receiver_id, &deleted).await?;
    sqlx::query("DELETE FROM lane_changes WHERE account_id=$1 AND data->'receiver'->>'id'=$2")
        .bind(account)
        .bind(receiver_id)
        .execute(&mut *db)
        .await?;
    crate::entities::change(db, account, &json!({"deletion":deleted})).await?;
    Ok(deleted)
}
/// Erases payload, processing history, lease credentials and saved response bodies.
pub async fn delete_event(db: &mut PgConnection, account: &str, event_id: &str) -> Result<Value> {
    let row = event_row(db, account, event_id).await?;
    let version = row
        .get::<Option<String>, _>("deletion_version_id")
        .unwrap_or_else(id);
    let deleted = deletion("EVENT", event_id, &version);
    sqlx::query("UPDATE lane_webhook_events SET data=NULL,state='DELETED',lease_hash=NULL,claimant=NULL,expires_at=NULL,last_error=NULL,deletion_version_id=$3,revision=revision+CASE WHEN deletion_version_id IS NULL THEN 1 ELSE 0 END WHERE account_id=$1 AND id=$2")
        .bind(account).bind(event_id).bind(version).execute(&mut *db).await?;
    sqlx::query("DELETE FROM lane_webhook_processing WHERE account_id=$1 AND event_id=$2")
        .bind(account)
        .bind(event_id)
        .execute(&mut *db)
        .await?;
    redact_resource(db, account, event_id, &deleted).await?;
    sqlx::query("DELETE FROM lane_changes WHERE account_id=$1 AND (data->'event'->>'id'=$2 OR data->'processing'->>'eventId'=$2)")
        .bind(account).bind(event_id).execute(&mut *db).await?;
    crate::entities::change(db, account, &json!({"deletion":deleted})).await?;
    Ok(deleted)
}
async fn redact_resource(
    db: &mut PgConnection,
    account: &str,
    resource: &str,
    deletion: &Value,
) -> Result<()> {
    let receiver = deletion["kind"] == "ENTITY_KIND_OR_REMOTE_KIND_RECEIVER";
    let tombstone = if receiver {
        json!({"receiver":{"id":resource,"deleted":true}})
    } else {
        json!({"event":{"id":resource,"state":"EVENT_STATE_DELETED"}})
    };
    sqlx::query("UPDATE lane_webhook_receipts SET response=CASE WHEN method IN ('DeleteEvent','DeleteReceiver') THEN $3 ELSE $4 END WHERE account_id=$1 AND resource_id=$2 AND ((method IN ('CreateReceiver','SaveReceiver','RotateReceiverKey','DeleteReceiver')) = $5)")
        .bind(account).bind(resource).bind(json!({"deletion":deletion})).bind(tombstone).bind(receiver).execute(db).await?;
    Ok(())
}
/// Called by repository deletion in the same account-locked transaction.
pub async fn delete_repository(
    db: &mut PgConnection,
    account: &str,
    repository: &str,
) -> Result<()> {
    let receivers:Vec<String>=sqlx::query_scalar("SELECT id FROM lane_webhook_receivers WHERE account_id=$1 AND repository_id=$2 AND deletion_version_id IS NULL")
        .bind(account).bind(repository).fetch_all(&mut *db).await?;
    for id in receivers {
        delete_receiver(db, account, &id).await?;
    }
    let events:Vec<String>=sqlx::query_scalar("SELECT id FROM lane_webhook_events WHERE account_id=$1 AND repository_id=$2 AND state<>'DELETED'")
        .bind(account).bind(repository).fetch_all(&mut *db).await?;
    for id in events {
        delete_event(db, account, &id).await?;
    }
    Ok(())
}
/// Account deletion runs before account epoch rotation; the caller removes account changes.
pub async fn delete_account(db: &mut PgConnection, account: &str) -> Result<()> {
    for table in [
        "lane_webhook_receipts",
        "lane_webhook_processing",
        "lane_webhook_events",
        "lane_webhook_receivers",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE account_id=$1"))
            .bind(account)
            .execute(&mut *db)
            .await?;
    }
    Ok(())
}
/// Remove deleted task content from completion receipts while preserving processing task IDs.
pub async fn redact_task(db: &mut PgConnection, account: &str, task_id: &str) -> Result<()> {
    sqlx::query("UPDATE lane_webhook_receipts SET response=jsonb_set(response,'{tasks}',(SELECT coalesce(jsonb_agg(CASE WHEN item->'version'->>'entityId'=$2 THEN jsonb_build_object('result','RESULT_KIND_DELETED') ELSE item END),'[]'::jsonb) FROM jsonb_array_elements(response->'tasks') item)) WHERE account_id=$1 AND jsonb_typeof(response->'tasks')='array'")
        .bind(account).bind(task_id).execute(db).await?;
    Ok(())
}

/// Routing lookup only. Intake rechecks the receiver after acquiring the account lock.
pub async fn receiver_account(db: &mut PgConnection, receiver_id: &str) -> Result<String> {
    Ok(sqlx::query_scalar(
        "SELECT account_id FROM lane_webhook_receivers WHERE id=$1 AND deletion_version_id IS NULL",
    )
    .bind(receiver_id)
    .fetch_one(db)
    .await?)
}

/// Signed HTTP intake. Caller begins/ends the transaction and bounds the body before reading it.
/// This function resolves the account, obtains its advisory lock, then rereads the current key.
/// Rate limits and the HTTP response/status mapping belong to the transport adapter.
pub async fn intake(
    db: &mut PgConnection,
    receiver_id: &str,
    raw: &[u8],
    timestamp: &str,
    signature: &str,
    config: &WebhookConfig,
) -> Result<Value> {
    if raw.len() > config.max_webhook_bytes {
        return Err(too_large());
    }
    let account: String =
        sqlx::query_scalar("SELECT account_id FROM lane_webhook_receivers WHERE id=$1")
            .bind(receiver_id)
            .fetch_one(&mut *db)
            .await?;
    account_lock(db, &account).await?;
    let row = receiver_row(db, &account, receiver_id).await?;
    let encrypted: Vec<u8> = row
        .get::<Option<Vec<u8>>, _>("encrypted_key")
        .ok_or_else(Fault::missing)?;
    let key = config.decrypt(&account, receiver_id, &encrypted)?;
    verify_signature(
        &key,
        raw,
        timestamp,
        signature,
        clock(db).await?.timestamp(),
    )?;
    let mut content = unique_json(raw)?;
    let sender = required(&content, "id")?.to_owned();
    let event_type = required(&content, "type")?.to_owned();
    if !content["payload"].is_object() {
        return Err(Fault::invalid("payload must be an object"));
    }
    let occurred = DateTime::parse_from_rfc3339(required(&content, "occurredAt")?)
        .map_err(|_| Fault::invalid("invalid occurrence time"))?
        .with_timezone(&Utc);
    content["occurredAt"] = json!(stamp(occurred));
    let digest = fingerprint(&content)?;
    if let Some(existing)=sqlx::query("SELECT * FROM lane_webhook_events WHERE account_id=$1 AND receiver_id=$2 AND sender_event_id=$3")
        .bind(&account).bind(receiver_id).bind(&sender).fetch_optional(&mut *db).await? {
        if existing.get::<String,_>("fingerprint")!=digest {return Err(Fault::conflict());}
        return Ok(json!({"id":existing.get::<String,_>("id"),"alreadyReceived":true,"deleted":existing.get::<String,_>("state")=="DELETED"}));
    }
    let tenant: String =
        sqlx::query_scalar("SELECT tenant_id FROM lane_accounts WHERE account_id=$1")
            .bind(&account)
            .fetch_one(&mut *db)
            .await?;
    let p = Principal {
        account_id: account.clone(),
        tenant_id: tenant,
        session_id: String::new(),
        device_id: String::new(),
        device_name: String::new(),
        origin_kind: "ORIGIN_KIND_UNSPECIFIED".into(),
    };
    let repository: String = row.get("repository_id");
    live_repository(db, &p, &repository).await?;
    let event_id = id();
    let data =
        json!({"eventType":event_type,"occurredAt":stamp(occurred),"payload":content["payload"]});
    let row=sqlx::query("INSERT INTO lane_webhook_events(account_id,id,receiver_id,repository_id,sender_event_id,fingerprint,data,state) VALUES($1,$2,$3,$4,$5,$6,$7,'PENDING') RETURNING *")
        .bind(&account).bind(&event_id).bind(receiver_id).bind(repository).bind(sender).bind(digest).bind(data).fetch_one(&mut *db).await?;
    crate::entities::change(db, &account, &json!({"event":event(&row)})).await?;
    Ok(json!({"id":event_id,"alreadyReceived":false,"deleted":false}))
}
/// Checks exact transport bytes with constant-time HMAC verification.
pub fn verify_signature(
    key: &[u8],
    raw: &[u8],
    timestamp: &str,
    signature: &str,
    now_seconds: i64,
) -> Result<()> {
    let seconds: i64 = timestamp.parse().map_err(|_| Fault::unauthenticated())?;
    if seconds.to_string() != timestamp || seconds.abs_diff(now_seconds) > 300 {
        return Err(Fault::unauthenticated());
    }
    let encoded = signature
        .strip_prefix("v1=")
        .ok_or_else(Fault::unauthenticated)?;
    if encoded.len() != 64
        || !encoded
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Fault::unauthenticated());
    }
    let bytes = hex::decode(encoded).map_err(|_| Fault::unauthenticated())?;
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|_| Fault::unauthenticated())?;
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(raw);
    mac.verify_slice(&bytes)
        .map_err(|_| Fault::unauthenticated())
}

// Deserialize every nested object with duplicate detection before a Value can discard keys.
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = Unique;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON with unique object keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Unique, E> {
                Ok(Unique(json!(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Unique, E> {
                Ok(Unique(json!(v as f64)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Unique, E> {
                Ok(Unique(json!(v as f64)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Unique, E> {
                serde_json::Number::from_f64(v)
                    .map(|v| Unique(Value::Number(v)))
                    .ok_or_else(|| E::custom("nonfinite JSON number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Unique, E> {
                Ok(Unique(json!(v)))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Unique, E> {
                Ok(Unique(json!(v)))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut values = Vec::new();
                while let Some(Unique(v)) = seq.next_element()? {
                    values.push(v);
                }
                Ok(Unique(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                    let Unique(v) = map.next_value()?;
                    values.insert(key, v);
                }
                Ok(Unique(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}
pub fn unique_json(raw: &[u8]) -> Result<Value> {
    Ok(serde_json::from_slice::<Unique>(raw)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(base: &str) -> Result<WebhookConfig> {
        WebhookConfig::new(&URL_SAFE_NO_PAD.encode([7; 32]), base, 1024)
    }
    #[test]
    fn origin_bases_append_the_intake_route_once() {
        let id = "01900000-0000-7000-8000-000000000010";
        for base in [
            "https://lane.api.delino.io",
            "https://lane.api.delino.io/",
            "http://127.0.0.1:8080",
            "http://127.0.0.1:8080/",
        ] {
            let endpoint = config(base).unwrap().endpoint(id);
            assert_eq!(
                endpoint,
                format!("{}/webhooks/{id}", base.trim_end_matches('/'))
            );
        }
    }
    #[test]
    fn explicit_paths_are_the_complete_intake_base() {
        let id = "01900000-0000-7000-8000-000000000010";
        for (base, expected) in [
            (
                "https://lane.example/webhooks",
                "https://lane.example/webhooks",
            ),
            (
                "https://lane.example/webhooks/",
                "https://lane.example/webhooks",
            ),
            ("https://lane.example/hooks", "https://lane.example/hooks"),
            ("https://lane.example/hooks/", "https://lane.example/hooks"),
            ("https://lane.example/a/b/", "https://lane.example/a/b"),
        ] {
            assert_eq!(
                config(base).unwrap().endpoint(id),
                format!("{expected}/{id}")
            );
        }
    }
    #[test]
    fn base_url_validation_is_unchanged() {
        for base in [
            "https://user@lane.example",
            "https://user:pass@lane.example/webhooks",
            "https://lane.example/webhooks?x=1",
            "https://lane.example/#fragment",
            "http://lane.example",
            "ftp://lane.example",
            "lane.example",
        ] {
            assert!(config(base).is_err(), "{base} must stay invalid");
        }
        assert!(config("http://localhost:8080").is_ok());
        assert!(config("http://[::1]:8080/").is_ok());
    }
    #[test]
    fn shared_signature_vectors() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../../protos/lane/v1/fixtures/webhooks.json"
        ))
        .unwrap();
        assert_eq!(fixtures["formatVersion"], 1);
        let mut count = 0;
        for case in fixtures["cases"].as_array().unwrap() {
            let input = &case["input"];
            if input["operation"] != "verifySignature" {
                continue;
            }
            count += 1;
            let key = URL_SAFE_NO_PAD
                .decode(string(input, "secretBase64url"))
                .unwrap();
            let result = verify_signature(
                &key,
                string(input, "rawBody").as_bytes(),
                string(input, "timestamp"),
                string(input, "signature"),
                string(input, "nowSeconds").parse().unwrap(),
            );
            assert_eq!(
                result.is_ok(),
                case["expected"]["accepted"].as_bool().unwrap(),
                "{}",
                case["id"]
            );
        }
        assert_eq!(count, 9);
    }
    #[test]
    fn canonical_json_and_duplicate_rejection() {
        for raw in [
            r#"{"x":1,"x":2}"#,
            r#"{"payload":{"x":1,"\u0078":2}}"#,
            r#"{"payload":[{"x":1,"x":2}]}"#,
            r#"{"x":"\udead"}"#,
            r#"{"x":1e999}"#,
        ] {
            assert!(unique_json(raw.as_bytes()).is_err(), "{raw}");
        }
        let value = unique_json(
            br#"{"numbers":[333333333.33333329,1E30,4.50,2e-3,0.000000000000000000000000001,-0]}"#,
        )
        .unwrap();
        assert_eq!(
            serde_jcs::to_string(&value).unwrap(),
            r#"{"numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27,0]}"#
        );
        let value = json!({"\u{e000}":1,"\u{1f600}":2});
        assert_eq!(
            serde_jcs::to_string(&value).unwrap(),
            "{\"😀\":2,\"\u{e000}\":1}"
        );
    }
    #[test]
    fn authenticated_encryption_binds_account_and_receiver() {
        let config = WebhookConfig::new(
            &URL_SAFE_NO_PAD.encode([7; 32]),
            "https://lane.example/hooks",
            1024,
        )
        .unwrap();
        let encrypted = config.encrypt("a", "r", &[9; 32]).unwrap();
        assert_ne!(&encrypted[..32], &[9; 32]);
        assert_eq!(config.decrypt("a", "r", &encrypted).unwrap(), [9; 32]);
        assert!(config.decrypt("b", "r", &encrypted).is_err());
        assert!(config.decrypt("a", "s", &encrypted).is_err());
        let mut corrupt = encrypted;
        corrupt[30] ^= 1;
        assert!(config.decrypt("a", "r", &corrupt).is_err());
    }
}
