// SPDX-License-Identifier: FSL-1.1-ALv2
//! Service-neutral API key lifecycle and authorization. Keys are managed by
//! Logto user tokens only. Registered services verify them with
//! `AuthorizeApiKey`, which evaluates only the grants of the calling service.
use super::{Call, canonical, forbidden, invalid, page, precondition, string, uuid};
use crate::{
    Fault, Op, Result,
    api_keys::{self, GrantRequest, OwnerKind},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Row, postgres::PgRow};

const KEY_COLUMNS: &str = "k.id::text AS id,k.owner_kind::int4 AS owner_kind,k.tenant_id::text AS tenant_id,k.user_id::text AS user_id,k.name,k.created_at,k.expires_at,k.revoked_at,COALESCE((SELECT jsonb_agg(jsonb_build_object('serviceId',g.service_id::text,'capabilities',to_jsonb(g.capabilities)) ORDER BY g.service_id) FROM api_key_grants g WHERE g.key_id=k.id),'[]'::jsonb) AS grants";

fn key_json(row: &PgRow) -> Result<Value> {
    let mut key = json!({
        "id": row.try_get::<String, _>("id")?,
        "ownerKind": row.try_get::<i32, _>("owner_kind")?,
        "tenantId": row.try_get::<String, _>("tenant_id")?,
        "userId": row.try_get::<String, _>("user_id")?,
        "name": row.try_get::<String, _>("name")?,
        "grants": row.try_get::<Value, _>("grants")?,
        "createdAt": row.try_get::<DateTime<Utc>, _>("created_at")?.to_rfc3339(),
    });
    for (column, field) in [("expires_at", "expiresAt"), ("revoked_at", "revokedAt")] {
        if let Some(at) = row.try_get::<Option<DateTime<Utc>>, _>(column)? {
            key[field] = json!(at.to_rfc3339());
        }
    }
    Ok(key)
}

/// Parses the optional expiry. It must lie in the future when supplied.
fn expiry(value: &Value, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
    if value.is_null() {
        return Ok(None);
    }
    let at = value
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .ok_or_else(|| invalid("expiration must be an RFC 3339 timestamp"))?
        .with_timezone(&Utc);
    if at <= now {
        return Err(invalid("expiration must be in the future"));
    }
    Ok(Some(at))
}

