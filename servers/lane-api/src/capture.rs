// SPDX-License-Identifier: FSL-1.1-ALv2
//! Account-locked attachment manifests, private object transfer and URL rules.
use crate::entities::{self as e, fault};
use crate::{Fault, Principal, Result, hash, id, now, string};
use base64::Engine;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};

const AVAILABLE: &str = "ATTACHMENT_STATE_AVAILABLE";
const WAITING: &str = "ATTACHMENT_STATE_WAITING_FOR_SYNC";
const DELETED: &str = "ATTACHMENT_STATE_DELETED";

pub fn rule_body(v: &Value) -> Result<Value> {
    let input = string(v, "prefix");
    if input.trim() != input || input.contains('\\') || input.chars().any(char::is_control) {
        return Err(fault(1, "invalid URL prefix"));
    }
    let mut url = url::Url::parse(input).map_err(|_| fault(1, "invalid URL prefix"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(fault(1, "HTTP(S) URL without credentials required"));
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(if path.is_empty() { "/" } else { &path });
    e::uuid(string(v, "repositoryId"))?;
    Ok(json!({"prefix":url.as_str(),"repositoryId":v["repositoryId"]}))
}
pub(crate) async fn validate_rule(
    db: &mut PgConnection,
    account: &str,
    entity: &str,
    body: &Value,
) -> Result<()> {
    let duplicate: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM lane_entities e JOIN lane_heads h ON h.account_id=e.account_id AND h.entity_id=e.id JOIN lane_versions v ON v.account_id=h.account_id AND v.version_id=h.version_id WHERE e.account_id=$1 AND e.kind=3 AND NOT e.deleted AND e.id<>$2 AND v.envelope->'urlConnection'->>'prefix'=$3)").bind(account).bind(entity).bind(string(body,"prefix")).fetch_one(&mut *db).await?;
    if duplicate {
        return Err(fault(6, "URL prefix is already registered"));
    }
    let repo = e::require_repository(db, account, string(body, "repositoryId")).await?;
    if e::heads(db, account, &repo).await?.len() != 1 {
        return Err(fault(4, "target repository is conflicted"));
    }
    Ok(())
}
fn size(v: &Value) -> Result<usize> {
    v["byteSize"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v["byteSize"].as_u64().and_then(|n| n.try_into().ok()))
        .filter(|n| *n > 0 && (*n as u128) <= i64::MAX as u128)
        .ok_or_else(|| fault(1, "invalid PNG byte size"))
}
fn manifest(v: &Value) -> Result<Value> {
    e::uuid(string(v, "attachmentId"))?;
    for key in ["width", "height"] {
        if !v[key]
            .as_u64()
            .is_some_and(|n| n > 0 && n <= i32::MAX as u64)
        {
            return Err(fault(1, "invalid PNG dimensions"));
        }
    }
    let digest = string(v, "sha256");
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(fault(1, "invalid PNG digest"));
    }
    Ok(
        json!({"attachmentId":v["attachmentId"],"width":v["width"],"height":v["height"],"sha256":digest,"byteSize":size(v)?.to_string()}),
    )
}
pub fn references(v: &Value) -> Result<Value> {
    let items = v
        .as_array()
        .ok_or_else(|| fault(1, "captures must be an array"))?;
    let mut ids = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let mut value = manifest(item)?;
        if !ids.insert(string(item, "attachmentId")) {
            return Err(fault(1, "duplicate attachment reference"));
        }
        for key in ["pageUrl", "pageTitle", "cssSelector"] {
            if let Some(text) = item.get(key).filter(|v| !v.is_null()) {
                if !text.is_string() {
                    return Err(fault(1, "capture metadata must be text"));
                }
                value[key] = text.clone();
            }
        }
        out.push(value);
    }
    Ok(json!(out))
}
pub(crate) async fn validate_ownership(
    db: &mut PgConnection,
    account: &str,
    task: &str,
    body: &Value,
) -> Result<()> {
    for capture in body["captures"].as_array().into_iter().flatten() {
        let expected = manifest(capture)?;
        if let Some(row) = sqlx::query(
            "SELECT task_id,fingerprint FROM lane_attachment_identities WHERE account_id=$1 AND id=$2",
        )
        .bind(account)
        .bind(string(capture, "attachmentId"))
        .fetch_optional(&mut *db)
        .await?
            && (row.get::<String, _>("task_id") != task
                || row.get::<String, _>("fingerprint") != e::fingerprint(&expected)?)
        {
            return Err(fault(13, "attachment identity or owner mismatch"));
        }
    }
    Ok(())
}
pub(crate) async fn register_manifests(
    db: &mut PgConnection,
    account: &str,
    task: &str,
    body: &Value,
) -> Result<()> {
    validate_ownership(db, account, task, body).await?;
    for capture in body["captures"].as_array().into_iter().flatten() {
        sqlx::query("INSERT INTO lane_attachment_identities(account_id,id,task_id,fingerprint) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(account).bind(string(capture,"attachmentId")).bind(task).bind(e::fingerprint(&manifest(capture)?)?).execute(&mut *db).await?;
        sqlx::query("INSERT INTO lane_attachments(account_id,id,task_id,manifest,object_key) VALUES($1,$2,$3,$4,$5) ON CONFLICT(account_id,id) DO NOTHING").bind(account).bind(string(capture,"attachmentId")).bind(task).bind(manifest(capture)?).bind(format!("captures/{}",id())).execute(&mut *db).await?;
    }
    Ok(())
}
/// Deletion revokes new access immediately and durably queues every provider object.
/// Staging deletion waits until the signed PUT has expired so delayed PUTs cannot resurrect it.
pub(crate) async fn delete_task(db: &mut PgConnection, account: &str, task: &str) -> Result<()> {
    sqlx::query("INSERT INTO lane_attachment_garbage(object_key) SELECT object_key FROM lane_attachments WHERE account_id=$1 AND task_id=$2 ON CONFLICT DO NOTHING").bind(account).bind(task).execute(&mut *db).await?;
    sqlx::query("INSERT INTO lane_attachment_garbage(object_key,delete_after) SELECT u.object_key,u.expires_at + interval '1 minute' FROM lane_attachment_uploads u JOIN lane_attachments a ON a.account_id=u.account_id AND a.id=u.attachment_id WHERE a.account_id=$1 AND a.task_id=$2 ON CONFLICT DO NOTHING").bind(account).bind(task).execute(&mut *db).await?;
    sqlx::query("DELETE FROM lane_attachments WHERE account_id=$1 AND task_id=$2")
        .bind(account)
        .bind(task)
        .execute(db)
        .await?;
    Ok(())
}
pub(crate) async fn delete_account(db: &mut PgConnection, account: &str) -> Result<()> {
    let tasks: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT task_id FROM lane_attachments WHERE account_id=$1")
            .bind(account)
            .fetch_all(&mut *db)
            .await?;
    for task in tasks {
        delete_task(db, account, &task).await?;
    }
    Ok(())
}

pub fn validate_png(capture: &Value, bytes: &[u8], decode_limit: usize) -> Result<()> {
    manifest(capture)?;
    if bytes.len() != size(capture)?
        || hash(bytes) != string(capture, "sha256")
        || bytes.len() < 33
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[8..16] != b"\0\0\0\rIHDR"
    {
        return Err(fault(1, "PNG does not match manifest"));
    }
    if u32::from_be_bytes(bytes[16..20].try_into().unwrap()) as u64
        != capture["width"].as_u64().unwrap()
        || u32::from_be_bytes(bytes[20..24].try_into().unwrap()) as u64
            != capture["height"].as_u64().unwrap()
    {
        return Err(fault(1, "PNG dimensions do not match manifest"));
    }
    let mut offset = 8usize;
    let mut data = false;
    while offset < bytes.len() {
        let header = bytes
            .get(offset..offset.saturating_add(8))
            .ok_or_else(|| fault(1, "truncated PNG"))?;
        let n = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(12)
            .and_then(|v| v.checked_add(n))
            .ok_or_else(|| fault(1, "invalid PNG chunk"))?;
        let chunk = bytes
            .get(offset..end)
            .ok_or_else(|| fault(1, "truncated PNG"))?;
        let mut crc = !0u32;
        for b in &chunk[4..8 + n] {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320u32 & 0u32.wrapping_sub(crc & 1));
            }
        }
        if !crc != u32::from_be_bytes(chunk[8 + n..].try_into().unwrap()) {
            return Err(fault(1, "invalid PNG checksum"));
        }
        if &header[4..8] == b"IDAT" {
            data = true;
        }
        if &header[4..8] == b"IEND" {
            return if data && n == 0 && end == bytes.len() {
                decode_png(bytes, decode_limit)
            } else {
                Err(fault(1, "invalid final PNG boundary"))
            };
        }
        offset = end;
    }
    Err(fault(1, "missing PNG boundary"))
}

