// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{
    accounting::{self, Decimal},
    metering::Aggregation,
    rating,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
pub enum LedgerKind {
    Usage,
    Correction,
    Postpaid,
}
impl LedgerKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Correction => "correction",
            Self::Postpaid => "postpaid",
        }
    }
}
#[derive(Clone, Debug)]
pub struct MeterPeriod {
    pub tenant_id: String,
    pub service_id: String,
    pub meter_id: String,
    pub meter_key: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}
pub const ALLOWANCE_GRANTS_QUERY: &str = "SELECT amount::text,applied_at FROM allowance_grants WHERE tenant_id=$1 AND service_id=$2 AND meter=$3 AND period_start=$4 ORDER BY applied_at,id";
// Derive epochs from the ordered grant timestamps, not a grant-table scan per contribution.
pub const CONTRIBUTION_BUCKETS_QUERY: &str = "WITH applied AS (
 SELECT c.group_key,COALESCE(ep.price_id::text,'') price,
 width_bucket(e.occurred_at,$4::timestamptz[]) epoch,
 e.occurred_at,CASE WHEN c.value_kind='number' THEN c.value::numeric END amount
 FROM metering_events e
 JOIN metering_contributions c ON c.event_id=e.id AND c.revision=e.applied_revision AND c.meter_id=$2
 LEFT JOIN metering_event_prices ep ON ep.event_id=e.id AND ep.meter_id=$2
 WHERE e.tenant_id=$1 AND e.period_start=$3 AND e.applied_revision>0)
 SELECT group_key,price,epoch,min(occurred_at),max(occurred_at),count(*),COALESCE(sum(amount),0)::text FROM applied GROUP BY group_key,price,epoch ORDER BY group_key,price,epoch";

