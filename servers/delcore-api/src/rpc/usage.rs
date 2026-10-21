// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Call, canonical, invalid, page, precondition, string, uuid};
use crate::{Op, Result};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;

impl Call<'_> {
    pub async fn usage(&mut self, op: Op, q: &Value) -> Result<Value> {
        let tenant = string(q, "tenantId");
        match op {
            Op::Authorize => {
                self.service_scope("authorize").await?;
                let mode = self.consumer_mode(q).await?;
                // A tenant service key acts for the tenant, so it names no user.
                let user_free = mode == super::consumer::Mode::TenantServiceKey;
                if !canonical(tenant) || (!user_free && !canonical(string(q, "userId"))) {
                    return Err(invalid("invalid tenant or user ID"));
                }
                let tenant_id = uuid(tenant)?;
                let feature = self.feature_request(q).await?;
                let allowed: bool = if mode == super::consumer::Mode::User {
                    let user_id = uuid(string(q, "userId"))?;
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM members m JOIN tenants t ON t.id=m.tenant_id JOIN users u ON u.id=m.user_id JOIN services s ON s.id=$3 WHERE m.tenant_id=$1 AND m.user_id=$2 AND u.withdrawing_at IS NULL AND t.deleted_at IS NULL AND $4=ANY(s.capabilities) AND (1=ANY(m.roles) OR EXISTS(SELECT 1 FROM group_members gm JOIN group_permissions gp ON gp.group_id=gm.group_id WHERE gm.tenant_id=m.tenant_id AND gm.user_id=m.user_id AND gp.service_id=$3 AND gp.capability=$4)))")
                        .bind(tenant_id).bind(user_id).bind(uuid(&self.service)?).bind(string(q,"capability")).fetch_one(&mut *self.tx).await?
                } else {
                    self.consumer_allowed(mode, q).await?
                };
                tracing::debug!(category="authorization",correlation_id=%self.correlation,service_id=%self.service,mode=mode.as_str(),allowed,"authorization evaluated");
                if !allowed {
                    return Ok(json!({"allowed":false,"reasons":[1]}));
                }
                let mut reasons: Vec<i32> = Vec::new();
                if q["paid"].as_bool().unwrap_or(false) {
                    let sql = format!(
                        "SELECT t.operator_block,(t.renewal_failed OR {}),t.deleting_at IS NOT NULL, EXISTS(SELECT 1 FROM disputes WHERE tenant_id=t.id AND active), COALESCE((SELECT sum(amount) FROM ledger WHERE tenant_id=t.id AND kind IN ('credit','usage','refund','correction')),0)<0, (NOT {} AND COALESCE((SELECT sum(amount) FROM ledger WHERE tenant_id=t.id AND kind IN ('credit','usage','refund','correction')),0)<=0), EXISTS(SELECT 1 FROM entitlements en JOIN prices pr ON pr.id=en.price_id WHERE en.tenant_id=t.id AND pr.service_id=$2 AND en.starts_at<=$3 AND (en.ends_at IS NULL OR en.ends_at>$3)) FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1",
                        crate::postgres::period_expired_sql("t", "p", "$3"),
                        crate::postgres::selected_plan_bool_sql("t", "p", "postpaid")
                    );
                    let row = sqlx::query(&sql)
                        .bind(tenant_id)
                        .bind(uuid(&self.service)?)
                        .bind(self.now)
                        .fetch_one(&mut *self.tx)
                        .await?;
                    // Call-time priced services bill every tenant from the report's own
                    // prices, so no subscription price entitlement is required.
                    let call_time = crate::delitools::is_delitools(self.tx, &self.service).await?;
                    let mut depleted: bool = row.try_get(5)?;
                    // Included usage of the service's other meters is never consumed by
                    // call-time usage, so it cannot keep a depleted balance authorized.
                    if depleted && !call_time {
                        let start: DateTime<Utc> =
                            sqlx::query_scalar("SELECT period_start FROM tenants WHERE id=$1")
                                .bind(tenant_id)
                                .fetch_one(&mut *self.tx)
                                .await?;
                        depleted = !crate::postgres::has_allowance(
                            &mut *self.tx,
                            tenant,
                            &self.service,
                            start,
                        )
                        .await?;
                    }
                    for (blocked, reason) in [
                        (row.try_get::<bool, _>(0)?, 2),
                        (row.try_get(1)?, 3),
                        (row.try_get(2)?, 6),
                        (row.try_get(3)?, 4),
                        (row.try_get::<bool, _>(4)? || depleted, 5),
                        (!call_time && !row.try_get::<bool, _>(6)?, 1),
                    ] {
                        if blocked {
                            reasons.push(reason);
                        }
                    }
                }
                let mut out = json!({"allowed":false,"reasons":reasons});
                self.authorize_feature(feature.as_ref(), tenant, &mut out)
                    .await?;
                out["allowed"] = json!(out["reasons"].as_array().is_none_or(Vec::is_empty));
                Ok(out)
            }
            Op::ReportUsage | Op::CorrectUsage => {
                self.service_scope("usage:write").await?;
                tracing::warn!(category="usage_reporting",correlation_id=%self.correlation,service_id=%self.service,"retired usage call rejected");
                Err(precondition(if matches!(op, Op::ReportUsage) {
                    "ReportUsage is retired; report measured events with IngestEvent"
                } else {
                    "CorrectUsage is retired; correct accepted events with CorrectEvent"
                }))
            }
            Op::ListUsage => {
                if !self.service.is_empty() {
                    self.service_scope("usage:read").await?;
                    if !canonical(tenant) {
                        return Err(invalid("invalid tenant ID"));
                    }
                } else {
                    self.require(tenant, &[3]).await?;
                }
                let (n, after) = page(&q["page"])?;
                let rows = sqlx::query("SELECT id::text,tenant_id::text,service_id::text,event_key,meter,quantity::text,occurred_at,COALESCE(correction_of::text,'') FROM usage_events WHERE tenant_id=$1 AND id>$2 AND ($4='' OR service_id::text=$4) ORDER BY id LIMIT $3")
                    .bind(uuid(tenant)?).bind(uuid(&after)?).bind(n+1).bind(&self.service).fetch_all(&mut *self.tx).await?;
                let mut events = Vec::new();
                for r in rows {
                    events.push(json!({"id":r.try_get::<String,_>(0)?,"tenantId":r.try_get::<String,_>(1)?,"serviceId":r.try_get::<String,_>(2)?,"eventKey":r.try_get::<String,_>(3)?,"meter":r.try_get::<String,_>(4)?,"quantity":r.try_get::<String,_>(5)?,"occurredAt":r.try_get::<DateTime<Utc>,_>(6)?.to_rfc3339(),"correctionOf":r.try_get::<String,_>(7)?}));
                }
                let next = if events.len() > n as usize {
                    events.truncate(n as usize);
                    string(&events[n as usize - 1], "id").to_owned()
                } else {
                    String::new()
                };
                Ok(json!({"events":events,"nextAfterId":next}))
            }
            Op::ListLedger => {
                self.require(tenant, &[3]).await?;
                let (n, after) = page(&q["page"])?;
                let rows = sqlx::query("SELECT id::text,kind,amount::text,meter,created_at,COALESCE(usage_id::text,'') FROM ledger WHERE tenant_id=$1 AND id>$2 ORDER BY id LIMIT $3")
                    .bind(uuid(tenant)?).bind(uuid(&after)?).bind(n+1).fetch_all(&mut *self.tx).await?;
                let mut entries = Vec::new();
                for r in rows {
                    entries.push(json!({"id":r.try_get::<String,_>(0)?,"kind":r.try_get::<String,_>(1)?,"amount":r.try_get::<String,_>(2)?,"meter":r.try_get::<String,_>(3)?,"createdAt":r.try_get::<DateTime<Utc>,_>(4)?.to_rfc3339(),"usageId":r.try_get::<String,_>(5)?}));
                }
                let next = if entries.len() > n as usize {
                    entries.truncate(n as usize);
                    string(&entries[n as usize - 1], "id").to_owned()
                } else {
                    String::new()
                };
                Ok(json!({"entries":entries,"nextAfterId":next}))
            }
            Op::CheckTenantLifecycle => {
                self.service_scope("authorize").await?;
                let values = q["tenantIds"]
                    .as_array()
                    .ok_or_else(|| invalid("supply 1 to 100 known tenant IDs"))?;
                if values.is_empty() || values.len() > 100 {
                    return Err(invalid("supply 1 to 100 known tenant IDs"));
                }
                let ids = values
                    .iter()
                    .map(|v| {
                        let value = v.as_str().unwrap_or("");
                        if !canonical(value) {
                            return Err(invalid("invalid tenant ID"));
                        }
                        uuid(value)
                    })
                    .collect::<Result<Vec<_>>>()?;
                let finalized:Vec<String> = sqlx::query_scalar("SELECT id::text FROM tenants WHERE id=ANY($1::uuid[]) AND deleted_at IS NOT NULL ORDER BY id").bind(ids).fetch_all(&mut *self.tx).await?;
                Ok(json!({"finalizedTenantIds":finalized}))
            }
            _ => Err(invalid("unknown usage operation")),
        }
    }
}