impl Call<'_> {
    pub async fn api_keys(&mut self, op: Op, q: &Value) -> Result<Value> {
        match op {
            Op::CreateApiKey => self.create_api_key(q).await,
            Op::ListApiKeys => self.list_api_keys(q).await,
            Op::RevokeApiKey => self.revoke_api_key(q).await,
            Op::AuthorizeApiKey => self.authorize_api_key(q).await,
            _ => Err(invalid("unknown API key operation")),
        }
    }

    /// Key management belongs to signed-in users. Service identities and API
    /// keys never manage keys.
    fn key_manager(&self, tenant: &str) -> Result<()> {
        if self.user.is_empty() || !self.service.is_empty() {
            return Err(forbidden());
        }
        if !canonical(tenant) {
            return Err(invalid("invalid tenant ID"));
        }
        Ok(())
    }

    /// Current-member requirement for personal keys, Owner/Admin for tenant keys.
    async fn key_scope(&mut self, tenant: &str, kind: OwnerKind) -> Result<()> {
        match kind {
            OwnerKind::Personal => self.member(tenant).await,
            OwnerKind::Tenant => self.require(tenant, &[2]).await,
        }
    }

    async fn create_api_key(&mut self, q: &Value) -> Result<Value> {
        let tenant = string(q, "tenantId");
        self.key_manager(tenant)?;
        let kind = OwnerKind::from_request(&q["ownerKind"])?;
        let name = string(q, "name");
        api_keys::validate_name(name)?;
        let grants = api_keys::parse_grants(&q["grants"])?;
        let expires_at = expiry(&q["expiresAt"], self.now)?;
        self.key_scope(tenant, kind).await?;
        let deleting: bool =
            sqlx::query_scalar("SELECT deleting_at IS NOT NULL FROM tenants WHERE id=$1")
                .bind(uuid(tenant)?)
                .fetch_one(&mut *self.tx)
                .await?;
        if deleting {
            return Err(precondition("tenant is being deleted"));
        }
        self.check_grants(&grants).await?;
        let active: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys WHERE tenant_id=$1 AND owner_kind=$2::int2 AND ($2::int2=2 OR user_id=$3) AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>$4)")
            .bind(uuid(tenant)?).bind(kind as i16).bind(uuid(&self.user)?).bind(self.now)
            .fetch_one(&mut *self.tx).await?;
        if active >= api_keys::MAX_ACTIVE_KEYS {
            return Err(precondition("too many active API keys; revoke one first"));
        }
        let key_id = uuid::Uuid::now_v7();
        let generated = api_keys::generate(key_id);
        sqlx::query("INSERT INTO api_keys(id,tenant_id,owner_kind,user_id,name,secret_hash,created_at,expires_at) VALUES($1,$2,$3::int2,$4,$5,$6,$7,$8)")
            .bind(key_id).bind(uuid(tenant)?).bind(kind as i16).bind(uuid(&self.user)?)
            .bind(name).bind(generated.hash.as_slice()).bind(self.now).bind(expires_at)
            .execute(&mut *self.tx).await?;
        for grant in &grants {
            sqlx::query(
                "INSERT INTO api_key_grants(key_id,service_id,capabilities) VALUES($1,$2,$3)",
            )
            .bind(key_id)
            .bind(grant.service_id)
            .bind(&grant.capabilities)
            .execute(&mut *self.tx)
            .await?;
        }
        self.audit(tenant, "api_key.create", &format!("key_id={key_id}"))
            .await?;
        let row = sqlx::query(&format!(
            "SELECT {KEY_COLUMNS} FROM api_keys k WHERE k.id=$1"
        ))
        .bind(key_id)
        .fetch_one(&mut *self.tx)
        .await?;
        tracing::info!(category="api_key",correlation_id=%self.correlation,key_id=%key_id,owner_kind=?kind,grants=grants.len(),expires=expires_at.is_some(),"API key created");
        // The replay record is stored without the secret by the caller.
        Ok(json!({"key":key_json(&row)?,"secret":generated.secret}))
    }

    /// Every grant must name a registered service and only capabilities that
    /// service declares in the catalog.
    async fn check_grants(&mut self, grants: &[GrantRequest]) -> Result<()> {
        for grant in grants {
            let declared: Option<Vec<String>> =
                sqlx::query_scalar("SELECT capabilities FROM services WHERE id=$1")
                    .bind(grant.service_id)
                    .fetch_optional(&mut *self.tx)
                    .await?;
            let declared = declared.ok_or_else(|| invalid("grant names an unknown service"))?;
            if grant.capabilities.iter().any(|c| !declared.contains(c)) {
                return Err(invalid(
                    "grant names a capability the service does not declare",
                ));
            }
        }
        Ok(())
    }

    async fn list_api_keys(&mut self, q: &Value) -> Result<Value> {
        let tenant = string(q, "tenantId");
        self.key_manager(tenant)?;
        let kind = OwnerKind::from_request(&q["ownerKind"])?;
        let (n, after) = page(&q["page"])?;
        self.key_scope(tenant, kind).await?;
        let rows = sqlx::query(&format!("SELECT {KEY_COLUMNS} FROM api_keys k WHERE k.tenant_id=$1 AND k.owner_kind=$2::int2 AND ($2::int2=2 OR k.user_id=$3) AND k.id>$4 ORDER BY k.id LIMIT $5"))
            .bind(uuid(tenant)?).bind(kind as i16).bind(uuid(&self.user)?).bind(uuid(&after)?).bind(n + 1)
            .fetch_all(&mut *self.tx).await?;
        let mut keys = rows.iter().map(key_json).collect::<Result<Vec<_>>>()?;
        let next = if keys.len() > n as usize {
            keys.truncate(n as usize);
            string(&keys[n as usize - 1], "id").to_owned()
        } else {
            String::new()
        };
        Ok(json!({"keys":keys,"nextAfterId":next}))
    }

    /// Owner kind and owner of a key within a tenant, or `None` when absent.
    async fn key_owner(&mut self, tenant: &str, key: &str) -> Result<Option<(OwnerKind, String)>> {
        let row: Option<(i32, String)> = sqlx::query_as(
            "SELECT owner_kind::int4,user_id::text FROM api_keys WHERE id=$1 AND tenant_id=$2",
        )
        .bind(uuid(key)?)
        .bind(uuid(tenant)?)
        .fetch_optional(&mut *self.tx)
        .await?;
        row.map(|(kind, owner)| Ok((OwnerKind::from_db(kind)?, owner)))
            .transpose()
    }

    async fn revoke_api_key(&mut self, q: &Value) -> Result<Value> {
        let tenant = string(q, "tenantId");
        let key = string(q, "keyId");
        self.key_manager(tenant)?;
        if !canonical(key) {
            return Err(invalid("invalid key ID"));
        }
        // Membership first, so non-members cannot probe for key IDs.
        self.member(tenant).await?;
        let Some((kind, owner)) = self.key_owner(tenant, key).await? else {
            return Err(Fault::missing());
        };
        match kind {
            // Personal keys are invisible to everyone but their owner.
            OwnerKind::Personal if owner != self.user => return Err(Fault::missing()),
            OwnerKind::Personal => {}
            OwnerKind::Tenant => self.require(tenant, &[2]).await?,
        }
        let revoked =
            sqlx::query("UPDATE api_keys SET revoked_at=$2 WHERE id=$1 AND revoked_at IS NULL")
                .bind(uuid(key)?)
                .bind(self.now)
                .execute(&mut *self.tx)
                .await?
                .rows_affected();
        // Revoking twice returns the original revocation time without a new audit row.
        if revoked > 0 {
            self.audit(tenant, "api_key.revoke", &format!("key_id={key}"))
                .await?;
            tracing::info!(category="api_key",correlation_id=%self.correlation,key_id=%key,owner_kind=?kind,"API key revoked");
        }
        let row = sqlx::query(&format!(
            "SELECT {KEY_COLUMNS} FROM api_keys k WHERE k.id=$1"
        ))
        .bind(uuid(key)?)
        .fetch_one(&mut *self.tx)
        .await?;
        Ok(json!({"key":key_json(&row)?}))
    }

    /// Re-checks current authority before a stored key mutation is replayed.
    pub async fn authorize_api_key_replay(&mut self, op: Op, q: &Value) -> Result<()> {
        let tenant = string(q, "tenantId");
        self.key_manager(tenant)?;
        match op {
            Op::CreateApiKey => {
                let kind = OwnerKind::from_request(&q["ownerKind"])?;
                self.key_scope(tenant, kind).await
            }
            _ => {
                self.member(tenant).await?;
                let key = string(q, "keyId");
                if !canonical(key) {
                    return Err(invalid("invalid key ID"));
                }
                match self.key_owner(tenant, key).await? {
                    Some((OwnerKind::Tenant, _)) => self.require(tenant, &[2]).await,
                    Some((OwnerKind::Personal, owner)) if owner != self.user => {
                        Err(Fault::missing())
                    }
                    _ => Ok(()),
                }
            }
        }
    }

    async fn authorize_api_key(&mut self, q: &Value) -> Result<Value> {
        self.service_scope("authorize").await?;
        let capability = string(q, "capability");
        if capability.is_empty() || capability.len() > api_keys::MAX_CAPABILITY_BYTES {
            return Err(invalid("capability must contain 1 to 100 bytes"));
        }
        let parsed = api_keys::parse(string(q, "secret"));
        let lookup = parsed
            .as_ref()
            .map(|p| p.key_id)
            .unwrap_or_else(uuid::Uuid::nil);
        let row = sqlx::query("SELECT k.owner_kind::int4 AS owner_kind,k.tenant_id::text AS tenant_id,k.user_id::text AS user_id,k.secret_hash,(k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at>$2) AND t.deleted_at IS NULL) AS active FROM api_keys k JOIN tenants t ON t.id=k.tenant_id WHERE k.id=$1")
            .bind(lookup).bind(self.now).fetch_optional(&mut *self.tx).await?;
        // Always hash-compare, even for unknown or malformed secrets, so the
        // outcome classes cost the same.
        let stored: Vec<u8> = match &row {
            Some(row) => row.try_get("secret_hash")?,
            None => vec![0; 32],
        };
        let presented = parsed.as_ref().map_or([0u8; 32], |p| p.hash);
        let matches = api_keys::constant_time_eq(&stored, &presented);
        let (Some(row), true, true) = (row, matches, parsed.is_some()) else {
            // Malformed, unknown and wrong secrets are indistinguishable to callers
            // and never reveal attribution.
            tracing::warn!(category="api_key_authorization",correlation_id=%self.correlation,service_id=%self.service,outcome="unauthenticated",cause="invalid_secret","API key rejected");
            return Err(Fault::unauthenticated());
        };
        let key_id = lookup.to_string();
        if !row.try_get::<bool, _>("active")? {
            tracing::warn!(category="api_key_authorization",correlation_id=%self.correlation,service_id=%self.service,key_id=%key_id,outcome="unauthenticated",cause="inactive_key","API key rejected");
            return Err(Fault::unauthenticated());
        }
        let kind = OwnerKind::from_db(row.try_get("owner_kind")?)?;
        let tenant: String = row.try_get("tenant_id")?;
        let owner: String = row.try_get("user_id")?;
        // Only grants of the calling service are visible: a key never authorizes
        // another service's capabilities.
        let granted: Option<bool> = sqlx::query_scalar(
            "SELECT $3=ANY(capabilities) FROM api_key_grants WHERE key_id=$1 AND service_id=$2",
        )
        .bind(lookup)
        .bind(uuid(&self.service)?)
        .bind(capability)
        .fetch_optional(&mut *self.tx)
        .await?;
        let mut out = if granted == Some(true) {
            self.authorize_tenant(
                &tenant,
                // Tenant keys act independently of the issuing user.
                (kind == OwnerKind::Personal).then_some(owner.as_str()),
                capability,
                q["paid"].as_bool().unwrap_or(false),
                true,
                q,
            )
            .await?
        } else {
            json!({"allowed":false,"reasons":[1]})
        };
        out["keyId"] = json!(key_id);
        out["tenantId"] = json!(tenant);
        out["ownerKind"] = json!(kind as i32);
        out["userId"] = json!(owner);
        tracing::info!(category="api_key_authorization",correlation_id=%self.correlation,service_id=%self.service,key_id=%key_id,tenant_id=%tenant,owner_kind=?kind,capability=%capability,allowed=out["allowed"].as_bool().unwrap_or(false),granted=granted==Some(true),"API key authorization evaluated");
        Ok(out)
    }
}