fn decode_error(error: png::DecodingError) -> Fault {
    if matches!(error, png::DecodingError::LimitsExceeded) {
        fault(15, "decoded PNG exceeds configured byte limit")
    } else {
        fault(1, "invalid PNG pixel stream")
    }
}
fn decode_png(bytes: &[u8], decode_limit: usize) -> Result<()> {
    // Decode rows to verify the compressed stream without allocating a full pixel canvas.
    // PNG dimensions and operator byte limits govern input; no product image-count quota.
    let decoder = png::Decoder::new_with_limits(
        std::io::Cursor::new(bytes),
        png::Limits {
            bytes: decode_limit,
        },
    );
    let mut reader = decoder.read_info().map_err(decode_error)?;
    if reader.output_buffer_size().is_none_or(|n| n > decode_limit) {
        return Err(fault(15, "decoded PNG exceeds configured byte limit"));
    }
    while reader.next_row().map_err(decode_error)?.is_some() {}
    reader.finish().map_err(decode_error)?;
    Ok(())
}

async fn create(db: &mut PgConnection, p: &Principal, q: &Value, max: usize) -> Result<Value> {
    e::uuid(string(q, "id"))?;
    e::uuid(string(&q["meta"], "requestId"))?;
    let mut normalized = q.clone();
    normalized["meta"]["origin"] = p.origin();
    let digest = e::fingerprint(&normalized)?;
    if let Some(r) = e::receipt(
        db,
        &p.account_id,
        "CreateCaptureTask",
        string(&q["meta"], "requestId"),
        &digest,
    )
    .await?
    {
        return Ok(r);
    }
    if let Some(existing) = e::entity(db, &p.account_id, string(q, "id")).await? {
        if existing.deleted {
            return Ok(json!({"result":e::result(db,&p.account_id,string(q,"id")).await?}));
        }
        return Err(fault(6, "task ID already exists"));
    }
    let images = q["images"]
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| fault(1, "at least one image is required"))?;
    let mut captures = Vec::new();
    let mut decoded = Vec::new();
    for image in images {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(string(image, "png"))
            .map_err(|_| fault(1, "invalid PNG encoding"))?;
        if bytes.len() > max {
            return Err(fault(15, "attachment exceeds configured byte limit"));
        }
        validate_png(&image["capture"], &bytes, max)?;
        captures.push(image["capture"].clone());
        decoded.push(bytes);
    }
    let mut task = json!({"meta":q["meta"],"id":q["id"],"repository":{"id":q["repositoryId"]},"title":q["title"],"description":"","prompt":string(q,"description"),"captures":captures});
    task["captures"] = references(&task["captures"])?;
    let mut result = e::dispatch(db, p, "CreateTask", &task, max).await?;
    result.as_object_mut().unwrap().remove("repository");
    for (capture, bytes) in captures.iter().zip(decoded) {
        sqlx::query("UPDATE lane_attachments SET pending_png=$3 WHERE account_id=$1 AND id=$2 AND NOT available").bind(&p.account_id).bind(string(capture,"attachmentId")).bind(bytes).execute(&mut *db).await?;
    }
    e::save_receipt(
        db,
        &p.account_id,
        "CreateCaptureTask",
        string(&q["meta"], "requestId"),
        &digest,
        string(q, "id"),
        &result,
    )
    .await?;
    Ok(result)
}

