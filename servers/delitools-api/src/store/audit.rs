// SPDX-License-Identifier: FSL-1.1-ALv2
//! Platform audit records and safe operational events. Both expire 30 days after creation.
//! Neither may carry inputs, schemas, results, credentials or raw identity values.
use crate::error::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

/// Audited operator actions. Stored as stable lowercase identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SettingsUpdate,
    RequestInspect,
    GenerationAssociate,
}
impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SettingsUpdate => "settings.update",
            Self::RequestInspect => "request.inspect",
            Self::GenerationAssociate => "generation.associate",
        }
    }
    /// Mutations require a reason; read-only inspection has none.
    pub fn requires_reason(self) -> bool {
        !matches!(self, Self::RequestInspect)
    }
}

pub struct AuditEntry<'a> {
    pub action: Action,
    /// `operator:<subject hash>`.
    pub actor: &'a str,
    pub reason: &'a str,
    pub request_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub details: Value,
}

pub async fn insert(
    conn: &mut PgConnection,
    entry: AuditEntry<'_>,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query("INSERT INTO audit_records(id,actor,action,reason,request_id,tenant_id,target_id,details,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(Uuid::now_v7())
        .bind(entry.actor)
        .bind(entry.action.as_str())
        .bind(entry.reason)
        .bind(entry.request_id)
        .bind(entry.tenant_id)
        .bind(entry.target_id)
        .bind(entry.details)
        .bind(now)
        .execute(conn)
        .await?;
    Ok(())
}

/// Bounded operational event categories. Extension layers add variants together with the code
/// that records them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventCategory {
    Startup,
    SettingsSeeded,
    SettingsUpdated,
    KeyCreated,
    KeyRevoked,
    RetentionSweep,
    InterruptedRequestsRecovered,
    BacklogAlert,
    ExpiryAlert,
    WorkerFailure,
    /// Reserved for the execution layer.
    PriceSyncFailure,
    ProviderFailure,
    UsageDeliveryFailure,
}
impl EventCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::SettingsSeeded => "settings_seeded",
            Self::SettingsUpdated => "settings_updated",
            Self::KeyCreated => "key_created",
            Self::KeyRevoked => "key_revoked",
            Self::RetentionSweep => "retention_sweep",
            Self::InterruptedRequestsRecovered => "interrupted_requests_recovered",
            Self::BacklogAlert => "backlog_alert",
            Self::ExpiryAlert => "expiry_alert",
            Self::WorkerFailure => "worker_failure",
            Self::PriceSyncFailure => "price_sync_failure",
            Self::ProviderFailure => "provider_failure",
            Self::UsageDeliveryFailure => "usage_delivery_failure",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warn,
    Error,
}
impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Persist a safe operational event. `details` must contain only counts, state names and
/// identifiers. Failure to record an event never fails the caller.
pub async fn record_event(
    pool: &PgPool,
    category: EventCategory,
    severity: Severity,
    request_id: Option<Uuid>,
    correlation_id: Uuid,
    details: Value,
) {
    let result = sqlx::query("INSERT INTO operational_events(id,category,severity,request_id,correlation_id,details,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(Uuid::now_v7())
        .bind(category.as_str())
        .bind(severity.as_str())
        .bind(request_id)
        .bind(correlation_id)
        .bind(details)
        .bind(super::now())
        .execute(pool)
        .await;
    if result.is_err() {
        tracing::warn!(
            category = category.as_str(),
            "operational event could not be recorded"
        );
    }
}
