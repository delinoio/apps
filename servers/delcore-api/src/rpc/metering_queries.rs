// SPDX-License-Identifier: FSL-1.1-ALv2

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;

use super::{Call, canonical, forbidden, invalid, string, uuid};
use crate::{Fault, Op, Result, accounting::Decimal};

// Digests keep cursors bounded even when group keys exceed the B-tree entry limit.
fn aggregate_page(q: &Value) -> Result<(i64, Vec<u8>)> {
    let page = &q["page"];
    let size = server_common::integer(page, "size");
    let size = if size == 0 { 50 } else { size };
    if !(1..=100).contains(&size) {
        return Err(invalid("page size must be 1 to 100"));
    }
    let cursor = string(page, "afterId");
    if cursor.is_empty() {
        return Ok((size, Vec::new()));
    }
    let digest = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid("invalid cursor"))?;
    if digest.len() != 32 {
        return Err(invalid("invalid cursor"));
    }
    Ok((size, digest))
}

const AGGREGATE_PAGE: &str = "SELECT group_digest,group_values,value::text,event_count,sum::text,distinct_count,updated_at FROM metering_aggregates WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest>$4 ORDER BY group_digest LIMIT $5";

// Guard the UUID cast: malformed jobs still count for operators without joining an event.
const UNFINISHED: &str = "FROM jobs j LEFT JOIN metering_events e ON e.id=CASE WHEN j.payload->>'event_id' ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' THEN (j.payload->>'event_id')::uuid END WHERE j.kind='metering' AND j.state<>'done' AND ($1::uuid IS NULL OR j.tenant_id=$1) AND ($2::uuid IS NULL OR e.service_id=$2)";

fn failure_category(category: &str) -> usize {
    match category {
        "metering_transient" => 1,
        "retry_exhausted" => 2,
        "metering_invalid" => 3,
        "invalid_payload" => 4,
        _ => 0,
    }
}

