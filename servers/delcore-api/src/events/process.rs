// SPDX-License-Identifier: FSL-1.1-ALv2

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};

use super::{
    Contribution, JOB_KIND, decode_values, encode_values, group_digest, load_contributions,
    permanent,
};
use crate::{Result, accounting::Decimal, metering};

#[derive(Clone, Debug)]
pub struct MeterChange {
    pub meter_id: String,
    pub change: metering::Change,
}

#[derive(Clone, Debug)]
pub struct Applied {
    pub event_id: String,
    pub tenant_id: String,
    pub service_id: String,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub from_revision: i32,
    pub to_revision: i32,
    pub changes: Vec<MeterChange>,
}

/// Apply the latest accepted revision once. Intermediate queued revisions collapse
/// into the same exact before/after transition.
pub async fn process(tx: &mut PgConnection, now: DateTime<Utc>, event_id: &str) -> Result<Applied> {
    let row = sqlx::query("SELECT tenant_id::text,service_id::text,period_start,period_end,applied_revision,latest_revision FROM metering_events WHERE id=$1::text::uuid FOR UPDATE")
        .bind(event_id).fetch_optional(&mut *tx).await?
        .ok_or_else(|| permanent("event is missing"))?;
    let mut applied = Applied {
        event_id: event_id.to_owned(),
        tenant_id: row.try_get(0)?,
        service_id: row.try_get(1)?,
        period_start: row.try_get(2)?,
        period_end: row.try_get(3)?,
        from_revision: row.try_get(4)?,
        to_revision: row.try_get(5)?,
        changes: Vec::new(),
    };
    if applied.from_revision == applied.to_revision {
        return Ok(applied);
    }
    let old = if applied.from_revision > 0 {
        load_contributions(tx, event_id, applied.from_revision).await?
    } else {
        Vec::new()
    };
    let latest = load_contributions(tx, event_id, applied.to_revision).await?;
    let mut meters: BTreeMap<String, (Option<Contribution>, Option<Contribution>)> =
        BTreeMap::new();
    for c in old {
        let meter_id = c.meter_id.clone();
        meters.entry(meter_id).or_default().0 = Some(c);
    }
    for c in latest {
        let meter_id = c.meter_id.clone();
        meters.entry(meter_id).or_default().1 = Some(c);
    }
    let mut meters: Vec<_> = meters.into_iter().collect();
    meters.sort_by(|(aid, (ao, an)), (bid, (bo, bn))| {
        an.as_ref()
            .or(ao.as_ref())
            .unwrap()
            .meter_key
            .cmp(&bn.as_ref().or(bo.as_ref()).unwrap().meter_key)
            .then(aid.cmp(bid))
    });
    for (id, (old, latest)) in meters {
        type Delta = (
            BTreeMap<String, metering::Value>,
            Vec<metering::Value>,
            Vec<metering::Value>,
        );
        let mut groups: BTreeMap<String, Delta> = BTreeMap::new();
        if let Some(c) = &old {
            groups.insert(
                c.group_key.clone(),
                (c.group_values.clone(), vec![c.value.clone()], Vec::new()),
            );
        }
        if let Some(c) = &latest {
            groups
                .entry(c.group_key.clone())
                .or_insert_with(|| (c.group_values.clone(), Vec::new(), Vec::new()))
                .2
                .push(c.value.clone());
        }
        let reference = latest.as_ref().or(old.as_ref()).unwrap();
        for (key, (values, removes, adds)) in groups {
            let (before, after) =
                apply_group(tx, now, &applied, reference, &key, values, &removes, &adds).await?;
            if match (&before, &after) {
                (None, None) => true,
                (Some(a), Some(b)) => same_aggregate(a, b),
                _ => false,
            } {
                continue;
            }
            applied.changes.push(MeterChange {
                meter_id: id.clone(),
                change: metering::Change {
                    meter_key: reference.meter_key.clone(),
                    group_key: key,
                    before,
                    after,
                },
            });
        }
    }
    sqlx::query("UPDATE metering_events SET applied_revision=$2 WHERE id=$1::text::uuid")
        .bind(event_id)
        .bind(applied.to_revision)
        .execute(&mut *tx)
        .await?;
    Ok(applied)
}

fn same_aggregate(a: &metering::Aggregate, b: &metering::Aggregate) -> bool {
    a.count == b.count
        && a.distinct_count == b.distinct_count
        && a.sum == b.sum
        && a.value == b.value
}

