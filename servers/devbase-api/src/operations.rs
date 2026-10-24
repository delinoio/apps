// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Op, Server, auth::Identity, validation};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde_json::{Value, json};
use server_common::{Fault, Result, integer, now, string, uuid};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row, postgres::PgRow};
pub struct Outcome {
    pub body: Value,
    pub project: Option<uuid::Uuid>,
    pub user: Option<uuid::Uuid>,
}
fn project(r: &PgRow) -> Value {
    json!({"id":r.get::<uuid::Uuid,_>("id"),"tenantId":r.get::<uuid::Uuid,_>("tenant_id"),"name":r.get::<String,_>("name"),"version":r.get::<i64,_>("version").to_string(),"createTime":r.get::<DateTime<Utc>,_>("created_at"),"updateTime":r.get::<DateTime<Utc>,_>("updated_at")})
}
fn user(r: &PgRow) -> Value {
    json!({"id":r.get::<uuid::Uuid,_>("id"),"projectId":r.get::<uuid::Uuid,_>("project_id"),"externalId":r.get::<String,_>("external_id"),"name":r.get::<Option<String>,_>("name").unwrap_or_default(),"email":r.get::<Option<String>,_>("email").unwrap_or_default(),"metadata":r.get::<Value,_>("metadata"),"version":r.get::<i64,_>("version").to_string(),"createTime":r.get::<DateTime<Utc>,_>("created_at"),"updateTime":r.get::<DateTime<Utc>,_>("updated_at")})
}
fn key(r: &PgRow) -> Value {
    json!({"id":r.get::<uuid::Uuid,_>("id"),"projectId":r.get::<uuid::Uuid,_>("project_id"),"permission":r.get::<i16,_>("permission"),"createTime":r.get::<DateTime<Utc>,_>("created_at"),"expireTime":r.get::<Option<DateTime<Utc>>,_>("expires_at"),"revokeTime":r.get::<Option<DateTime<Utc>>,_>("revoked_at")})
}
pub async fn scrub(db: &mut PgConnection, column: &str, id: uuid::Uuid) -> Result<()> {
    // Only internal column constants reach this query. Delete replies retain their fingerprint.
    // A scrubbed record keeps no end-user ID, so target_user_id is cleared with the payload.
    assert!(matches!(column, "project_id" | "target_user_id"));
    sqlx::query(&format!("UPDATE replays SET scrubbed=true,response=NULL,fingerprint='\\x'::bytea,target_user_id=NULL WHERE {column}=$1 AND NOT scrubbed AND operation NOT LIKE 'Delete%'")) .bind(id).execute(db).await?;
    Ok(())
}
pub async fn audit(
    db: &mut PgConnection,
    actor: &str,
    action: &str,
    target: uuid::Uuid,
    trace: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO audit_events(id,occurred_at,action,actor,target_id,trace_id) VALUES($1,$2,$3,$4,$5,$6)").bind(uuid::Uuid::now_v7()).bind(now()).bind(action).bind(actor).bind(target).bind(trace).execute(db).await?;
    Ok(())
}
async fn require_project(db: &mut PgConnection, id: uuid::Uuid) -> Result<()> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(id)
        .fetch_one(db)
        .await?;
    if !exists {
        return Err(Fault::missing());
    }
    Ok(())
}
async fn stale_project(db: &mut PgConnection, id: uuid::Uuid) -> Result<Outcome> {
    require_project(db, id).await?;
    Err(Fault::stale())
}
impl Server {
    pub async fn operation(
        &self,
        db: &mut PgConnection,
        op: Op,
        q: &Value,
        who: &Identity,
        trace: &str,
    ) -> Result<Outcome> {
        let mut out = Outcome {
            body: json!({}),
            project: None,
            user: None,
        };
        let pid = if who.project.is_empty() {
            uuid::Uuid::nil()
        } else {
            uuid(&who.project)?
        };
        let tenant = uuid(&who.tenant)?;
        let time = now();
        let mut event = None;
        match op {
            Op::CreateProject => {
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                    .bind(format!("projects:{}", who.tenant))
                    .execute(&mut *db)
                    .await?;
                let cleaned: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM deleted_tenants WHERE tenant_id=$1)",
                )
                .bind(tenant)
                .fetch_one(&mut *db)
                .await?;
                if cleaned {
                    return Err(Fault::denied());
                }
                let count: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM projects WHERE tenant_id=$1")
                        .bind(tenant)
                        .fetch_one(&mut *db)
                        .await?;
                if count >= self.config.projects {
                    return Err(Fault::quota());
                }
                let id = uuid::Uuid::now_v7();
                let row=sqlx::query("INSERT INTO projects(id,tenant_id,name,created_at,updated_at) VALUES($1,$2,$3,$4,$4) RETURNING *").bind(id).bind(tenant).bind(string(q,"name")).bind(time).fetch_one(&mut *db).await?;
                out.body["project"] = project(&row);
                out.project = Some(id);
                event = Some(("project.create", id));
            }
            Op::GetProject => {
                let row = sqlx::query("SELECT * FROM projects WHERE id=$1")
                    .bind(pid)
                    .fetch_one(&mut *db)
                    .await?;
                out.body["project"] = project(&row);
            }
            Op::UpdateProject => {
                let row=sqlx::query("UPDATE projects SET name=$2,version=version+1,updated_at=$3 WHERE id=$1 AND version=$4 RETURNING *").bind(pid).bind(string(q,"name")).bind(time).bind(integer(q,"version")).fetch_optional(&mut *db).await?;
                let Some(row) = row else {
                    return stale_project(db, pid).await;
                };
                out.body["project"] = project(&row);
                event = Some(("project.update", pid));
            }
            Op::DeleteProject => {
                let n = sqlx::query("DELETE FROM projects WHERE id=$1 AND version=$2")
                    .bind(pid)
                    .bind(integer(q, "version"))
                    .execute(&mut *db)
                    .await?
                    .rows_affected();
                if n == 0 {
                    return stale_project(db, pid).await;
                }
                scrub(db, "project_id", pid).await?;
                event = Some(("project.delete", pid));
            }
            Op::ListProjects | Op::ListApiKeys | Op::ListEndUsers => {
                let (size, at, id) = validation::page(q)?;
                let (table, parent, field, convert): (&str, &str, &str, fn(&PgRow) -> Value) =
                    match op {
                        Op::ListProjects => ("projects", "tenant_id", "projects", project),
                        Op::ListApiKeys => ("api_keys", "project_id", "apiKeys", key),
                        _ => ("end_users", "project_id", "endUsers", user),
                    };
                let rows=sqlx::query(&format!("SELECT * FROM {table} WHERE {parent}=$1 AND ($2::timestamptz IS NULL OR (created_at,id)>($2,$3)) ORDER BY created_at,id LIMIT $4")).bind(if op==Op::ListProjects{tenant}else{pid}).bind(at).bind(id).bind(size+1).fetch_all(&mut *db).await?;
                let items: Vec<_> = rows.iter().take(size as usize).map(convert).collect();
                if rows.len() > size as usize {
                    let row = &rows[size as usize - 1];
                    out.body["nextPageToken"] =
                        validation::cursor(row.get("created_at"), row.get("id")).into();
                }
                if items.is_empty() && op != Op::ListProjects {
                    require_project(db, pid).await?
                }
                out.body[field] = items.into();
            }
            Op::IssueApiKey => {
                let expiry = if string(q, "expireTime").is_empty() {
                    None
                } else {
                    Some(
                        DateTime::parse_from_rfc3339(string(q, "expireTime"))
                            .map_err(|_| Fault::invalid("invalid expiration"))?
                            .with_timezone(&Utc),
                    )
                };
                if expiry.is_some_and(|e| e <= time) {
                    return Err(Fault::invalid("expire_time must be in the future"));
                }
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                    .bind(format!("keys:{pid}"))
                    .execute(&mut *db)
                    .await?;
                let count:i64=sqlx::query_scalar("SELECT count(*) FROM api_keys WHERE project_id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>$2)").bind(pid).bind(time).fetch_one(&mut *db).await?;
                if count >= self.config.keys {
                    return Err(Fault::quota());
                }
                let id = uuid::Uuid::now_v7();
                let mut bytes = [0; 32];
                rand::rng().fill_bytes(&mut bytes);
                let secret = URL_SAFE_NO_PAD.encode(bytes);
                let digest = Sha256::digest(secret.as_bytes()).to_vec();
                let permission: i16 = if string(q, "permission") == "API_KEY_PERMISSION_CRUD" {
                    2
                } else {
                    1
                };
                let row=sqlx::query("INSERT INTO api_keys(id,project_id,permission,secret_hash,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6) RETURNING *").bind(id).bind(pid).bind(permission).bind(digest).bind(time).bind(expiry).fetch_one(&mut *db).await?;
                out.body["apiKey"] = key(&row);
                out.body["secret"] = format!("dbk_{}_{secret}", id.simple()).into();
                event = Some(("api_key.issue", id));
            }
            Op::GetApiKey | Op::RevokeApiKey => {
                let id = uuid(string(q, "apiKeyId"))?;
                let mut changed = None;
                if op == Op::RevokeApiKey {
                    changed=sqlx::query("UPDATE api_keys SET revoked_at=$3 WHERE id=$1 AND project_id=$2 AND revoked_at IS NULL RETURNING *").bind(id).bind(pid).bind(time).fetch_optional(&mut *db).await?;
                    if changed.is_some() {
                        event = Some(("api_key.revoke", id));
                    }
                }
                let row = match changed {
                    Some(r) => r,
                    None => {
                        sqlx::query("SELECT * FROM api_keys WHERE id=$1 AND project_id=$2")
                            .bind(id)
                            .bind(pid)
                            .fetch_one(&mut *db)
                            .await?
                    }
                };
                out.body["apiKey"] = key(&row);
            }
            Op::CreateEndUser => {
                let n = sqlx::query(
                    "UPDATE projects SET user_count=user_count+1 WHERE id=$1 AND user_count<$2",
                )
                .bind(pid)
                .bind(self.config.users)
                .execute(&mut *db)
                .await?
                .rows_affected();
                if n == 0 {
                    require_project(db, pid).await?;
                    return Err(Fault::quota());
                }
                let id = uuid::Uuid::now_v7();
                let row=sqlx::query("INSERT INTO end_users(id,project_id,external_id,name,email,metadata,created_at,updated_at) VALUES($1,$2,$3,NULLIF($4,''),NULLIF($5,''),$6,$7,$7) RETURNING *").bind(id).bind(pid).bind(string(q,"externalId")).bind(string(q,"name")).bind(string(q,"email")).bind(validation::metadata(q)?).bind(time).fetch_one(&mut *db).await?;
                out.body["endUser"] = user(&row);
                out.user = Some(id);
                event = Some(("end_user.create", id));
            }
            Op::GetEndUser | Op::LookupEndUser => {
                let row = if op == Op::GetEndUser {
                    sqlx::query("SELECT * FROM end_users WHERE project_id=$1 AND id=$2")
                        .bind(pid)
                        .bind(uuid(string(q, "endUserId"))?)
                        .fetch_one(&mut *db)
                        .await?
                } else {
                    sqlx::query("SELECT * FROM end_users WHERE project_id=$1 AND external_id=$2")
                        .bind(pid)
                        .bind(string(q, "externalId"))
                        .fetch_one(&mut *db)
                        .await?
                };
                out.body["endUser"] = user(&row);
            }
            Op::UpdateEndUser => {
                let id = uuid(string(q, "endUserId"))?;
                let row =
                    sqlx::query("SELECT * FROM end_users WHERE id=$1 AND project_id=$2 FOR UPDATE")
                        .bind(id)
                        .bind(pid)
                        .fetch_one(&mut *db)
                        .await?;
                if row.get::<i64, _>("version") != integer(q, "version") {
                    return Err(Fault::stale());
                }
                let mut value = user(&row);
                for field in validation::mask(q) {
                    value[field] = if field == "metadata" {
                        validation::metadata(q)?
                    } else {
                        string(q, field).into()
                    };
                }
                let row=sqlx::query("UPDATE end_users SET external_id=$3,name=NULLIF($4,''),email=NULLIF($5,''),metadata=$6,version=version+1,updated_at=$7 WHERE id=$1 AND project_id=$2 RETURNING *").bind(id).bind(pid).bind(string(&value,"externalId")).bind(string(&value,"name")).bind(string(&value,"email")).bind(&value["metadata"]).bind(time).fetch_one(&mut *db).await?;
                out.body["endUser"] = user(&row);
                out.user = Some(id);
                event = Some(("end_user.update", id));
            }
            Op::DeleteEndUser => {
                let id = uuid(string(q, "endUserId"))?;
                sqlx::query("SELECT id FROM projects WHERE id=$1 FOR NO KEY UPDATE")
                    .bind(pid)
                    .fetch_one(&mut *db)
                    .await?;
                let n = sqlx::query(
                    "DELETE FROM end_users WHERE id=$1 AND project_id=$2 AND version=$3",
                )
                .bind(id)
                .bind(pid)
                .bind(integer(q, "version"))
                .execute(&mut *db)
                .await?
                .rows_affected();
                if n == 0 {
                    let found: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM end_users WHERE id=$1 AND project_id=$2)",
                    )
                    .bind(id)
                    .bind(pid)
                    .fetch_one(&mut *db)
                    .await?;
                    return Err(if found {
                        Fault::stale()
                    } else {
                        Fault::missing()
                    });
                }
                sqlx::query("UPDATE projects SET user_count=user_count-1 WHERE id=$1")
                    .bind(pid)
                    .execute(&mut *db)
                    .await?;
                scrub(db, "target_user_id", id).await?;
                event = Some(("end_user.delete", id));
            }
        }
        if let Some((action, target)) = event {
            audit(db, &who.actor, action, target, trace).await?
        }
        Ok(out)
    }
}
