// SPDX-License-Identifier: FSL-1.1-ALv2
//! Audit events, which also back the user-visible activity log. Events never contain secrets
//! or usage values and expire 30 days after creation.
use crate::{Result, call::Call};
use buffa::Enumeration;
use delino_protocol::anydash::v1::ActivityKind;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub struct Event {
    pub kind: ActivityKind,
    pub actor_user: Option<Uuid>,
    pub actor_device: Option<Uuid>,
    /// The user whose own device, connection or account activity this is.
    pub subject_user: Option<Uuid>,
    /// The affected space for dashboard and sharing events.
    pub tenant: Option<Uuid>,
    pub device: Option<Uuid>,
    pub connection: Option<Uuid>,
    pub dashboard: Option<Uuid>,
    pub widget: Option<Uuid>,
}

pub async fn record(db: &mut PgConnection, call: &Call, e: Event) -> Result<()> {
    insert(
        db,
        &call.correlation,
        call.client_kind.to_i32(),
        call.now,
        e,
    )
    .await
}

pub async fn record_pool(db: &sqlx::PgPool, call: &Call, e: Event) -> Result<()> {
    let mut conn = db.acquire().await?;
    record(&mut conn, call, e).await
}

/// Records a worker event that has no request; `correlation` identifies the worker run.
pub async fn insert(
    db: &mut PgConnection,
    correlation: &str,
    client_kind: i32,
    at: chrono::DateTime<chrono::Utc>,
    e: Event,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_events(id,occurred_at,kind,actor_user_id,actor_device_id,subject_user_id,tenant_id,device_id,connection_id,dashboard_id,widget_id,correlation_id,client_kind) \
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
    )
    .bind(crate::new_id())
    .bind(at)
    .bind(e.kind.to_i32() as i16)
    .bind(e.actor_user)
    .bind(e.actor_device)
    .bind(e.subject_user)
    .bind(e.tenant)
    .bind(e.device)
    .bind(e.connection)
    .bind(e.dashboard)
    .bind(e.widget)
    .bind(correlation)
    .bind(client_kind as i16)
    .execute(db)
    .await?;
    tracing::info!(kind = e.kind.proto_name(), "audit event recorded");
    Ok(())
}
