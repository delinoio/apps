// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::accounting::{Decimal, prorated_allowance};
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use std::collections::HashMap;
use uuid::Uuid;

pub type Allowances = HashMap<String, Decimal>;
/// Sums the usage allowances of the entitlements the tenant holds now, or, when `as_of` is set,
/// of the entitlements held at that instant. Starts are inclusive and ends are exclusive.
async fn allowances_inner(
    db: &mut PgConnection,
    tenant: &str,
    as_of: Option<DateTime<Utc>>,
) -> anyhow::Result<Allowances> {
    let rows:Vec<(String,String,String)> = match as_of {
        None => sqlx::query_as("SELECT p.service_id::text,p.meter,sum(p.included)::text FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=$1 AND en.ends_at IS NULL AND p.kind=1 GROUP BY p.service_id,p.meter")
            .bind(Uuid::parse_str(tenant)?).fetch_all(db).await?,
        Some(at) => sqlx::query_as("SELECT p.service_id::text,p.meter,sum(p.included)::text FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=$1 AND en.starts_at<=$2 AND (en.ends_at IS NULL OR en.ends_at>$2) AND p.kind=1 GROUP BY p.service_id,p.meter")
            .bind(Uuid::parse_str(tenant)?).bind(at).fetch_all(db).await?,
    };
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
    as_of: Option<DateTime<Utc>>,
) -> anyhow::Result<()> {
    let current = allowances_inner(db, tenant, as_of).await?;
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
    allowances_inner(db, tenant, None).await.map_err(Into::into)
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
    grant_allowances_inner(db, tenant, start, end, at, source, previous, None)
        .await
        .map_err(Into::into)
}

/// Grants the full allowances the tenant held at `held_at` for the period `[start, end)`.
///
/// A late payment for an already ended period must grant what the tenant held when that period
/// began, not what it holds now: later plan changes never rewrite older periods. The grant is
/// keyed by `source`, so repeating the call adds nothing.
pub async fn grant_allowances_held_at(
    db: &mut PgConnection,
    tenant: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    held_at: DateTime<Utc>,
    source: &str,
) -> crate::Result<()> {
    grant_allowances_inner(db, tenant, start, end, held_at, source, None, Some(held_at))
        .await
        .map_err(Into::into)
}
