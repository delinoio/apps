// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Op, Server};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde_json::Value;
use server_common::{Fault, Result, authority::TenantState, now, string, uuid};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
pub struct Identity {
    pub actor: String,
    pub tenant: String,
    pub project: String,
    pub key: Option<uuid::Uuid>,
}
pub fn parse_key(raw: &str) -> Result<(uuid::Uuid, String)> {
    let rest = raw
        .strip_prefix("dbk_")
        .ok_or_else(Fault::unauthenticated)?;
    if rest.len() != 76
        || !rest.is_ascii()
        || rest.as_bytes()[32] != b'_'
        || !rest[..32]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Fault::unauthenticated());
    }
    let id = uuid::Uuid::parse_str(&rest[..32]).map_err(|_| Fault::unauthenticated())?;
    if id.get_version_num() != 7
        || URL_SAFE_NO_PAD
            .decode(&rest[33..])
            .map_or(true, |b| b.len() != 32)
    {
        return Err(Fault::unauthenticated());
    }
    Ok((id, rest[33..].into()))
}
pub async fn usable(
    db: &mut PgConnection,
    id: uuid::Uuid,
    lock: bool,
) -> Result<sqlx::postgres::PgRow> {
    let query = format!(
        "SELECT k.project_id,p.tenant_id,k.permission,k.secret_hash,k.expires_at,k.revoked_at FROM api_keys k JOIN projects p ON p.id=k.project_id WHERE k.id=$1{}",
        if lock { " FOR SHARE OF k" } else { "" }
    );
    let row = sqlx::query(&query)
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(Fault::unauthenticated)?;
    if row.get::<Option<DateTime<Utc>>, _>("revoked_at").is_some()
        || row
            .get::<Option<DateTime<Utc>>, _>("expires_at")
            .is_some_and(|v| v <= now())
    {
        return Err(Fault::unauthenticated());
    }
    Ok(row)
}
impl Server {
    pub async fn authenticate(&self, authorization: &str, op: Op, q: &Value) -> Result<Identity> {
        let parts: Vec<_> = authorization.split_whitespace().collect();
        if parts.len() != 2 || !parts[0].eq_ignore_ascii_case("Bearer") {
            return Err(Fault::unauthenticated());
        }
        let project = string(q, "projectId");
        let requested_tenant = string(q, "tenantId");
        let target = if op.tenant_target() {
            requested_tenant
        } else {
            project
        };
        if !server_common::canonical_id(target) {
            return Err(Fault::invalid("target must be a canonical UUID v7"));
        }
        if parts[1].starts_with("dbk_") {
            let (id, secret) = parse_key(parts[1])?;
            let mut db = self.db.acquire().await?;
            let row = usable(&mut db, id, false).await?;
            let expected: Vec<u8> = row.get("secret_hash");
            let actual = Sha256::digest(secret.as_bytes());
            use subtle::ConstantTimeEq;
            if !bool::from(actual.as_slice().ct_eq(&expected)) {
                return Err(Fault::unauthenticated());
            }
            if op.management() || (op.mutation() && row.get::<i16, _>("permission") != 2) {
                return Err(Fault::denied());
            }
            if row.get::<uuid::Uuid, _>("project_id").to_string() != project {
                return Err(Fault::missing());
            }
            let tenant = row.get::<uuid::Uuid, _>("tenant_id").to_string();
            drop(db);
            if self.authority.states(std::slice::from_ref(&tenant)).await?[0] != TenantState::Active
            {
                return Err(Fault::denied());
            }
            return Ok(Identity {
                actor: format!("key:{id}"),
                tenant,
                project: project.into(),
                key: Some(id),
            });
        }
        let authorization = format!("Bearer {}", parts[1]);
        let mut user = None;
        let tenant = if !requested_tenant.is_empty() {
            requested_tenant.to_owned()
        } else {
            let found: Option<uuid::Uuid> =
                sqlx::query_scalar("SELECT tenant_id FROM projects WHERE id=$1")
                    .bind(uuid(project)?)
                    .fetch_optional(&self.db)
                    .await?;
            match found {
                Some(t) => t.to_string(),
                None => {
                    let identity = self.authority.identify(&authorization).await?;
                    if !op.mutation() {
                        return Err(Fault::missing());
                    }
                    // The routing-only lookup is keyed by the idempotency key, so a malformed key is
                    // rejected as `invalid_argument` before any replay storage access. Authentication
                    // above still wins, and the caller validates the key again for found projects.
                    crate::validation::validate_routing(op, q)?;
                    let t:uuid::Uuid=sqlx::query_scalar("SELECT tenant_id FROM replays WHERE principal=$1 AND idempotency_key=$2 AND expires_at>$3").bind(format!("user:{identity}")).bind(string(q,"idempotencyKey")).bind(now()).fetch_optional(&self.db).await?.ok_or_else(Fault::missing)?;
                    user = Some(identity);
                    t.to_string()
                }
            }
        };
        let tenants = [tenant.clone()];
        let (identified, manager, states) = tokio::join!(
            async {
                match user {
                    Some(u) => Ok(u),
                    None => self.authority.identify(&authorization).await,
                }
            },
            self.authority.manager(&authorization, &tenant),
            self.authority.states(&tenants)
        );
        for result in [
            identified.as_ref().map(|_| ()),
            manager.as_ref().map(|_| ()),
        ] {
            if result.is_err_and(|e| e.code == connectrpc::ErrorCode::Unauthenticated) {
                return Err(Fault::unauthenticated());
            }
        }
        manager?;
        let user = identified?;
        let state = states?[0];
        if state != TenantState::Active
            && !(op.degraded() && matches!(state, TenantState::Blocked | TenantState::Deleting))
        {
            return Err(Fault::denied());
        }
        Ok(Identity {
            actor: format!("user:{user}"),
            tenant,
            project: project.into(),
            key: None,
        })
    }
}
