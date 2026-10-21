// SPDX-License-Identifier: FSL-1.1-ALv2
//! Restart recovery and backlog statistics.
use crate::error::Result;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

/// Mark every request and idempotency record still `processing` as `unknown`. The service runs
/// as a single instance, so a processing row at startup belongs to a process that no longer
/// exists. An interrupted AI prompt is never resumed or repeated; usage processing for its
/// attempts continues through the normal reconciliation path. Returns the number of requests.
pub async fn mark_interrupted_unknown(pool: &PgPool, now: DateTime<Utc>) -> Result<u64> {
    let mut tx = pool.begin().await?;
    let requests = sqlx::query("UPDATE requests SET state=3,updated_at=$1 WHERE state=1")
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("UPDATE idempotency_records SET state=3,updated_at=$1 WHERE state=1")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(requests)
}

/// Point-in-time operational counts exposed as gauges. No identifiers, tenants or models.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Backlog {
    /// Attempts whose measured usage is not yet known.
    pub unknown_usage: i64,
    /// Attempts with known usage that are not yet delivered.
    pub pending_delivery: i64,
    /// Requests whose outcome is unknown.
    pub unknown_requests: i64,
    /// Undelivered attempts that expire within the alert window.
    pub nearing_expiry: i64,
    /// Whether the catalog holds at least one synchronized price.
    pub price_available: bool,
}

pub async fn backlog(
    pool: &PgPool,
    now: DateTime<Utc>,
    alert_expiry_seconds: u32,
) -> Result<Backlog> {
    // An undelivered attempt created before this instant expires within the alert window.
    let near =
        super::retention::cutoff(now) + chrono::Duration::seconds(alert_expiry_seconds.into());
    let row = sqlx::query(
        "SELECT \
         (SELECT count(*) FROM attempts WHERE delivered_at IS NULL AND measured_usage IS NULL) AS unknown_usage, \
         (SELECT count(*) FROM attempts WHERE delivered_at IS NULL AND measured_usage IS NOT NULL) AS pending_delivery, \
         (SELECT count(*) FROM requests WHERE state=3) AS unknown_requests, \
         (SELECT count(*) FROM attempts WHERE delivered_at IS NULL AND created_at <= $1) AS nearing_expiry, \
         EXISTS(SELECT 1 FROM catalog_prices) AS price_available",
    )
    .bind(near)
    .fetch_one(pool)
    .await?;
    Ok(Backlog {
        unknown_usage: row.try_get("unknown_usage")?,
        pending_delivery: row.try_get("pending_delivery")?,
        unknown_requests: row.try_get("unknown_requests")?,
        nearing_expiry: row.try_get("nearing_expiry")?,
        price_available: row.try_get("price_available")?,
    })
}
