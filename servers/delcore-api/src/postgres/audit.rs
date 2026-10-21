// SPDX-License-Identifier: FSL-1.1-ALv2
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

pub const ACTOR_CATALOG: &str = "system:catalog";
pub const ACTOR_METERING: &str = "system:metering";

/// Append an audit record. Reasons must contain only safe, bounded diagnostics.
#[allow(clippy::too_many_arguments)]
async fn audit_inner(
    db: &mut PgConnection,
    actor: &str,
    tenant: &str,
    action: &str,
    reason: &str,
    correlation: &str,
    at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let tenant = if tenant.is_empty() {
        None
    } else {
        Some(Uuid::parse_str(tenant)?)
    };
    sqlx::query("INSERT INTO audit(id,actor_id,tenant_id,action,reason,correlation_id,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(Uuid::now_v7()).bind(actor).bind(tenant).bind(action).bind(reason).bind(Uuid::parse_str(correlation)?).bind(at).execute(db).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn audit(
    db: &mut PgConnection,
    actor: &str,
    tenant: &str,
    action: &str,
    reason: &str,
    correlation: &str,
    at: DateTime<Utc>,
) -> crate::Result<()> {
    audit_inner(db, actor, tenant, action, reason, correlation, at)
        .await
        .map_err(Into::into)
}