#[allow(clippy::too_many_arguments)]
async fn apply_group(
    tx: &mut PgConnection,
    now: DateTime<Utc>,
    a: &Applied,
    c: &Contribution,
    key: &str,
    mut values: BTreeMap<String, metering::Value>,
    removes: &[metering::Value],
    adds: &[metering::Value],
) -> Result<(Option<metering::Aggregate>, Option<metering::Aggregate>)> {
    use metering::Aggregation::*;
    let kind = c.aggregation;
    let digest = group_digest(key);
    let row = sqlx::query("SELECT group_values,aggregation,event_count,sum::text,distinct_count,value::text FROM metering_aggregates WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4 FOR UPDATE")
        .bind(&a.tenant_id).bind(&c.meter_id).bind(a.period_start).bind(&digest).fetch_optional(&mut *tx).await?;
    let (mut count, mut distinct, mut sum) = (0i64, 0i64, Decimal::zero());
    let before = if let Some(row) = row {
        if row.try_get::<String, _>("aggregation")? != kind.as_str() {
            return Err(permanent("stored aggregation differs from the meter"));
        }
        let value = Decimal::parse(&row.try_get::<String, _>("value")?)
            .map_err(|_| permanent("stored aggregate value"))?;
        sum = Decimal::parse(&row.try_get::<String, _>("sum")?)
            .map_err(|_| permanent("stored aggregate sum"))?;
        values = decode_values(&row.try_get("group_values")?)?;
        count = row.try_get("event_count")?;
        distinct = row.try_get("distinct_count")?;
        Some(metering::Aggregate {
            meter_key: c.meter_key.clone(),
            group_key: key.to_owned(),
            group_values: values.clone(),
            kind,
            count,
            sum: sum.clone(),
            value,
            distinct_count: distinct,
        })
    } else {
        None
    };
    for (items, delta) in [(removes, -1i64), (adds, 1i64)] {
        for v in items {
            count += delta;
            if matches!(kind, Sum | Avg | Min | Max) {
                let mut number = Decimal::parse(&v.text)
                    .map_err(|_| permanent("stored contribution is not numeric"))?;
                if v.kind != metering::ValueKind::Number {
                    return Err(permanent("stored contribution is not numeric"));
                }
                if delta < 0 {
                    number = number.neg();
                }
                sum = sum.add(&number);
            }
            if metering::retains_values(kind) {
                distinct += change_value(tx, a, &c.meter_id, &digest, v, delta).await?;
            }
        }
    }
    if count < 0 || distinct < 0 {
        return Err(permanent("aggregate state would become negative"));
    }
    if count == 0 {
        if distinct != 0 {
            return Err(permanent("empty group retained values"));
        }
        sqlx::query("DELETE FROM metering_aggregates WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4")
            .bind(&a.tenant_id).bind(&c.meter_id).bind(a.period_start).bind(&digest).execute(&mut *tx).await?;
        return Ok((before, None));
    }
    let value = match kind {
        Count => Decimal::from_minor(count.into(), 0),
        Sum => sum.clone(),
        Avg => metering::rounded_average(&sum, count),
        UniqueCount => Decimal::from_minor(distinct.into(), 0),
        Min | Max => {
            let function = if kind == Max { "max" } else { "min" };
            let text: Option<String> = sqlx::query_scalar(&format!("SELECT {function}(numeric_value)::text FROM metering_aggregate_values WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4 AND value_kind='number'"))
                .bind(&a.tenant_id).bind(&c.meter_id).bind(a.period_start).bind(&digest).fetch_one(&mut *tx).await?;
            Decimal::parse(&text.ok_or_else(|| permanent("extremum has no retained values"))?)
                .map_err(|_| permanent("stored extremum"))?
        }
    };
    sqlx::query("INSERT INTO metering_aggregates(tenant_id,meter_id,period_start,period_end,group_key,group_digest,group_values,aggregation,event_count,sum,distinct_count,value,updated_at) VALUES($1::text::uuid,$2::text::uuid,$3,$4,$5,$6,$7,$8,$9,$10::text::numeric,$11,$12::text::numeric,$13) ON CONFLICT(tenant_id,meter_id,period_start,group_digest) DO UPDATE SET event_count=EXCLUDED.event_count,sum=EXCLUDED.sum,distinct_count=EXCLUDED.distinct_count,value=EXCLUDED.value,updated_at=EXCLUDED.updated_at")
        .bind(&a.tenant_id).bind(&c.meter_id).bind(a.period_start).bind(a.period_end).bind(key).bind(&digest).bind(encode_values(&values)?).bind(kind.as_str()).bind(count).bind(sum.to_string()).bind(distinct).bind(value.to_string()).bind(now).execute(&mut *tx).await?;
    let after = metering::Aggregate {
        meter_key: c.meter_key.clone(),
        group_key: key.to_owned(),
        group_values: values,
        kind,
        count,
        sum: if matches!(kind, Count | UniqueCount) {
            Decimal::zero()
        } else {
            sum
        },
        value,
        distinct_count: if metering::retains_values(kind) {
            distinct
        } else {
            0
        },
    };
    Ok((before, Some(after)))
}