impl Call<'_> {
    pub async fn metering_queries(&mut self, op: Op, q: &Value) -> Result<Value> {
        match op {
            Op::ListMeterAggregates => self.list_meter_aggregates(q).await,
            Op::GetMeteringStatus => self.get_metering_status(q).await,
            _ => Err(invalid("unsupported metering query")),
        }
    }

    async fn list_meter_aggregates(&mut self, q: &Value) -> Result<Value> {
        let tenant = string(q, "tenantId");
        let mut service = string(q, "serviceId").to_owned();
        if !self.service.is_empty() {
            self.service_scope("usage:read").await?;
            if !canonical(tenant) {
                return Err(invalid("invalid tenant ID"));
            }
            if service.is_empty() {
                service.clone_from(&self.service);
            }
            if service != self.service {
                return Err(forbidden());
            }
        } else {
            self.require(tenant, &[3]).await?;
            if !canonical(&service) {
                return Err(invalid("invalid service ID"));
            }
        }
        let key = string(q, "meterKey");
        if key.is_empty()
            || key.len() > 100
            || !key.as_bytes()[0].is_ascii_lowercase()
            || !key
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            return Err(invalid("invalid meter key"));
        }
        let (size, after) = aggregate_page(q)?;
        let at = if q["periodAt"].is_null() {
            self.now
        } else {
            DateTime::parse_from_rfc3339(string(q, "periodAt"))
                .map_err(|_| invalid("invalid period instant"))?
                .with_timezone(&Utc)
        };
        let meter = sqlx::query("SELECT ce.id,cv.definition FROM catalog_entities ce LEFT JOIN catalog_versions cv ON cv.id=ce.active_version_id WHERE ce.kind='meter' AND ce.scope=$1 AND ce.key=$2")
            .bind(&service).bind(key).fetch_optional(&mut *self.tx).await?
            .ok_or_else(|| invalid("unknown meter"))?;
        let meter_id: uuid::Uuid = meter.try_get("id")?;
        let definition: Option<Value> = meter.try_get("definition")?;
        let definition = crate::features::decode_meter(definition.as_ref())?;
        // Legacy untyped meters use the compatibility SUM definition at ingestion.
        let aggregation = if definition.event_type.is_empty() {
            2
        } else {
            definition.aggregation
        };
        let (start, end) = crate::postgres::period::billing_period(&mut *self.tx, tenant, at)
            .await
            .map_err(|e| {
                if e.is::<crate::postgres::period::PeriodUnavailable>() {
                    invalid("billing period unavailable")
                } else {
                    Fault::from(e)
                }
            })?;
        let rows = sqlx::query(AGGREGATE_PAGE)
            .bind(uuid(tenant)?)
            .bind(meter_id)
            .bind(start)
            .bind(after)
            .bind(size + 1)
            .fetch_all(&mut *self.tx)
            .await?;
        let mut aggregates = Vec::new();
        let mut next = String::new();
        for row in rows.iter().take(size as usize) {
            let values: Value = row.try_get("group_values")?;
            let values = crate::events::decode_values(&values)?;
            let value: String = row.try_get("value")?;
            let sum: String = row.try_get("sum")?;
            let value = Decimal::parse(&value)
                .map_err(|e| Fault::from(anyhow::Error::new(e)))?
                .to_string();
            let sum = Decimal::parse(&sum)
                .map_err(|e| Fault::from(anyhow::Error::new(e)))?
                .to_string();
            let count: i64 = row.try_get("event_count")?;
            let distinct: i64 = row.try_get("distinct_count")?;
            let updated: DateTime<Utc> = row.try_get("updated_at")?;
            aggregates.push(json!({"groupValues":super::events::wire_attributes(&values),"value":value,"sum":sum,"eventCount":count.to_string(),"distinctCount":distinct.to_string(),"updatedAt":updated}));
        }
        if rows.len() > size as usize {
            let digest: Vec<u8> = rows[size as usize - 1].try_get("group_digest")?;
            next = URL_SAFE_NO_PAD.encode(digest);
        }
        tracing::info!(category="metering_query",correlation_id=%self.correlation,service_id=%self.service,meter_key=key,aggregates=aggregates.len(),"meter aggregates listed");
        Ok(
            json!({"meterId":meter_id,"aggregation":aggregation,"periodStart":start,"periodEnd":end,"aggregates":aggregates,"nextAfterId":next}),
        )
    }

    async fn get_metering_status(&mut self, q: &Value) -> Result<Value> {
        let tenant = string(q, "tenantId");
        let mut service = None;
        let mut scope = "tenant";
        let tenant = if !self.service.is_empty() {
            self.service_scope("usage:read").await?;
            service = Some(uuid(&self.service)?);
            if !canonical(tenant) {
                return Err(invalid("invalid tenant ID"));
            }
            Some(uuid(tenant)?)
        } else if tenant.is_empty() {
            if !self.identity.operator {
                return Err(forbidden());
            }
            scope = "service_wide";
            None
        } else if self.identity.operator {
            if !canonical(tenant) {
                return Err(invalid("invalid tenant ID"));
            }
            Some(uuid(tenant)?)
        } else {
            self.require(tenant, &[3]).await?;
            if !canonical(tenant) {
                return Err(invalid("invalid tenant ID"));
            }
            Some(uuid(tenant)?)
        };
        let sql = format!(
            "SELECT count(DISTINCT e.id) FILTER(WHERE e.applied_revision<e.latest_revision) AS unapplied, count(*) FILTER(WHERE j.state IN ('pending','running')) AS pending,min(j.created_at) FILTER(WHERE j.state IN ('pending','running')) AS oldest,count(*) FILTER(WHERE j.state='failed') AS failed {UNFINISHED}"
        );
        let row = sqlx::query(&sql)
            .bind(tenant)
            .bind(service)
            .fetch_one(&mut *self.tx)
            .await?;
        let unapplied: i64 = row.try_get("unapplied")?;
        let pending: i64 = row.try_get("pending")?;
        let failed: i64 = row.try_get("failed")?;
        let oldest: Option<DateTime<Utc>> = row.try_get("oldest")?;
        let lag = oldest.map_or(0, |at| (self.now - at).num_seconds().max(0));
        // failed_at survives manual retry, unlike updated_at.
        let sql = format!(
            "SELECT j.error_category,count(*) AS jobs,max(j.failed_at) AS latest {UNFINISHED} AND j.error_category<>'' GROUP BY j.error_category ORDER BY j.error_category"
        );
        let rows = sqlx::query(&sql)
            .bind(tenant)
            .bind(service)
            .fetch_all(&mut *self.tx)
            .await?;
        let mut counts = [0_i64; 5];
        let mut latest = None;
        let mut last_category = 0;
        for row in rows {
            let category: String = row.try_get("error_category")?;
            let category = failure_category(&category);
            counts[category] += row.try_get::<i64, _>("jobs")?;
            let at: Option<DateTime<Utc>> = row.try_get("latest")?;
            if at.is_some() && at > latest {
                latest = at;
                last_category = category;
            }
        }
        let failures: Vec<Value> = counts
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(category, n)| json!({"category":category,"jobs":n.to_string()}))
            .collect();
        let mut out = json!({"unappliedEvents":unapplied.to_string(),"pendingJobs":pending.to_string(),"failedJobs":failed.to_string(),"lagSeconds":lag.to_string(),"failures":failures,"lastFailureCategory":last_category});
        if let Some(at) = oldest {
            out["oldestPendingAt"] = json!(at);
        }
        if let Some(at) = latest {
            out["lastFailureAt"] = json!(at);
        }
        tracing::info!(category="metering_query",correlation_id=%self.correlation,scope,service_id=%self.service,pending_jobs=pending,failed_jobs=failed,lag_seconds=lag,"metering status read");
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aggregate_cursor_is_one_sha256_digest() {
        assert_eq!(aggregate_page(&json!({})).unwrap(), (50, vec![]));
        let digest = vec![255; 32];
        assert_eq!(
            aggregate_page(&json!({"page":{"afterId":URL_SAFE_NO_PAD.encode(&digest)}})).unwrap(),
            (50, digest)
        );
        for cursor in [
            "!!".to_owned(),
            "AA".to_owned(),
            URL_SAFE_NO_PAD.encode([0; 31]),
            URL_SAFE_NO_PAD.encode([0; 33]),
        ] {
            assert!(aggregate_page(&json!({"page":{"afterId":cursor}})).is_err());
        }
        for size in [-1, 101] {
            assert!(aggregate_page(&json!({"page":{"size":size}})).is_err());
        }
    }
    #[test]
    fn bounded_failure_categories_include_unknown() {
        assert_eq!(failure_category("metering_transient"), 1);
        assert_eq!(failure_category("retry_exhausted"), 2);
        assert_eq!(failure_category("metering_invalid"), 3);
        assert_eq!(failure_category("invalid_payload"), 4);
        assert_eq!(failure_category("other"), 0);
    }
}