pub async fn rate_meter_period(
    db: &mut PgConnection,
    p: &MeterPeriod,
) -> anyhow::Result<rating::Result> {
    let tenant = Uuid::parse_str(&p.tenant_id)?;
    let service = Uuid::parse_str(&p.service_id)?;
    let mut input = rating::Input {
        aggregation: Aggregation::Count,
        groups: Vec::new(),
        buckets: Vec::new(),
        grants: Vec::new(),
        prices: std::collections::HashMap::new(),
        // Call-time priced AI meters carry USD amounts and ignore plan prices.
        call_time_usd: crate::delitools::meter_is_call_time(&mut *db, &p.meter_id)
            .await
            .map_err(|fault| anyhow::anyhow!("meter definition unavailable: {}", fault.message))?,
    };
    let rows = sqlx::query(ALLOWANCE_GRANTS_QUERY)
        .bind(tenant)
        .bind(service)
        .bind(&p.meter_key)
        .bind(p.start)
        .fetch_all(&mut *db)
        .await?;
    let mut grant_times = Vec::<DateTime<Utc>>::new();
    for row in rows {
        input.grants.push(
            Decimal::parse(row.try_get::<&str, _>(0)?)
                .map_err(|_| rating::Inconsistent("stored allowance grant"))?,
        );
        grant_times.push(row.try_get(1)?);
    }
    if input.call_time_usd {
        // No allowance applies, and every epoch is zero.
        input.grants.clear();
        grant_times.clear();
    }
    if p.meter_id.is_empty() {
        return Ok(rating::compute(&input)?);
    }
    let meter = Uuid::parse_str(&p.meter_id)?;
    let rows = sqlx::query("SELECT group_key,aggregation,value::text FROM metering_aggregates WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 ORDER BY group_key")
        .bind(tenant).bind(meter).bind(p.start).fetch_all(&mut *db).await?;
    let mut previous_aggregation = None;
    for row in rows {
        let kind: String = row.try_get(1)?;
        if previous_aggregation
            .as_ref()
            .is_some_and(|previous| previous != &kind)
        {
            return Err(rating::Inconsistent("aggregations differ within a meter").into());
        }
        input.aggregation = serde_json::from_value(serde_json::Value::String(kind.clone()))
            .map_err(|_| rating::Inconsistent("invalid aggregate kind"))?;
        previous_aggregation = Some(kind);
        input.groups.push(rating::Group {
            key: row.try_get(0)?,
            value: Decimal::parse(row.try_get::<&str, _>(2)?)
                .map_err(|_| rating::Inconsistent("stored aggregate value"))?,
        });
    }
    let rows = sqlx::query(CONTRIBUTION_BUCKETS_QUERY)
        .bind(tenant)
        .bind(meter)
        .bind(p.start)
        .bind(grant_times)
        .fetch_all(&mut *db)
        .await?;
    let mut price_ids = Vec::new();
    for row in rows {
        let price_id: String = row.try_get(1)?;
        if !price_id.is_empty() && !input.prices.contains_key(&price_id) {
            price_ids.push(Uuid::parse_str(&price_id)?);
            input.prices.insert(price_id.clone(), Decimal::zero());
        }
        input.buckets.push(rating::Bucket {
            group_key: row.try_get(0)?,
            price_id,
            epoch: row.try_get::<i32, _>(2)?.try_into()?,
            first: row.try_get(3)?,
            last: row.try_get(4)?,
            count: row.try_get(5)?,
            sum: Decimal::parse(row.try_get::<&str, _>(6)?)
                .map_err(|_| rating::Inconsistent("stored contribution sum"))?,
        });
    }
    if !price_ids.is_empty() {
        let rows = sqlx::query(
            "SELECT id::text,unit_price::text,currency FROM prices WHERE id=ANY($1::uuid[])",
        )
        .bind(&price_ids)
        .fetch_all(db)
        .await?;
        if rows.len() != price_ids.len() {
            return Err(rating::Inconsistent("attributed price is missing").into());
        }
        for row in rows {
            let unit = Decimal::parse(row.try_get::<&str, _>(1)?)
                .map_err(|_| rating::Inconsistent("attributed price is not a USD unit price"))?;
            if row.try_get::<&str, _>(2)? != accounting::USD {
                return Err(
                    rating::Inconsistent("attributed price is not a USD unit price").into(),
                );
            }
            input.prices.insert(row.try_get(0)?, unit);
        }
    }
    Ok(rating::compute(&input)?)
}
pub async fn period_postpaid(
    db: &mut PgConnection,
    tenant: &str,
    start: DateTime<Utc>,
) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT postpaid FROM billing_periods WHERE tenant_id=$1 AND starts_at=$2",
    )
    .bind(Uuid::parse_str(tenant)?)
    .bind(start)
    .fetch_optional(db)
    .await?
    .unwrap_or(false))
}
#[allow(clippy::too_many_arguments)]
pub async fn append_charge_difference(
    db: &mut PgConnection,
    p: &MeterPeriod,
    target: &Decimal,
    postpaid: bool,
    mut kind: LedgerKind,
    source_key: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<Decimal> {
    let (charged,unpaid):(String,String) = sqlx::query_as("SELECT (-COALESCE(sum(amount),0))::text,greatest(-COALESCE(sum(amount) FILTER(WHERE kind='postpaid'),0),0)::text FROM ledger WHERE tenant_id=$1 AND service_id=$2 AND meter=$3 AND period_start=$4 AND kind IN ('usage','postpaid','correction')")
        .bind(Uuid::parse_str(&p.tenant_id)?).bind(Uuid::parse_str(&p.service_id)?).bind(&p.meter_key).bind(p.start).fetch_one(&mut *db).await?;
    let delta = target.sub(&Decimal::parse(&charged)?);
    let unpaid = Decimal::parse(&unpaid)?;
    let zero = Decimal::zero();
    if delta == zero {
        return Ok(zero);
    }
    let mut remainder = delta.clone();
    if delta < zero {
        kind = LedgerKind::Correction;
        let refund = delta.neg().min(unpaid);
        if refund > zero {
            append_ledger(
                db,
                p,
                LedgerKind::Postpaid,
                &refund,
                &format!("{source_key}:postpaid"),
                now,
            )
            .await?;
            remainder = remainder.add(&refund);
        }
    } else if postpaid {
        let balance:String = sqlx::query_scalar("SELECT greatest(COALESCE(sum(amount),0),0)::text FROM ledger WHERE tenant_id=$1 AND kind IN ('credit','usage','refund','correction')").bind(Uuid::parse_str(&p.tenant_id)?).fetch_one(&mut *db).await?;
        let balance = Decimal::parse(&balance)?;
        if delta > balance {
            append_ledger(
                db,
                p,
                LedgerKind::Postpaid,
                &delta.sub(&balance).neg(),
                &format!("{source_key}:postpaid"),
                now,
            )
            .await?;
            remainder = balance;
        }
    }
    if remainder != zero {
        append_ledger(db, p, kind, &remainder.neg(), source_key, now).await?;
    }
    Ok(delta)
}
async fn append_ledger(
    db: &mut PgConnection,
    p: &MeterPeriod,
    kind: LedgerKind,
    amount: &Decimal,
    source_key: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO ledger(id,tenant_id,service_id,meter,kind,amount,period_start,period_end,source_key,created_at) VALUES($1,$2,$3,$4,$5,$6::numeric,$7,$8,$9,$10)")
        .bind(Uuid::now_v7()).bind(Uuid::parse_str(&p.tenant_id)?).bind(Uuid::parse_str(&p.service_id)?).bind(&p.meter_key).bind(kind.as_str()).bind(amount.to_string()).bind(p.start).bind(p.end).bind(source_key).bind(now).execute(db).await?;
    Ok(())
}
pub async fn reconcile_meter_period(
    db: &mut PgConnection,
    p: &MeterPeriod,
    kind: LedgerKind,
    source_key: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<Decimal> {
    let result = rate_meter_period(db, p).await?;
    let postpaid = period_postpaid(db, &p.tenant_id, p.start).await?;
    let delta =
        append_charge_difference(db, p, &result.charge, postpaid, kind, source_key, now).await?;
    record_rating(db, p, &result, now).await?;
    Ok(delta)
}
pub async fn reconcile_granted_periods(
    db: &mut PgConnection,
    grant_id: &str,
    tenant: &str,
    service: &str,
    meter: &str,
    start: DateTime<Utc>,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let rows:Vec<(String,DateTime<Utc>)> = sqlx::query_as("SELECT meter_id::text,period_end FROM metering_rated_periods WHERE tenant_id=$1 AND service_id=$2 AND meter=$3 AND period_start=$4 ORDER BY meter_id").bind(Uuid::parse_str(tenant)?).bind(Uuid::parse_str(service)?).bind(meter).bind(start).fetch_all(&mut *db).await?;
    for (meter_id, end) in rows {
        let p = MeterPeriod {
            tenant_id: tenant.into(),
            service_id: service.into(),
            meter_id,
            meter_key: meter.into(),
            start,
            end,
        };
        match reconcile_meter_period(
            db,
            &p,
            LedgerKind::Correction,
            &format!("allowance:{grant_id}:{}", p.meter_id),
            now,
        )
        .await
        {
            Err(e) if e.is::<rating::Inconsistent>() => tracing::error!(
                category = "metering_invalid",
                meter,
                "rated period cannot be reconciled with an allowance grant"
            ),
            Err(e) => return Err(e),
            Ok(delta) => {
                tracing::debug!(category="metering_processing",meter,difference=%delta,"rated period reconciled with an allowance grant")
            }
        }
    }
    Ok(())
}
pub async fn record_rating(
    db: &mut PgConnection,
    p: &MeterPeriod,
    r: &rating::Result,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO metering_rated_periods(tenant_id,meter_id,service_id,meter,period_start,period_end,charge,remaining_included,rated_at) VALUES($1,$2,$3,$4,$5,$6,$7::numeric,$8::numeric,$9) ON CONFLICT(tenant_id,meter_id,period_start) DO UPDATE SET charge=EXCLUDED.charge,remaining_included=EXCLUDED.remaining_included,rated_at=EXCLUDED.rated_at")
        .bind(Uuid::parse_str(&p.tenant_id)?).bind(Uuid::parse_str(&p.meter_id)?).bind(Uuid::parse_str(&p.service_id)?).bind(&p.meter_key).bind(p.start).bind(p.end).bind(r.charge.to_string()).bind(r.remaining.to_string()).bind(now).execute(db).await?;
    Ok(())
}
async fn has_allowance_inner(
    db: &mut PgConnection,
    tenant: &str,
    service: &str,
    start: DateTime<Utc>,
) -> anyhow::Result<bool> {
    let rows:Vec<(String,String)> = sqlx::query_as("SELECT DISTINCT p.meter,COALESCE(ce.id::text,'') FROM entitlements en JOIN prices p ON p.id=en.price_id LEFT JOIN catalog_entities ce ON ce.kind='meter' AND ce.scope=p.service_id::text AND ce.key=p.meter WHERE en.tenant_id=$1 AND p.service_id=$2 AND p.kind=1 AND en.ends_at IS NULL ORDER BY 1,2")
        .bind(Uuid::parse_str(tenant)?).bind(Uuid::parse_str(service)?).fetch_all(&mut *db).await?;
    for (meter_key, meter_id) in rows {
        let p = MeterPeriod {
            tenant_id: tenant.into(),
            service_id: service.into(),
            meter_id,
            meter_key,
            start,
            end: start,
        };
        if rate_meter_period(db, &p).await?.remaining > Decimal::zero() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn has_allowance(
    db: &mut PgConnection,
    tenant: &str,
    service: &str,
    start: DateTime<Utc>,
) -> crate::Result<bool> {
    has_allowance_inner(db, tenant, service, start)
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use sqlx::Connection;

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL through TEST_DATABASE_URL"]
    async fn rating_queries_use_period_index_and_precomputed_epochs() -> anyhow::Result<()> {
        let mut db = sqlx::PgConnection::connect(&std::env::var("TEST_DATABASE_URL")?).await?;
        // Transaction-local tables isolate the planner assertion from existing database data.
        sqlx::raw_sql("CREATE TEMP TABLE allowance_grants(id uuid,tenant_id uuid,service_id uuid,meter text,period_start timestamptz,amount numeric,applied_at timestamptz); CREATE INDEX allowance_grants_period ON allowance_grants(tenant_id,service_id,meter,period_start,applied_at,id); CREATE TEMP TABLE metering_events(id uuid,tenant_id uuid,period_start timestamptz,applied_revision bigint,occurred_at timestamptz); CREATE TEMP TABLE metering_contributions(event_id uuid,revision bigint,meter_id uuid,group_key text,value_kind text,value text); CREATE TEMP TABLE metering_event_prices(event_id uuid,meter_id uuid,price_id uuid);").execute(&mut db).await?;
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let plan: serde_json::Value = sqlx::query_scalar(&format!(
            "EXPLAIN (FORMAT JSON) {CONTRIBUTION_BUCKETS_QUERY}"
        ))
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(start)
        .bind(vec![start])
        .fetch_one(&mut db)
        .await?;
        assert!(!plan.to_string().contains("allowance_grants"));
        sqlx::query("SET enable_seqscan=off")
            .execute(&mut db)
            .await?;
        let plan: serde_json::Value =
            sqlx::query_scalar(&format!("EXPLAIN (FORMAT JSON) {ALLOWANCE_GRANTS_QUERY}"))
                .bind(Uuid::now_v7())
                .bind(Uuid::now_v7())
                .bind("units")
                .bind(start)
                .fetch_one(&mut db)
                .await?;
        assert!(plan.to_string().contains("allowance_grants_period"));
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL through TEST_DATABASE_URL"]
    async fn allowance_epochs_include_equal_and_duplicate_grants() -> anyhow::Result<()> {
        let mut db = sqlx::PgConnection::connect(&std::env::var("TEST_DATABASE_URL")?).await?;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let grants = vec![
            base,
            base,
            base + Duration::microseconds(1),
            base + Duration::hours(1),
        ];
        for (times, occurrence, want) in [
            (vec![], base, 0),
            (grants.clone(), base - Duration::microseconds(1), 0),
            (grants.clone(), base, 2),
            (grants.clone(), base + Duration::microseconds(1), 3),
            (grants.clone(), base + Duration::minutes(1), 3),
            (grants.clone(), base + Duration::hours(1), 4),
            (grants, base + Duration::hours(48), 4),
        ] {
            let got: i32 =
                sqlx::query_scalar("SELECT width_bucket($1::timestamptz,$2::timestamptz[])")
                    .bind(occurrence)
                    .bind(times)
                    .fetch_one(&mut db)
                    .await?;
            assert_eq!(got, want);
        }
        Ok(())
    }
    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL through TEST_DATABASE_URL"]
    async fn corrections_reverse_postpaid_before_returning_credit() -> anyhow::Result<()> {
        let mut db = sqlx::PgConnection::connect(&std::env::var("TEST_DATABASE_URL")?).await?;
        sqlx::raw_sql("CREATE TEMP TABLE ledger(id uuid PRIMARY KEY,tenant_id uuid NOT NULL,service_id uuid,meter text,kind text NOT NULL,amount numeric NOT NULL,period_start timestamptz,period_end timestamptz,source_key text UNIQUE NOT NULL,created_at timestamptz NOT NULL)").execute(&mut db).await?;
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let p = MeterPeriod {
            tenant_id: Uuid::now_v7().to_string(),
            service_id: Uuid::now_v7().to_string(),
            meter_id: Uuid::now_v7().to_string(),
            meter_key: "units".into(),
            start,
            end: start + Duration::days(31),
        };
        sqlx::query("INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2,'credit',3,'credit', $3)").bind(Uuid::now_v7()).bind(Uuid::parse_str(&p.tenant_id)?).bind(start).execute(&mut db).await?;
        assert_eq!(
            append_charge_difference(
                &mut db,
                &p,
                &Decimal::parse("10")?,
                true,
                LedgerKind::Usage,
                "usage",
                start
            )
            .await?
            .to_string(),
            "10"
        );
        assert_eq!(
            append_charge_difference(
                &mut db,
                &p,
                &Decimal::parse("5")?,
                true,
                LedgerKind::Correction,
                "correct-one",
                start
            )
            .await?
            .to_string(),
            "-5"
        );
        let balance: String = sqlx::query_scalar(
            "SELECT sum(amount)::text FROM ledger WHERE kind IN ('credit','usage','correction')",
        )
        .fetch_one(&mut db)
        .await?;
        assert_eq!(Decimal::parse(&balance)?, Decimal::zero());
        assert_eq!(
            append_charge_difference(
                &mut db,
                &p,
                &Decimal::parse("1")?,
                true,
                LedgerKind::Correction,
                "correct-two",
                start
            )
            .await?
            .to_string(),
            "-4"
        );
        let (balance,unpaid):(String,String)=sqlx::query_as("SELECT sum(amount) FILTER(WHERE kind IN ('credit','usage','correction'))::text,sum(amount) FILTER(WHERE kind='postpaid')::text FROM ledger").fetch_one(&mut db).await?;
        assert_eq!(Decimal::parse(&balance)?, Decimal::parse("2")?);
        assert_eq!(Decimal::parse(&unpaid)?, Decimal::zero());
        assert_eq!(
            append_charge_difference(
                &mut db,
                &p,
                &Decimal::parse("1")?,
                true,
                LedgerKind::Correction,
                "correct-two",
                start
            )
            .await?,
            Decimal::zero()
        );
        Ok(())
    }
}