async fn change_value(
    tx: &mut PgConnection,
    a: &Applied,
    meter_id: &str,
    digest: &[u8],
    v: &metering::Value,
    delta: i64,
) -> Result<i64> {
    if delta > 0 {
        let numeric = if v.kind == metering::ValueKind::Number {
            Some(v.text.as_str())
        } else {
            None
        };
        let events: i64 = sqlx::query_scalar("INSERT INTO metering_aggregate_values(tenant_id,meter_id,period_start,group_digest,value_kind,value,numeric_value,events) VALUES($1::text::uuid,$2::text::uuid,$3,$4,$5,$6,$7::text::numeric,1) ON CONFLICT(tenant_id,meter_id,period_start,group_digest,value_kind,value) DO UPDATE SET events=metering_aggregate_values.events+1 RETURNING events")
            .bind(&a.tenant_id).bind(meter_id).bind(a.period_start).bind(digest).bind(v.kind.as_str()).bind(&v.text).bind(numeric).fetch_one(&mut *tx).await?;
        return Ok(i64::from(events == 1));
    }
    let events: i64 = sqlx::query_scalar("SELECT events FROM metering_aggregate_values WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4 AND value_kind=$5 AND value=$6 FOR UPDATE")
        .bind(&a.tenant_id).bind(meter_id).bind(a.period_start).bind(digest).bind(v.kind.as_str()).bind(&v.text).fetch_optional(&mut *tx).await?.ok_or_else(|| permanent("retained value is missing"))?;
    let statement = if events == 1 {
        "DELETE FROM metering_aggregate_values WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4 AND value_kind=$5 AND value=$6"
    } else {
        "UPDATE metering_aggregate_values SET events=events-1 WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4 AND value_kind=$5 AND value=$6"
    };
    sqlx::query(statement)
        .bind(&a.tenant_id)
        .bind(meter_id)
        .bind(a.period_start)
        .bind(digest)
        .bind(v.kind.as_str())
        .bind(&v.text)
        .execute(&mut *tx)
        .await?;
    Ok(if events == 1 { -1 } else { 0 })
}

/// Clear raw attributes only after deletion and all metering jobs finish.
/// The caller must enable controlled retention mode in this transaction.
pub async fn remove_attributes(tx: &mut PgConnection, cutoff: DateTime<Utc>) -> Result<u64> {
    Ok(sqlx::query("UPDATE metering_event_revisions r SET attributes='{}'::jsonb WHERE r.attributes<>'{}'::jsonb AND r.tenant_id IN (SELECT t.id FROM tenants t WHERE t.deleted_at<$1 AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.tenant_id=t.id AND j.kind=$2 AND j.state<>'done'))")
        .bind(cutoff).bind(JOB_KIND).execute(&mut *tx).await?.rows_affected())
}

/// Preserve replay identities before removing deleted tenants' metering history.
/// The caller must enable controlled retention mode in this transaction.
pub async fn retain(tx: &mut PgConnection, cutoff: DateTime<Utc>) -> Result<usize> {
    let tenants: Vec<String> = sqlx::query_scalar("SELECT t.id::text FROM tenants t WHERE t.deleted_at<$1 AND EXISTS(SELECT 1 FROM metering_events e WHERE e.tenant_id=t.id) AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.tenant_id=t.id AND j.kind=$2 AND j.state<>'done')")
        .bind(cutoff).bind(JOB_KIND).fetch_all(&mut *tx).await?;
    if tenants.is_empty() {
        return Ok(0);
    }
    for statement in [
        "INSERT INTO metering_replay(service_id,event_key,event_id,fingerprint) SELECT service_id,event_key,id,fingerprint FROM metering_events WHERE tenant_id=ANY($1::text[]::uuid[]) ON CONFLICT DO NOTHING",
        "DELETE FROM metering_aggregate_values WHERE tenant_id=ANY($1::text[]::uuid[])",
        "DELETE FROM metering_aggregates WHERE tenant_id=ANY($1::text[]::uuid[])",
        "DELETE FROM metering_admission_values WHERE tenant_id=ANY($1::text[]::uuid[])",
        "DELETE FROM metering_admission_groups WHERE tenant_id=ANY($1::text[]::uuid[])",
        "DELETE FROM metering_admission_periods WHERE tenant_id=ANY($1::text[]::uuid[])",
        "DELETE FROM metering_rated_periods WHERE tenant_id=ANY($1::text[]::uuid[])",
        "DELETE FROM metering_event_prices WHERE tenant_id=ANY($1::text[]::uuid[])",
        "DELETE FROM metering_contributions WHERE tenant_id=ANY($1::text[]::uuid[])",
        "DELETE FROM metering_event_revisions WHERE tenant_id=ANY($1::text[]::uuid[])",
        "DELETE FROM metering_events WHERE tenant_id=ANY($1::text[]::uuid[])",
    ] {
        sqlx::query(statement)
            .bind(&tenants)
            .execute(&mut *tx)
            .await?;
    }
    Ok(tenants.len())
}
