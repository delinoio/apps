// SPDX-License-Identifier: FSL-1.1-ALv2
//! API key persistence. Only the verification hash is stored.
use crate::{
    error::{Fault, Result},
    keys::{Capabilities, kind_from_db, kind_to_db},
};
use buffa_types::google::protobuf::Timestamp;
use chrono::{DateTime, Utc};
use delino_protocol::delitools::v1::{ApiKey, KeyKind};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct KeyRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub kind: KeyKind,
    pub capabilities: Capabilities,
    pub secret_hash: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

fn from_row(row: &PgRow) -> Result<KeyRow> {
    let kind: i16 = row.try_get("kind")?;
    let capabilities: Vec<i16> = row.try_get("capabilities")?;
    Ok(KeyRow {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        user_id: row.try_get("user_id")?,
        kind: kind_from_db(kind).ok_or_else(Fault::internal)?,
        capabilities: Capabilities::from_database(&capabilities),
        secret_hash: row.try_get("secret_hash")?,
        created_at: row.try_get("created_at")?,
        expires_at: row.try_get("expires_at")?,
        revoked_at: row.try_get("revoked_at")?,
    })
}

fn timestamp(value: DateTime<Utc>) -> Timestamp {
    Timestamp {
        seconds: value.timestamp(),
        nanos: value.timestamp_subsec_nanos() as i32,
        ..Default::default()
    }
}

impl KeyRow {
    pub fn to_proto(&self) -> ApiKey {
        ApiKey {
            id: self.id.to_string(),
            tenant_id: self.tenant_id.to_string(),
            user_id: self.user_id.to_string(),
            kind: self.kind.into(),
            capabilities: self
                .capabilities
                .to_list()
                .into_iter()
                .map(Into::into)
                .collect(),
            created_at: timestamp(self.created_at).into(),
            expires_at: self.expires_at.map(timestamp).into(),
            revoked: self.revoked_at.is_some(),
            ..Default::default()
        }
    }
}

pub struct NewKey<'a> {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub kind: KeyKind,
    pub capabilities: Capabilities,
    pub secret_hash: &'a [u8],
    pub expires_at: Option<DateTime<Utc>>,
}

pub async fn insert(pool: &PgPool, key: NewKey<'_>, now: DateTime<Utc>) -> Result<KeyRow> {
    let row = sqlx::query("INSERT INTO api_keys(id,tenant_id,user_id,kind,capabilities,secret_hash,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING *")
        .bind(key.id)
        .bind(key.tenant_id)
        .bind(key.user_id)
        .bind(kind_to_db(key.kind))
        .bind(key.capabilities.to_database())
        .bind(key.secret_hash)
        .bind(now)
        .bind(key.expires_at)
        .fetch_one(pool)
        .await?;
    from_row(&row)
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<KeyRow>> {
    sqlx::query("SELECT * FROM api_keys WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .as_ref()
        .map(from_row)
        .transpose()
}

pub async fn get_in_tenant(pool: &PgPool, tenant_id: Uuid, id: Uuid) -> Result<Option<KeyRow>> {
    sqlx::query("SELECT * FROM api_keys WHERE id=$1 AND tenant_id=$2")
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await?
        .as_ref()
        .map(from_row)
        .transpose()
}

/// Which keys a listing selects. Both filters always include the tenant.
pub enum Listing {
    /// Personal keys owned by the user.
    PersonalOf(Uuid),
    /// Tenant service keys.
    Service,
    /// Own personal keys plus tenant service keys.
    Both(Uuid),
}

pub async fn list(
    pool: &PgPool,
    tenant_id: Uuid,
    listing: Listing,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<KeyRow>> {
    let (personal_user, include_service, include_personal) = match listing {
        Listing::PersonalOf(user) => (Some(user), false, true),
        Listing::Service => (None, true, false),
        Listing::Both(user) => (Some(user), true, true),
    };
    let rows = sqlx::query("SELECT * FROM api_keys WHERE tenant_id=$1 AND id>$2 AND ((kind=1 AND $3 AND user_id=$4) OR (kind=2 AND $5)) ORDER BY id LIMIT $6")
        .bind(tenant_id)
        .bind(after.unwrap_or(Uuid::nil()))
        .bind(include_personal)
        .bind(personal_user.unwrap_or(Uuid::nil()))
        .bind(include_service)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    rows.iter().map(from_row).collect()
}

/// Revoke a key once. The first revocation time is kept; revoking again changes nothing.
pub async fn revoke(pool: &PgPool, id: Uuid, now: DateTime<Utc>) -> Result<()> {
    sqlx::query("UPDATE api_keys SET revoked_at=COALESCE(revoked_at,$2) WHERE id=$1")
        .bind(id)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}