pub async fn dispatch(
    db: &mut PgConnection,
    p: &Principal,
    method: &str,
    q: &Value,
    max: usize,
) -> Result<Value> {
    if matches!(
        method,
        "ListURLConnections" | "CreateURLConnection" | "SaveURLConnection" | "DeleteURLConnection"
    ) {
        return e::dispatch(db, p, method, q, max).await;
    }
    if method == "CreateCaptureTask" {
        return create(db, p, q, max).await;
    }
    if !matches!(
        method,
        "GetAttachment" | "BeginAttachmentUpload" | "CompleteAttachmentUpload"
    ) {
        return Err(fault(1, "unknown Capture method"));
    }
    let task = string(q, "taskId");
    let attachment = string(q, "attachmentId");
    e::uuid(task)?;
    e::uuid(attachment)?;
    let entity = e::entity(db, &p.account_id, task)
        .await?
        .ok_or_else(|| fault(2, "task not found"))?;
    if entity.kind != e::TASK {
        return Err(fault(2, "task not found"));
    }
    if entity.deleted {
        return Ok(json!({"state":DELETED}));
    }
    let row=sqlx::query("SELECT manifest,object_key,available FROM lane_attachments WHERE account_id=$1 AND id=$2 AND task_id=$3").bind(&p.account_id).bind(attachment).bind(task).fetch_optional(&mut *db).await?.ok_or_else(||fault(2,"attachment not found"))?;
    let available: bool = row.get("available");
    let key: String = row.get("object_key");
    let capture: Value = row.get("manifest");
    if method == "GetAttachment" {
        if !available {
            return Ok(json!({"state":WAITING}));
        }
        let storage = ObjectStore::from_env()?;
        let expiry = now() + chrono::Duration::seconds(60);
        return Ok(
            json!({"state":AVAILABLE,"downloadUrl":storage.sign("GET",&key,60,None)?.0,"expiresAt":expiry.to_rfc3339()}),
        );
    }
    let epoch: Option<String> =
        sqlx::query_scalar("SELECT epoch FROM lane_accounts WHERE account_id=$1")
            .bind(&p.account_id)
            .fetch_optional(&mut *db)
            .await?;
    if epoch.as_deref() != Some(string(q, "epoch")) {
        return Err(fault(17, "stale account epoch"));
    }
    if available {
        return Ok(json!({"state":AVAILABLE}));
    }
    if size(&capture)? > max {
        return Err(fault(15, "attachment exceeds configured byte limit"));
    }
    let storage = ObjectStore::from_env()?;
    if method == "BeginAttachmentUpload" {
        let upload = id();
        let temp = format!("uploads/{upload}");
        let expires = now() + chrono::Duration::seconds(300);
        let (url, headers) = storage.sign("PUT", &temp, 300, Some(&capture))?;
        sqlx::query("INSERT INTO lane_attachment_uploads(account_id,id,attachment_id,object_key,expires_at) VALUES($1,$2,$3,$4,$5)").bind(&p.account_id).bind(&upload).bind(attachment).bind(&temp).bind(expires).execute(&mut *db).await?;
        // Enqueue at allocation, including successful uploads, abandoned uploads and lost responses.
        sqlx::query("INSERT INTO lane_attachment_garbage(object_key,delete_after) VALUES($1,$2)")
            .bind(&temp)
            .bind(expires + chrono::Duration::minutes(1))
            .execute(&mut *db)
            .await?;
        Ok(
            json!({"state":WAITING,"uploadId":upload,"uploadUrl":url,"headers":headers,"expiresAt":expires.to_rfc3339()}),
        )
    } else {
        let temp:Option<String>=sqlx::query_scalar("SELECT object_key FROM lane_attachment_uploads WHERE account_id=$1 AND id=$2 AND attachment_id=$3 AND expires_at>now()").bind(&p.account_id).bind(string(q,"uploadId")).bind(attachment).fetch_optional(&mut *db).await?;
        let bytes = storage
            .get(
                &temp.ok_or_else(|| fault(1, "upload expired or not found"))?,
                size(&capture)?,
            )
            .await?;
        validate_png(&capture, &bytes, max)?;
        storage.put(&key, &bytes).await?;
        sqlx::query("UPDATE lane_attachments SET available=true,pending_png=NULL WHERE account_id=$1 AND id=$2").bind(&p.account_id).bind(attachment).execute(&mut *db).await?;
        e::emit(db, &p.account_id, task).await?;
        Ok(json!({"state":AVAILABLE}))
    }
}

