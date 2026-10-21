// SPDX-License-Identifier: FSL-1.1-ALv2

use super::Worker;
use crate::accounting::{Decimal, USD, USD_MINOR_UNITS};
use anyhow::Result;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use sqlx::{PgConnection, Row};
use std::collections::BTreeMap;
use uuid::Uuid;

struct Line {
    service: Option<Uuid>,
    meter: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    amount: Decimal,
    units: BigInt,
}
#[derive(Default)]
struct Batch {
    amount: Decimal,
    units: BigInt,
    lines: Vec<Line>,
}

impl Worker {
    pub(super) async fn settle(&self, tx: &mut PgConnection, now: DateTime<Utc>) -> Result<()> {
        let grace = now - Duration::seconds(self.config.reporting_grace_seconds);
        let rows = sqlx::query(r#"SELECT l.tenant_id,l.service_id,l.meter,l.period_start,l.period_end,
 (-sum(l.amount))::text AS total,
 COALESCE((SELECT sum(s.amount) FROM settlements s WHERE s.tenant_id=l.tenant_id AND s.service_id=l.service_id AND s.meter=l.meter AND s.period_start=l.period_start),0)::text AS claimed,
 COALESCE((SELECT sum(s.billed_minor) FROM settlements s WHERE s.tenant_id=l.tenant_id AND s.service_id=l.service_id AND s.meter=l.meter AND s.period_start=l.period_start),0)::text AS billed
 FROM ledger l JOIN tenants t ON t.id=l.tenant_id WHERE l.kind='postpaid'
 AND (l.period_end<=$1 OR t.deleting_at < $2)
 GROUP BY l.tenant_id,l.service_id,l.meter,l.period_start,l.period_end"#)
            .bind(now).bind(grace).fetch_all(&mut *tx).await?;
        let mut batches: BTreeMap<Uuid, Batch> = BTreeMap::new();
        for row in rows {
            let total = Decimal::parse(row.try_get("total")?)?;
            let claimed = Decimal::parse(row.try_get("claimed")?)?;
            let amount = total.sub(&claimed);
            if amount.is_zero() {
                continue;
            }
            let billed: BigInt = row.try_get::<&str, _>("billed")?.parse()?;
            let units = total.minor_units(USD_MINOR_UNITS)? - billed;
            let batch = batches.entry(row.try_get("tenant_id")?).or_default();
            batch.amount = batch.amount.add(&amount);
            batch.units += &units;
            batch.lines.push(Line {
                service: row.try_get("service_id")?,
                meter: row.try_get("meter")?,
                start: row.try_get("period_start")?,
                end: row.try_get("period_end")?,
                amount,
                units,
            });
        }
        for (tenant, mut batch) in batches {
            // Credit allocations can remain fractional while other periods are open.
            // Carry that fraction across invoices by rounding cumulative receivables.
            let row = sqlx::query(r#"SELECT (-COALESCE(sum(l.amount),0))::text AS cumulative,COALESCE((SELECT sum(billed_minor) FROM settlements WHERE tenant_id=$1),0)::text AS paid,COALESCE((SELECT sum(amount) FROM settlements WHERE tenant_id=$1),0)::text AS claimed FROM ledger l JOIN tenants t ON t.id=l.tenant_id WHERE l.tenant_id=$1 AND l.kind='postpaid' AND (l.period_end<=$2 OR t.deleting_at<$3)"#)
                .bind(tenant).bind(now).bind(grace).fetch_one(&mut *tx).await?;
            let cumulative = Decimal::parse(row.try_get("cumulative")?)?;
            let claimed = Decimal::parse(row.try_get("claimed")?)?;
            let amount = cumulative.sub(&claimed);
            let difference = amount.sub(&batch.amount);
            // Invoice refunds are tenant offsets, not fabricated meter usage.
            if !difference.is_zero() {
                let first = &batch.lines[0];
                batch.lines.push(Line {
                    service: None,
                    meter: String::new(),
                    start: first.start,
                    end: first.end,
                    amount: difference,
                    units: BigInt::zero(),
                });
            }
            batch.amount = amount;
            let paid: BigInt = row.try_get::<&str, _>("paid")?.parse()?;
            let units = cumulative.minor_units(USD_MINOR_UNITS)? - paid;
            if let Some(first) = batch.lines.first_mut() {
                first.units += &units - &batch.units;
            }
            batch.units = units;
            if batch.amount.is_zero() && batch.units.is_zero() {
                continue;
            }
            // A net credit remains an attributed adjustment for the next bill.
            if batch.units.is_negative() {
                continue;
            }
            let job = Uuid::now_v7();
            let done = batch.units.is_zero();
            let state = if done { "done" } else { "pending" };
            let payload = serde_json::json!({"amount": batch.amount.to_string(), "minor_units": batch.units.to_string(), "currency": USD});
            sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,state,available_at,created_at,updated_at) VALUES($1,$2,'settlement',$1::uuid::text,$3,$4,$5,$5,$5)")
                .bind(job).bind(tenant).bind(payload).bind(state).bind(now).execute(&mut *tx).await?;
            for line in batch.lines {
                let source = format!(
                    "{}:{}:{}:{}",
                    job,
                    line.service.map(|id| id.to_string()).unwrap_or_default(),
                    line.meter,
                    rfc3339_nano(line.start)
                );
                sqlx::query("INSERT INTO settlements(id,tenant_id,period_start,period_end,service_id,meter,amount,source_key,job_id,created_at,billed_minor) VALUES($1,$2,$3,$4,$5,$6,$7::text::numeric,$8,$9,$10,$11::text::numeric)")
                    .bind(Uuid::now_v7()).bind(tenant).bind(line.start).bind(line.end).bind(line.service).bind(line.meter).bind(line.amount.to_string()).bind(source).bind(job).bind(now).bind(line.units.to_string()).execute(&mut *tx).await?;
            }
            if done {
                sqlx::query("INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2,'settlement',$3::text::numeric,$4,$5)")
                    .bind(Uuid::now_v7()).bind(tenant).bind(batch.amount.to_string()).bind(format!("rounding:{job}")).bind(now).execute(&mut *tx).await?;
            }
        }
        Ok(())
    }
}

// Match Go RFC3339Nano source keys, including trimming fractional zeroes.
pub(super) fn rfc3339_nano(time: DateTime<Utc>) -> String {
    let value = time.to_rfc3339_opts(SecondsFormat::Nanos, true);
    let (seconds, fraction) = value.split_once('.').expect("nanosecond timestamp");
    let fraction = fraction.trim_end_matches('Z').trim_end_matches('0');
    if fraction.is_empty() {
        format!("{seconds}Z")
    } else {
        format!("{seconds}.{fraction}Z")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_timestamp_preserves_original_format() {
        for value in [
            "2026-10-21T01:02:03Z",
            "2026-10-21T01:02:03.1234Z",
            "2026-10-21T01:02:03.000000001Z",
        ] {
            let time = DateTime::parse_from_rfc3339(value)
                .unwrap()
                .with_timezone(&Utc);
            assert_eq!(rfc3339_nano(time), value);
        }
    }
}
