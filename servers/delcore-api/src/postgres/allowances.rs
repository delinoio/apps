// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::accounting::{Decimal, prorated_allowance};
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use std::collections::HashMap;
use uuid::Uuid;

pub type Allowances = HashMap<String, Decimal>;
async fn current_allowances_inner(
    db: &mut PgConnection,
    tenant: &str,
) -> anyhow::Result<Allowances> {
    let rows:Vec<(String,String,String)> = sqlx::query_as("SELECT p.service_id::text,p.meter,sum(p.included)::text FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=$1 AND en.ends_at IS NULL AND p.kind=1 GROUP BY p.service_id,p.meter")
        .bind(Uuid::parse_str(tenant)?).fetch_all(db).await?;
    rows.into_iter()
        .map(|(service, meter, amount)| {
            Ok((format!("{service}\0{meter}"), Decimal::parse(&amount)?))
        })
        .collect()
}
#[allow(clippy::too_many_arguments)]
async fn grant_allowances_inner(
    db: &mut PgConnection,
    tenant: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    at: DateTime<Utc>,
    source: &str,
    previous: Option<&Allowances>,
) -> anyhow::Result<()> {
    let current = current_allowances_inner(db, tenant).await?;
    for (key, mut amount) in current {
        if let Some(previous) = previous {
            amount = amount.sub(previous.get(&key).unwrap_or(&Decimal::zero()));
            if amount <= Decimal::zero() {
                continue;
            }
            let duration = (end - start)
                .num_microseconds()
                .ok_or_else(|| anyhow::anyhow!("allowance period exceeds supported duration"))?;
            let remaining = (end - at)
                .num_microseconds()
                .ok_or_else(|| anyhow::anyhow!("allowance remainder exceeds supported duration"))?
                .max(0)
                .min(duration);
            amount = prorated_allowance(&amount, remaining, duration)?;
        }
        let (service, meter) = key
            .split_once('\0')
            .ok_or_else(|| anyhow::anyhow!("invalid allowance key"))?;
        let grant_id = Uuid::now_v7();
        let result=sqlx::query("INSERT INTO allowance_grants VALUES($1,$2,$3,$4,$5::numeric,$6,$7,$8,$9) ON CONFLICT(source_key) DO NOTHING")
            .bind(grant_id).bind(Uuid::parse_str(tenant)?).bind(Uuid::parse_str(service)?).bind(meter).bind(amount.to_string()).bind(start).bind(end).bind(at).bind(format!("{source}:{service}:{meter}")).execute(&mut *db).await?;
        // Replayed grants change nothing. A new backdated grant can reduce an existing charge.
        if result.rows_affected() == 1 {
            super::rating::reconcile_granted_periods(
                db,
                &grant_id.to_string(),
                tenant,
                service,
                meter,
                start,
                Utc::now(),
            )
            .await?;
        }
    }
    Ok(())
}

pub async fn current_allowances(db: &mut PgConnection, tenant: &str) -> crate::Result<Allowances> {
    current_allowances_inner(db, tenant)
        .await
        .map_err(Into::into)
}
#[allow(clippy::too_many_arguments)]
pub async fn grant_allowances(
    db: &mut PgConnection,
    tenant: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    at: DateTime<Utc>,
    source: &str,
    previous: Option<&Allowances>,
) -> crate::Result<()> {
    grant_allowances_inner(db, tenant, start, end, at, source, previous)
        .await
        .map_err(Into::into)
}