/// S3 Signature V4, path-style R2 endpoint, region `auto`. Never log signed URLs.
#[derive(Clone)]
pub struct ObjectStore {
    endpoint: String,
    bucket: String,
    access: String,
    secret: String,
    client: reqwest::Client,
}
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn mac(key: &[u8], value: &str) -> Vec<u8> {
    use hmac::Mac;
    let mut h = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("HMAC key");
    h.update(value.as_bytes());
    h.finalize().into_bytes().to_vec()
}
impl ObjectStore {
    pub fn from_env() -> Result<Self> {
        let get = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|s| !s.is_empty())
                .ok_or_else(Fault::unavailable)
        };
        Self::new(
            get("LANE_R2_ENDPOINT")?,
            get("LANE_R2_BUCKET")?,
            get("LANE_R2_ACCESS_KEY_ID")?,
            get("LANE_R2_SECRET_ACCESS_KEY")?,
        )
    }
    fn new(endpoint: String, bucket: String, access: String, secret: String) -> Result<Self> {
        let url = url::Url::parse(&endpoint).map_err(|_| Fault::unavailable())?;
        if !(url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))))
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || bucket.is_empty()
            || !bucket
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-.".contains(&b))
        {
            return Err(Fault::unavailable());
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|_| Fault::unavailable())?;
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').into(),
            bucket,
            access,
            secret,
            client,
        })
    }
    fn sign(
        &self,
        method: &str,
        key: &str,
        seconds: i64,
        capture: Option<&Value>,
    ) -> Result<(String, Value)> {
        self.sign_at(method, key, seconds, capture, now())
    }
    fn sign_at(
        &self,
        method: &str,
        key: &str,
        seconds: i64,
        capture: Option<&Value>,
        time: chrono::DateTime<chrono::Utc>,
    ) -> Result<(String, Value)> {
        let date = time.format("%Y%m%d").to_string();
        let stamp = time.format("%Y%m%dT%H%M%SZ").to_string();
        let scope = format!("{date}/auto/s3/aws4_request");
        let url = url::Url::parse(&self.endpoint).map_err(|_| Fault::unavailable())?;
        let host = &url[url::Position::BeforeHost..url::Position::AfterPort];
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("host", host.to_string());
        if let Some(capture) = capture {
            headers.insert("content-length", size(capture)?.to_string());
            headers.insert("content-type", "image/png".into());
        }
        let signed = headers.keys().copied().collect::<Vec<_>>().join(";");
        let canonical_headers = headers
            .iter()
            .map(|(k, v)| format!("{k}:{v}\n"))
            .collect::<String>();
        let mut query = std::collections::BTreeMap::new();
        query.insert("X-Amz-Algorithm", "AWS4-HMAC-SHA256".to_owned());
        query.insert("X-Amz-Credential", format!("{}/{scope}", self.access));
        query.insert("X-Amz-Date", stamp.clone());
        query.insert("X-Amz-Expires", seconds.to_string());
        query.insert("X-Amz-SignedHeaders", signed.clone());
        let query = query
            .iter()
            .map(|(k, v)| format!("{k}={}", encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let path = format!(
            "/{}/{}",
            encode(&self.bucket),
            key.split('/').map(encode).collect::<Vec<_>>().join("/")
        );
        let canonical =
            format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed}\nUNSIGNED-PAYLOAD");
        let signing = mac(
            &mac(
                &mac(
                    &mac(format!("AWS4{}", self.secret).as_bytes(), &date),
                    "auto",
                ),
                "s3",
            ),
            "aws4_request",
        );
        let signature = hex::encode(mac(
            &signing,
            &format!(
                "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
                hash(canonical.as_bytes())
            ),
        ));
        headers.remove("host");
        Ok((
            format!(
                "{}{path}?{query}&X-Amz-Signature={signature}",
                self.endpoint
            ),
            json!(headers),
        ))
    }
    async fn get(&self, key: &str, expected: usize) -> Result<Vec<u8>> {
        let mut response = self
            .client
            .get(self.sign("GET", key, 60, None)?.0)
            .send()
            .await
            .map_err(|_| Fault::unavailable())?;
        if !response.status().is_success() {
            return Err(Fault::unavailable());
        }
        if response
            .content_length()
            .is_some_and(|n| n != expected as u64)
        {
            return Err(fault(1, "uploaded byte size mismatch"));
        }
        let mut out = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Fault::unavailable())? {
            if out.len().saturating_add(chunk.len()) > expected {
                return Err(fault(1, "uploaded byte size mismatch"));
            }
            out.extend_from_slice(&chunk);
        }
        if out.len() != expected {
            return Err(fault(1, "uploaded byte size mismatch"));
        }
        Ok(out)
    }
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let response = self
            .client
            .put(self.sign("PUT", key, 60, None)?.0)
            .header("content-type", "image/png")
            .body(bytes.to_vec())
            .send()
            .await
            .map_err(|_| Fault::unavailable())?;
        if !response.status().is_success() {
            return Err(Fault::unavailable());
        }
        Ok(())
    }
    async fn delete(&self, key: &str) -> Result<()> {
        let response = self
            .client
            .delete(self.sign("DELETE", key, 60, None)?.0)
            .send()
            .await
            .map_err(|_| Fault::unavailable())?;
        if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_FOUND {
            return Err(Fault::unavailable());
        }
        Ok(())
    }
}

