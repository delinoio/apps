// SPDX-License-Identifier: FSL-1.1-ALv2
//! Stored operator settings. The singleton row is seeded from the environment once and then
//! changes only through the audited update path.
use super::audit::{self, Action, AuditEntry};
use crate::{
    error::{Fault, Result},
    settings::Settings,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgRow};

fn from_row(row: &PgRow) -> std::result::Result<Settings, sqlx::Error> {
    let int = |name: &str| -> std::result::Result<u32, sqlx::Error> {
        Ok(row.try_get::<i32, _>(name)? as u32)
    };
    let models: serde_json::Value = row.try_get("translation_models")?;
    Ok(Settings {
        max_input_bytes: int("max_input_bytes")?,
        max_translation_items: int("max_translation_items")?,
        execution_timeout_seconds: int("execution_timeout_seconds")?,
        tenant_requests_per_minute: int("tenant_requests_per_minute")?,
        tenant_concurrency: int("tenant_concurrency")?,
        max_ai_retries: int("max_ai_retries")?,
        price_sync_seconds: int("price_sync_seconds")?,
        delivery_retry_seconds: int("delivery_retry_seconds")?,
        max_backoff_seconds: int("max_backoff_seconds")?,
        alert_backlog: int("alert_backlog")?,
        alert_expiry_seconds: int("alert_expiry_seconds")?,
        translation_models: serde_json::from_value(models)
            .map_err(|e| sqlx::Error::Decode(e.into()))?,
    })
}

/// Insert the settings only when the table is empty. Returns whether this call seeded them.
pub async fn seed(pool: &PgPool, settings: &Settings, now: DateTime<Utc>) -> Result<bool> {
    let result = sqlx::query(
        "INSERT INTO settings(max_input_bytes,max_translation_items,execution_timeout_seconds,tenant_requests_per_minute,tenant_concurrency,max_ai_retries,price_sync_seconds,delivery_retry_seconds,max_backoff_seconds,alert_backlog,alert_expiry_seconds,translation_models,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$13) ON CONFLICT (singleton) DO NOTHING",
    )
    .bind(settings.max_input_bytes as i32)
    .bind(settings.max_translation_items as i32)
    .bind(settings.execution_timeout_seconds as i32)
    .bind(settings.tenant_requests_per_minute as i32)
    .bind(settings.tenant_concurrency as i32)
    .bind(settings.max_ai_retries as i32)
    .bind(settings.price_sync_seconds as i32)
    .bind(settings.delivery_retry_seconds as i32)
    .bind(settings.max_backoff_seconds as i32)
    .bind(settings.alert_backlog as i32)
    .bind(settings.alert_expiry_seconds as i32)
    .bind(json!(settings.translation_models))
    .bind(now)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn load(pool: &PgPool) -> Result<Settings> {
    let row = sqlx::query("SELECT * FROM settings")
        .fetch_optional(pool)
        .await?
        .ok_or_else(Fault::internal)?;
    Ok(from_row(&row)?)
}

/// Replace the settings and write the audit record in one transaction. Returns the previous
/// settings and the stored version. The caller has already validated the operator, reason and
/// values.
pub async fn update(
    pool: &PgPool,
    new: &Settings,
    entry: AuditEntry<'_>,
    now: DateTime<Utc>,
) -> Result<(Settings, i64)> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT * FROM settings FOR UPDATE")
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Fault::internal)?;
    let previous = from_row(&row)?;
    let changed = previous.changed_fields(new);
    sqlx::query("UPDATE settings SET max_input_bytes=$1,max_translation_items=$2,execution_timeout_seconds=$3,tenant_requests_per_minute=$4,tenant_concurrency=$5,max_ai_retries=$6,price_sync_seconds=$7,delivery_retry_seconds=$8,max_backoff_seconds=$9,alert_backlog=$10,alert_expiry_seconds=$11,translation_models=$12,version=version+1,updated_at=$13")
        .bind(new.max_input_bytes as i32)
        .bind(new.max_translation_items as i32)
        .bind(new.execution_timeout_seconds as i32)
        .bind(new.tenant_requests_per_minute as i32)
        .bind(new.tenant_concurrency as i32)
        .bind(new.max_ai_retries as i32)
        .bind(new.price_sync_seconds as i32)
        .bind(new.delivery_retry_seconds as i32)
        .bind(new.max_backoff_seconds as i32)
        .bind(new.alert_backlog as i32)
        .bind(new.alert_expiry_seconds as i32)
        .bind(json!(new.translation_models))
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let version: i64 = sqlx::query_scalar("SELECT version FROM settings")
        .fetch_one(&mut *tx)
        .await?;
    // Settings are safe operational values (limits and model identifiers), so the audit record
    // keeps the changed names with their previous and new values.
    let details = json!({
        "version": version,
        "changed": changed,
        "previous": snapshot(&previous, &changed),
        "current": snapshot(new, &changed),
    });
    audit::insert(
        &mut tx,
        AuditEntry {
            action: Action::SettingsUpdate,
            details,
            ..entry
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok((previous, version))
}

fn snapshot(settings: &Settings, only: &[&str]) -> serde_json::Value {
    let all = settings.to_json();
    let mut out = serde_json::Map::new();
    for name in only {
        if let Some(value) = all.get(*name) {
            out.insert((*name).to_owned(), value.clone());
        }
    }
    serde_json::Value::Object(out)
}
