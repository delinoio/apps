// SPDX-License-Identifier: FSL-1.1-ALv2
use super::Worker;
use crate::postgres::{self, recovery};
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

/// Distinct rated tenant periods examined per step, so a long backlog cannot starve other work.
pub const ALLOWANCE_RECOVERY_BATCH: i64 = 25;

impl Worker {
    /// Recover missed free-month baselines for periods that were rated before recovery existed.
    ///
    /// Periods rated from now on recover their baseline in the metering transaction that rates
    /// them, so this sweep only walks the rated periods once, in `(tenant, period start)` order.
    /// Sparse history costs one row per examined rated period, never one per calendar month. The
    /// position (see migration 14) moves in the transaction that examined its batch, so a failure
    /// or restart retries the same batch and the grant keys keep the retry idempotent. A period
    /// whose retained history is contradictory is reported through an audit record and a log and
    /// then passed; it receives no grant. Returns the number of grants recorded.
    pub async fn recover_free_allowances(&self) -> Result<usize> {
        let mut tx = postgres::begin(&self.pool).await?;
        let (after_tenant, after_start): (Uuid, DateTime<Utc>) = sqlx::query_as(
            "SELECT tenant_id,period_start FROM allowance_recovery_cursor WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        let rows = sqlx::query("SELECT DISTINCT ON (tenant_id,period_start) tenant_id,period_start,period_end FROM metering_rated_periods WHERE (tenant_id,period_start)>($1,$2) ORDER BY tenant_id,period_start LIMIT $3")
            .bind(after_tenant)
            .bind(after_start)
            .bind(ALLOWANCE_RECOVERY_BATCH)
            .fetch_all(&mut *tx)
            .await?;
        let Some(last) = rows.last() else {
            tx.commit().await?;
            return Ok(0);
        };
        let position: (Uuid, DateTime<Utc>) = (last.try_get(0)?, last.try_get(1)?);
        let (mut granted, mut invalid) = (0, 0);
        for row in &rows {
            let tenant: Uuid = row.try_get(0)?;
            let start: DateTime<Utc> = row.try_get(1)?;
            let end: DateTime<Utc> = row.try_get(2)?;
            match recovery::recover_free_period(&mut tx, &tenant.to_string(), start, end).await {
                Ok(recovery::Recovery::Granted(grants)) => granted += grants,
                Ok(recovery::Recovery::Skipped(_)) => {}
                Err(error) => {
                    let Some(history) = error.downcast_ref::<recovery::HistoryInvalid>() else {
                        return Err(error);
                    };
                    invalid += 1;
                    tracing::error!(
                        category = "allowance_recovery_invalid",
                        reason = %history,
                        "free allowance history is inconsistent; no baseline was granted"
                    );
                    sqlx::query("INSERT INTO audit(id,actor_id,tenant_id,action,reason,correlation_id,created_at) VALUES($1,'system:allowance_recovery',$2,'allowance_recovery.invalid',$3,$4,$5)")
                        .bind(Uuid::now_v7())
                        .bind(tenant)
                        .bind(format!("period={} {history}", start.format("%Y-%m")))
                        .bind(Uuid::now_v7())
                        .bind(Utc::now())
                        .execute(&mut *tx)
                        .await?;
                }
            }
        }
        sqlx::query("UPDATE allowance_recovery_cursor SET tenant_id=$1,period_start=$2,updated_at=now() WHERE singleton")
            .bind(position.0)
            .bind(position.1)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(
            category = "allowance_recovery",
            examined = rows.len(),
            granted,
            invalid,
            "free allowance recovery batch finished"
        );
        Ok(granted)
    }
}
