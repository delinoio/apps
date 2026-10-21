// SPDX-License-Identifier: FSL-1.1-ALv2

use super::Worker;
use crate::{postgres, rating};
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use sqlx::{PgConnection, Row};

impl Worker {
    /// Round whole meter charges, including purchased-credit allocations, only
    /// after the period closes or the tenant's deletion reporting grace ends.
    pub(super) async fn round_usage(
        &self,
        tx: &mut PgConnection,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let rows = sqlx::query("SELECT r.tenant_id::text,r.service_id::text,r.meter_id::text,r.meter,r.period_start,r.period_end FROM metering_rated_periods r JOIN tenants t ON t.id=r.tenant_id WHERE r.period_end<=$1 OR t.deleting_at<$2 ORDER BY r.tenant_id,r.meter_id,r.period_start")
            .bind(now).bind(now - Duration::seconds(self.config.reporting_grace_seconds)).fetch_all(&mut *tx).await?;
        for row in rows {
            let period = postgres::MeterPeriod {
                tenant_id: row.try_get(0)?,
                service_id: row.try_get(1)?,
                meter_id: row.try_get(2)?,
                meter_key: row.try_get(3)?,
                start: row.try_get(4)?,
                end: row.try_get(5)?,
            };
            let result = match postgres::rate_meter_period(tx, &period).await {
                Ok(result) => result,
                Err(error) if error.downcast_ref::<rating::Inconsistent>().is_some() => {
                    // Preserve the last charges until the failed metering work is
                    // recovered. One inconsistent period must not stop maintenance.
                    tracing::error!(category="metering_invalid", meter=%period.meter_key, "rated period cannot be rounded");
                    continue;
                }
                Err(error) => return Err(error),
            };
            let total = rating::rounded_charge(&result.charge)?;
            let postpaid = postgres::period_postpaid(tx, &period.tenant_id, period.start).await?;
            postgres::append_charge_difference(
                tx,
                &period,
                &total,
                postpaid,
                postgres::LedgerKind::Correction,
                &format!("currency_rounding:{}", uuid::Uuid::now_v7()),
                now,
            )
            .await?;
        }
        Ok(())
    }
}
