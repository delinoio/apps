// SPDX-License-Identifier: FSL-1.1-ALv2

use super::Worker;
use super::settlement::rfc3339_nano;
use crate::{
    accounting::{Decimal, USD_MINOR_UNITS},
    provider::Event,
};
use anyhow::{Result, bail};
use serde::Deserialize;
use sqlx::{PgConnection, Row};
use std::collections::HashMap;
use uuid::Uuid;

pub(super) async fn record_settlement_line(
    conn: &mut PgConnection,
    transaction: &str,
    item: &str,
    job: &str,
    amount: &str,
    minor: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO settlement_payment_lines VALUES($1,$2,$3,$4::text::numeric,$5::text::numeric) ON CONFLICT DO NOTHING")
        .bind(transaction).bind(item).bind(Uuid::parse_str(job)?).bind(amount).bind(minor).execute(&mut *conn).await?;
    Ok(())
}

#[derive(Deserialize, Default)]
struct RefundTotals {
    #[serde(default)]
    subtotal: String,
}
#[derive(Deserialize)]
struct RefundItem {
    #[serde(default)]
    item_id: String,
    #[serde(default)]
    totals: RefundTotals,
}
#[derive(Deserialize)]
struct RefundPayload {
    #[serde(default)]
    items: Vec<RefundItem>,
}

impl Worker {
    // Only recorded usage invoice lines affect usage receivables. Subscription
    // refunds remain payment history and cannot erase usage debt.
    pub(super) async fn apply_settlement_refund(
        &self,
        conn: &mut PgConnection,
        event: &Event,
        tenant: &str,
        adjustment: &str,
        transaction: &str,
        status: &str,
    ) -> Result<()> {
        let payload: RefundPayload = serde_json::from_value(event.data.clone())?;
        let mut amounts = HashMap::new();
        for item in payload.items {
            if amounts.insert(item.item_id, item.totals.subtotal).is_some() {
                bail!("duplicate adjustment line");
            }
        }
        let lines = sqlx::query(r#"SELECT p.item_id,p.job_id,p.amount::text AS principal,p.billed_minor::text AS paid,COALESCE(r.billed_minor,0)::text AS prior,COALESCE((SELECT sum(o.billed_minor) FROM settlement_refund_effects o WHERE o.transaction_id=p.transaction_id AND o.item_id=p.item_id AND o.adjustment_id<>$2),0)::text AS other FROM settlement_payment_lines p LEFT JOIN settlement_refund_effects r ON r.transaction_id=p.transaction_id AND r.item_id=p.item_id AND r.adjustment_id=$2 WHERE p.transaction_id=$1"#)
            .bind(transaction).bind(adjustment).fetch_all(&mut *conn).await?;
        if lines.is_empty() {
            let unmapped: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE external_id=$1 AND kind='settlement' AND state='done')")
                .bind(transaction).fetch_one(&mut *conn).await?;
            if unmapped {
                bail!("usage refund payment attribution missing");
            }
        }
        if status == "approved" && !lines.is_empty() && amounts.is_empty() {
            bail!("usage refund line attribution missing");
        }
        for line in lines {
            let item: &str = line.try_get("item_id")?;
            let current = if status == "approved" {
                amounts.get(item).map(String::as_str).unwrap_or("0")
            } else {
                "0"
            };
            let amount = Decimal::parse(current)
                .map_err(|_| anyhow::anyhow!("invalid usage refund subtotal"))?;
            if amount < Decimal::zero() || !amount.is_minor_exact(0) {
                bail!("invalid usage refund subtotal");
            }
            let prior: &str = line.try_get("prior")?;
            let other: &str = line.try_get("other")?;
            let paid: &str = line.try_get("paid")?;
            let principal: &str = line.try_get("principal")?;
            let valid: bool =
                sqlx::query_scalar("SELECT $1::text::numeric+$2::text::numeric<=$3::text::numeric")
                    .bind(current)
                    .bind(other)
                    .bind(paid)
                    .fetch_one(&mut *conn)
                    .await?;
            if !valid {
                bail!("usage refund exceeds paid line");
            }
            if current == prior {
                continue;
            }
            // The last refund includes the original sub-minor residual. Partial
            // refunds use exact currency units, without proportional division.
            let (delta, minor): (String, String) = sqlx::query_as(r#"SELECT ((CASE WHEN $1::text::numeric+$3::text::numeric=$4::text::numeric THEN $5::text::numeric ELSE ($1::text::numeric+$3::text::numeric)/power(10::numeric,$6::int) END)-(CASE WHEN $2::text::numeric+$3::text::numeric=$4::text::numeric THEN $5::text::numeric ELSE ($2::text::numeric+$3::text::numeric)/power(10::numeric,$6::int) END))::text,($1::text::numeric-$2::text::numeric)::text"#)
                .bind(prior).bind(current).bind(other).bind(paid).bind(principal).bind(USD_MINOR_UNITS as i32).fetch_one(&mut *conn).await?;
            let source = format!(
                "usage-refund:{adjustment}:{item}:{}",
                rfc3339_nano(event.occurred_at)
            );
            let tenant = Uuid::parse_str(tenant)?;
            sqlx::query("INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2,'settlement',$3::text::numeric,$4,$5)")
                .bind(Uuid::now_v7()).bind(tenant).bind(&delta).bind(&source).bind(event.occurred_at).execute(&mut *conn).await?;
            sqlx::query("INSERT INTO settlements(id,tenant_id,period_start,period_end,service_id,meter,amount,source_key,job_id,created_at,billed_minor) VALUES($1,$2,$3,$3,NULL,'',$4::text::numeric,$5,$6,$3,$7::text::numeric)")
                .bind(Uuid::now_v7()).bind(tenant).bind(event.occurred_at).bind(delta).bind(source).bind(line.try_get::<Uuid, _>("job_id")?).bind(minor).execute(&mut *conn).await?;
            sqlx::query("INSERT INTO settlement_refund_effects VALUES($1,$2,$3,$4::text::numeric) ON CONFLICT(adjustment_id,item_id) DO UPDATE SET billed_minor=EXCLUDED.billed_minor")
                .bind(adjustment).bind(transaction).bind(item).bind(current).execute(&mut *conn).await?;
        }
        Ok(())
    }
}