/// Each iteration commits durable progress independently. Account locking serializes
/// object publication with task/account deletion, including across worker replicas.
pub async fn tick(pool: &sqlx::PgPool, storage: &ObjectStore) -> Result<()> {
    let pending:Vec<(String,String)>=sqlx::query_as("SELECT account_id,id FROM lane_attachments WHERE pending_png IS NOT NULL AND NOT available AND attempt_after<=now() ORDER BY attempt_after,account_id,id LIMIT 16").fetch_all(pool).await?;
    for (account, attachment) in pending {
        let mut tx = pool.begin().await?;
        crate::account_lock(&mut tx, &account).await?;
        if let Some(row)=sqlx::query("SELECT object_key,pending_png,task_id FROM lane_attachments WHERE account_id=$1 AND id=$2 AND pending_png IS NOT NULL AND NOT available").bind(&account).bind(&attachment).fetch_optional(&mut *tx).await? {
            // Pending bytes passed decoder limits in the creating transaction. A later limit
            // reduction must not prevent publication of already accepted content.
            let bytes:Vec<u8>=row.get("pending_png");
            match storage.put(&row.get::<String,_>("object_key"),&bytes).await {
                Ok(()) => {
                    sqlx::query("UPDATE lane_attachments SET available=true,pending_png=NULL WHERE account_id=$1 AND id=$2").bind(&account).bind(&attachment).execute(&mut *tx).await?;
                    e::emit(&mut tx,&account,&row.get::<String,_>("task_id")).await?;
                }
                Err(_) => {
                    sqlx::query("UPDATE lane_attachments SET attempt_after=now()+interval '30 seconds' WHERE account_id=$1 AND id=$2").bind(&account).bind(&attachment).execute(&mut *tx).await?;
                    tracing::warn!(category="attachment_upload", "provider transfer remains pending");
                }
            }
        }
        tx.commit().await?;
    }
    let keys:Vec<String>=sqlx::query_scalar("SELECT object_key FROM lane_attachment_garbage WHERE delete_after<=now() ORDER BY delete_after LIMIT 64").fetch_all(pool).await?;
    for key in keys {
        if storage.delete(&key).await.is_ok() {
            sqlx::query("DELETE FROM lane_attachment_garbage WHERE object_key=$1")
                .bind(key)
                .execute(pool)
                .await?;
        } else {
            sqlx::query("UPDATE lane_attachment_garbage SET delete_after=now()+interval '30 seconds' WHERE object_key=$1").bind(key).execute(pool).await?;
            tracing::warn!(
                category = "attachment_delete",
                "provider deletion remains pending"
            );
        }
    }
    sqlx::query(
        "DELETE FROM lane_attachment_uploads WHERE expires_at + interval '1 minute' <= now()",
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image() -> (Value, Vec<u8>) {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[0, 0, 0, 255])
                .unwrap();
        }
        (
            json!({"attachmentId":id(),"width":1,"height":1,"byteSize":bytes.len().to_string(),"sha256":hash(&bytes)}),
            bytes,
        )
    }
    #[test]
    fn pending_manifests_reject_invalid_integer_boundaries() {
        let (valid, bytes) = image();
        validate_png(&valid, &bytes, 1024 * 1024).unwrap();
        assert_eq!(validate_png(&valid, &bytes, 1).unwrap_err().reason, 15);
        for key in ["width", "height"] {
            for bad in [
                json!(0),
                json!(-1),
                json!(2147483648u64),
                json!(4294967295u64),
            ] {
                let mut v = valid.clone();
                v[key] = bad;
                assert!(references(&json!([v])).is_err());
            }
        }
        for bad in [
            json!("0"),
            json!("-1"),
            json!("9223372036854775808"),
            json!(18446744073709551615u64),
        ] {
            let mut v = valid.clone();
            v["byteSize"] = bad;
            assert!(references(&json!([v])).is_err());
        }
        let mut invalid = bytes.clone();
        invalid.push(0);
        let mut manifest = valid.clone();
        manifest["byteSize"] = json!(invalid.len().to_string());
        manifest["sha256"] = json!(hash(&invalid));
        assert!(validate_png(&manifest, &invalid, 1024 * 1024).is_err());
        let mut invalid = bytes;
        invalid[44] ^= 1;
        manifest["byteSize"] = json!(invalid.len().to_string());
        manifest["sha256"] = json!(hash(&invalid));
        assert!(validate_png(&manifest, &invalid, 1024 * 1024).is_err());
    }
    #[test]
    fn signature_matches_independent_botocore_sigv4_vector() {
        // Vector generated by botocore.auth.S3SigV4QueryAuth with the fixed clock,
        // synthetic credentials and canonical Content-Length/Content-Type below.
        let store = ObjectStore::new(
            "https://fixture.r2.cloudflarestorage.com".into(),
            "lane-fixture".into(),
            "fixture-access".into(),
            "fixture-secret".into(),
        )
        .unwrap();
        let time = chrono::DateTime::parse_from_rfc3339("2020-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let (url, headers) = store
            .sign_at(
                "PUT",
                "uploads/test",
                300,
                Some(&json!({"byteSize":"70"})),
                time,
            )
            .unwrap();
        assert_eq!(
            headers,
            json!({"content-length":"70","content-type":"image/png"})
        );
        assert!(url.ends_with(
            "X-Amz-Signature=13b3bb7ddfc59b738b21f4c52b687e1ac909496ea20727ccbe88cf8962da7fbe"
        ));
    }
    #[test]
    fn legacy_fingerprints_ignore_new_empty_repeated_fields() {
        for old in [
            json!({"task":{"title":"old"}}),
            json!({"resolved":{"task":{"title":"old"}}}),
            json!({"version":{"formatVersion":1,"task":{"title":"old"}}}),
        ] {
            let mut current = old.clone();
            if current.get("task").is_some() {
                current["task"]["captures"] = json!([]);
            } else if current.get("resolved").is_some() {
                current["resolved"]["task"]["captures"] = json!([]);
            } else {
                current["version"]["task"]["captures"] = json!([]);
            }
            assert_eq!(
                e::fingerprint(&old).unwrap(),
                e::fingerprint(&current).unwrap()
            );
        }
    }
    #[tokio::test]
    #[ignore = "requires disposable TEST_DATABASE_URL and S3-compatible LANE_R2_* fixture settings"]
    async fn signed_transfer_validates_bytes_retains_history_and_cleans_objects() {
        use sqlx::postgres::PgPoolOptions;
        let storage = ObjectStore::from_env().unwrap();
        let response = storage
            .client
            .put(storage.sign("PUT", "", 60, None).unwrap().0)
            .body("<CreateBucketConfiguration><LocationConstraint>auto</LocationConstraint></CreateBucketConfiguration>")
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success() || response.status() == reqwest::StatusCode::CONFLICT
        );
        let base = sqlx::PgPool::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let schema = format!("captures_{}", id().replace('-', ""));
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&base)
            .await
            .unwrap();
        let path = schema.clone();
        let pool = PgPoolOptions::new()
            .after_connect(move |conn, _| {
                let path = path.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {path}"))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        crate::migration::migrate(&pool).await.unwrap();
        let p = Principal {
            account_id: id(),
            tenant_id: id(),
            session_id: id(),
            device_id: id(),
            device_name: "test".into(),
            origin_kind: "ORIGIN_KIND_CLI".into(),
        };
        let epoch = id();
        sqlx::query("INSERT INTO lane_accounts(account_id,tenant_id,epoch) VALUES($1,$2,$3)")
            .bind(&p.account_id)
            .bind(&p.tenant_id)
            .bind(&epoch)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        crate::account_lock(&mut tx, &p.account_id).await.unwrap();
        let repo = id();
        e::dispatch(
            &mut tx,
            &p,
            "CreateRepository",
            &json!({"id":repo,"meta":{"requestId":id()},"url":"https://example.com/test"}),
            usize::MAX,
        )
        .await
        .unwrap();
        let (capture, bytes) = image();
        let task = id();
        e::dispatch(&mut tx,&p,"CreateTask",&json!({"id":task,"meta":{"requestId":id()},"repository":{"id":repo},"title":"test","captures":[capture]}),usize::MAX).await.unwrap();
        let q = json!({"taskId":task,"attachmentId":capture["attachmentId"],"epoch":epoch});
        let begun = dispatch(&mut tx, &p, "BeginAttachmentUpload", &q, usize::MAX)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let put = |url: String, bytes: Vec<u8>| {
            let client = storage.client.clone();
            let headers = begun["headers"].clone();
            async move {
                let mut req = client.put(url).body(bytes);
                for (k, v) in headers.as_object().unwrap() {
                    req = req.header(k, v.as_str().unwrap());
                }
                req.send().await.unwrap()
            }
        };
        let mut bad_epoch = q.clone();
        bad_epoch["epoch"] = json!(id());
        let mut tx = pool.begin().await.unwrap();
        crate::account_lock(&mut tx, &p.account_id).await.unwrap();
        assert_eq!(
            dispatch(&mut tx, &p, "BeginAttachmentUpload", &bad_epoch, usize::MAX)
                .await
                .unwrap_err()
                .reason,
            17
        );
        tx.rollback().await.unwrap();
        assert!(
            put(string(&begun, "uploadUrl").into(), vec![0; bytes.len()])
                .await
                .status()
                .is_success()
        );
        let mut bad_complete = q.clone();
        bad_complete["uploadId"] = begun["uploadId"].clone();
        let mut tx = pool.begin().await.unwrap();
        crate::account_lock(&mut tx, &p.account_id).await.unwrap();
        assert_eq!(
            dispatch(
                &mut tx,
                &p,
                "CompleteAttachmentUpload",
                &bad_complete,
                usize::MAX
            )
            .await
            .unwrap_err()
            .reason,
            1
        );
        tx.rollback().await.unwrap();
        assert!(
            put(string(&begun, "uploadUrl").into(), bytes.clone())
                .await
                .status()
                .is_success()
        );
        let mut complete = q.clone();
        complete["uploadId"] = begun["uploadId"].clone();
        let mut tx = pool.begin().await.unwrap();
        crate::account_lock(&mut tx, &p.account_id).await.unwrap();
        assert_eq!(
            dispatch(
                &mut tx,
                &p,
                "CompleteAttachmentUpload",
                &complete,
                usize::MAX
            )
            .await
            .unwrap()["state"],
            AVAILABLE
        );
        assert_eq!(
            dispatch(
                &mut tx,
                &p,
                "CompleteAttachmentUpload",
                &complete,
                usize::MAX
            )
            .await
            .unwrap()["state"],
            AVAILABLE
        );
        let download = dispatch(&mut tx, &p, "GetAttachment", &q, 0).await.unwrap();
        tx.commit().await.unwrap();
        let got = storage
            .client
            .get(string(&download, "downloadUrl"))
            .send()
            .await
            .unwrap();
        assert!(got.status().is_success());
        assert_eq!(got.bytes().await.unwrap().as_ref(), bytes);
        // A still-valid temporary PUT cannot overwrite the published immutable object.
        assert!(
            put(string(&begun, "uploadUrl").into(), vec![0; bytes.len()])
                .await
                .status()
                .is_success()
        );
        let got = storage
            .client
            .get(string(&download, "downloadUrl"))
            .send()
            .await
            .unwrap();
        assert_eq!(got.bytes().await.unwrap().as_ref(), bytes);
        let mut tx = pool.begin().await.unwrap();
        crate::account_lock(&mut tx, &p.account_id).await.unwrap();
        e::dispatch(
            &mut tx,
            &p,
            "DeleteTask",
            &json!({"id":task,"meta":{"requestId":id()}}),
            0,
        )
        .await
        .unwrap();
        assert_eq!(
            dispatch(&mut tx, &p, "GetAttachment", &q, 0).await.unwrap()["state"],
            DELETED
        );
        tx.commit().await.unwrap();
        tick(&pool, &storage).await.unwrap();
        assert_eq!(
            storage
                .client
                .get(string(&download, "downloadUrl"))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::NOT_FOUND
        );
        // Simulate expiry after proving a completed staging object was retained until then.
        sqlx::query("UPDATE lane_attachment_garbage SET delete_after=now()")
            .execute(&pool)
            .await
            .unwrap();
        tick(&pool, &storage).await.unwrap();
        let (capture2, bytes2) = image();
        let task2 = id();
        let mut tx = pool.begin().await.unwrap();
        crate::account_lock(&mut tx, &p.account_id).await.unwrap();
        dispatch(&mut tx,&p,"CreateCaptureTask",&json!({"id":task2,"meta":{"requestId":id()},"repositoryId":repo,"title":"inline","images":[{"capture":capture2,"png":base64::engine::general_purpose::STANDARD.encode(&bytes2)}]}),1024*1024).await.unwrap();
        tx.commit().await.unwrap();
        tick(&pool, &storage).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        crate::account_lock(&mut tx, &p.account_id).await.unwrap();
        let ready = dispatch(
            &mut tx,
            &p,
            "GetAttachment",
            &json!({"taskId":task2,"attachmentId":capture2["attachmentId"]}),
            0,
        )
        .await
        .unwrap();
        assert_eq!(ready["state"], AVAILABLE);
        let pending: bool = sqlx::query_scalar(
            "SELECT pending_png IS NULL FROM lane_attachments WHERE account_id=$1 AND id=$2",
        )
        .bind(&p.account_id)
        .bind(string(&capture2, "attachmentId"))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert!(pending);
        crate::clear_account_content(&mut tx, &p.account_id)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        tick(&pool, &storage).await.unwrap();
        assert_eq!(
            storage
                .client
                .get(string(&ready, "downloadUrl"))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::NOT_FOUND
        );
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&base)
            .await
            .unwrap();
        base.close().await;
    }
}
