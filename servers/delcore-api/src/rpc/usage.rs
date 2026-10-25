// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Call, canonical, invalid, page, string, uuid};
use crate::{Op, Result};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;

impl Call<'_> {
    /// Permission, billing and feature evaluation shared by `Authorize` and
    /// `AuthorizeApiKey`, so API keys never fork the billing rules.
    ///
    /// The calling service (`self.service`) selects the capability catalog and
    /// feature namespace. With `user` set, the user must currently hold the
    /// capability in the tenant (owner role or group permission) and must not be
    /// withdrawing. Without a user, only the tenant lifecycle and the service's
    /// declared capability are checked. `strict_lifecycle` additionally blocks a
    /// tenant that is being deleted even for unpaid checks.
    pub(super) async fn authorize_tenant(
        &mut self,
        tenant: &str,
        user: Option<&str>,
        capability: &str,
        paid: bool,
        strict_lifecycle: bool,
        q: &Value,
    ) -> Result<Value> {
        let tenant_id = uuid(tenant)?;
        let feature = self.feature_request(q).await?;
        let allowed: bool = match user {
            Some(user) => sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM members m JOIN tenants t ON t.id=m.tenant_id JOIN users u ON u.id=m.user_id JOIN services s ON s.id=$3 WHERE m.tenant_id=$1 AND m.user_id=$2 AND u.withdrawing_at IS NULL AND t.deleted_at IS NULL AND $4=ANY(s.capabilities) AND (1=ANY(m.roles) OR EXISTS(SELECT 1 FROM group_members gm JOIN group_permissions gp ON gp.group_id=gm.group_id WHERE gm.tenant_id=m.tenant_id AND gm.user_id=m.user_id AND gp.service_id=$3 AND gp.capability=$4)))")
                .bind(tenant_id).bind(uuid(user)?).bind(uuid(&self.service)?).bind(capability).fetch_one(&mut *self.tx).await?,
            None => sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tenants t JOIN services s ON s.id=$2 WHERE t.id=$1 AND t.deleted_at IS NULL AND $3=ANY(s.capabilities))")
                .bind(tenant_id).bind(uuid(&self.service)?).bind(capability).fetch_one(&mut *self.tx).await?,
        };
        if !allowed {
            return Ok(json!({"allowed":false,"reasons":[1]}));
        }
        let mut reasons: Vec<i32> = Vec::new();
        if paid {
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
            let mut depleted: bool = row.try_get(5)?;
            if depleted {
                let start: DateTime<Utc> =
                    sqlx::query_scalar("SELECT period_start FROM tenants WHERE id=$1")
                        .bind(tenant_id)
                        .fetch_one(&mut *self.tx)
                        .await?;
                depleted =
                    !crate::postgres::has_allowance(&mut *self.tx, tenant, &self.service, start)
                        .await?;
            }
            for (blocked, reason) in [
                (row.try_get::<bool, _>(0)?, 2),
                (row.try_get(1)?, 3),
                (row.try_get(2)?, 6),
                (row.try_get(3)?, 4),
                (row.try_get::<bool, _>(4)? || depleted, 5),
                (!row.try_get::<bool, _>(6)?, 1),
            ] {
                if blocked {
                    reasons.push(reason);
                }
            }
        } else if strict_lifecycle {
            let deleting: bool =
                sqlx::query_scalar("SELECT deleting_at IS NOT NULL FROM tenants WHERE id=$1")
                    .bind(tenant_id)
                    .fetch_one(&mut *self.tx)
                    .await?;
            if deleting {
                reasons.push(6);
            }
        }
        let mut out = json!({"allowed":false,"reasons":reasons});
        self.authorize_feature(feature.as_ref(), tenant, &mut out)
            .await?;
        out["allowed"] = json!(out["reasons"].as_array().is_none_or(Vec::is_empty));
        Ok(out)
    }
}

impl Call<'_> {
    pub async fn usage(&mut self, op: Op, q: &Value) -> Result<Value> {
        let tenant = string(q, "tenantId");
        match op {
            Op::Authorize => {
                self.service_scope("authorize").await?;
                if !canonical(tenant) || !canonical(string(q, "userId")) {
                    return Err(invalid("invalid tenant or user ID"));
                }
                self.authorize_tenant(
                    tenant,
                    Some(string(q, "userId")),
                    string(q, "capability"),
                    q["paid"].as_bool().unwrap_or(false),
                    false,
                    q,
                )
                .await
            }
            Op::ListLedger => {
                self.require(tenant, &[3]).await?;
                let (n, after) = page(&q["page"])?;
                let rows = sqlx::query("SELECT id::text,kind,amount::text,meter,created_at FROM ledger WHERE tenant_id=$1 AND id>$2 ORDER BY id LIMIT $3")
                    .bind(uuid(tenant)?).bind(uuid(&after)?).bind(n+1).fetch_all(&mut *self.tx).await?;
                let mut entries = Vec::new();
                for r in rows {
                    entries.push(json!({"id":r.try_get::<String,_>(0)?,"kind":r.try_get::<String,_>(1)?,"amount":r.try_get::<String,_>(2)?,"meter":r.try_get::<String,_>(3)?,"createdAt":r.try_get::<DateTime<Utc>,_>(4)?.to_rfc3339()}));
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
