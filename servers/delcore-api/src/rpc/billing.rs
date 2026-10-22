// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Call, forbidden, id, invalid, page, precondition, string, uuid};
use crate::{
    Op,
    accounting::{Decimal, USD, USD_MINOR_UNITS},
    postgres,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};

// A rejected charge remains attempted even after an operator resets its counter.
const UNATTEMPTED_AUTO_TOPUP: &str = "kind='auto_topup' AND state='pending' AND attempts=0 AND external_id='' AND error_category IN ('','provider_billing_window')";

impl Call<'_> {
    pub async fn billing(&mut self, op: Op, q: &Value) -> crate::Result<Value> {
        let tenant = string(q, "tenantId");
        match op {
            Op::RequestTopUp => {
                self.require(tenant, &[3]).await?;
                let amount = string(q, "amount");
                let d = Decimal::parse(amount)
                    .map_err(|_| invalid("invalid top-up amount or return URL"))?;
                if amount.len() > 64
                    || d <= Decimal::zero()
                    || !d.is_minor_exact(USD_MINOR_UNITS)
                    || !self.config.allows_return(string(q, "returnUrl"))
                {
                    return Err(invalid("invalid top-up amount or return URL"));
                }
                let units = d
                    .minor_units(USD_MINOR_UNITS)
                    .map_err(anyhow::Error::from)?;
                let row = sqlx::query("SELECT deleting_at IS NOT NULL AS deleting,COALESCE((SELECT sum(amount)::text FROM ledger WHERE tenant_id=t.id AND kind IN ('credit','usage','refund','correction')),'0') AS credit_balance FROM tenants t WHERE id=$1 AND deleted_at IS NULL").bind(uuid(tenant)?).fetch_one(&mut *self.tx).await?;
                if row.try_get::<bool, _>("deleting")?
                    && Decimal::parse(&row.try_get::<String, _>("credit_balance")?)
                        .map(|d| d >= Decimal::zero())
                        .unwrap_or(true)
                {
                    return Err(precondition(
                        "deleting tenants can only top up to recover credit debt",
                    ));
                }
                let job = self.job(tenant,"topup",&format!("request:{}",string(q,"requestKey")), &json!({"amount":d.to_string(),"minor_units":units.to_string(),"currency":USD,"return_url":string(q,"returnUrl")})).await?;
                Ok(json!({"jobId":job}))
            }
            Op::SetAutoTopUp => {
                self.require(tenant, &[3]).await?;
                if !q["enabled"].as_bool().unwrap_or(false) {
                    sqlx::query("UPDATE tenants SET auto_threshold=NULL,auto_amount=NULL,auto_cap=NULL WHERE id=$1").bind(uuid(tenant)?).execute(&mut *self.tx).await?;
                    // The shared transaction lock serializes clearing settings and stopping
                    // queued work with the maintenance pass that creates charges.
                    let stopped = sqlx::query(&format!("UPDATE jobs SET state='done',error_category='auto_topup_disabled',updated_at=$2 WHERE tenant_id=$1 AND {UNATTEMPTED_AUTO_TOPUP}")).bind(uuid(tenant)?).bind(self.now).execute(&mut *self.tx).await?.rows_affected();
                    tracing::info!(
                        tenant_id = tenant,
                        stopped_jobs = stopped,
                        "automatic top-up disabled"
                    );
                    return Ok(json!({}));
                }
                let parse = |key| {
                    Decimal::parse(string(q, key))
                        .map_err(|_| invalid("invalid automatic top-up settings"))
                };
                let (threshold, amount, cap) =
                    (parse("threshold")?, parse("amount")?, parse("monthlyCap")?);
                if ["threshold", "amount", "monthlyCap"]
                    .iter()
                    .any(|k| string(q, k).len() > 64)
                    || threshold < Decimal::zero()
                    || amount <= Decimal::zero()
                    || cap < amount
                    || !amount.is_minor_exact(USD_MINOR_UNITS)
                {
                    return Err(invalid("invalid automatic top-up settings"));
                }
                let n=sqlx::query("UPDATE tenants SET auto_threshold=$2::text::numeric,auto_amount=$3::text::numeric,auto_cap=$4::text::numeric,auto_suspended=false WHERE id=$1 AND deleting_at IS NULL AND external_subscription_id<>'' AND NOT renewal_failed AND period_end>$5 AND NOT EXISTS(SELECT 1 FROM jobs WHERE tenant_id=tenants.id AND kind='cancel' AND state IN ('pending','running','reconcile'))").bind(uuid(tenant)?).bind(threshold.to_string()).bind(amount.to_string()).bind(cap.to_string()).bind(self.now).execute(&mut *self.tx).await?.rows_affected();
                if n != 1 {
                    return Err(precondition(
                        "automatic top-ups require an active Paddle subscription",
                    ));
                }
                self.audit(tenant, "billing.auto_topup", "").await?;
                Ok(json!({}))
            }
            Op::ChangeSubscription => self.change_subscription(q).await,
            Op::CancelSubscription => {
                self.require(tenant, &[3]).await?;
                let (subscription,pending):(String,bool)=sqlx::query_as("SELECT external_subscription_id,EXISTS(SELECT 1 FROM jobs WHERE tenant_id=t.id AND kind IN ('subscription','cancel') AND state IN ('pending','running','reconcile')) FROM tenants t WHERE id=$1").bind(uuid(tenant)?).fetch_one(&mut *self.tx).await?;
                if subscription.is_empty() || pending {
                    return Err(precondition(
                        "an active subscription without a pending change is required",
                    ));
                }
                sqlx::query("UPDATE tenants SET auto_suspended=true WHERE id=$1")
                    .bind(uuid(tenant)?)
                    .execute(&mut *self.tx)
                    .await?;
                sqlx::query(&format!("UPDATE jobs SET state='done',error_category='subscription_canceled',updated_at=$2 WHERE tenant_id=$1 AND {UNATTEMPTED_AUTO_TOPUP}")).bind(uuid(tenant)?).bind(self.now).execute(&mut *self.tx).await?;
                let job = self
                    .job(
                        tenant,
                        "cancel",
                        &format!("request:{}", string(q, "requestKey")),
                        &json!({"effective_from":"next_billing_period"}),
                    )
                    .await?;
                Ok(json!({"jobId":job}))
            }
            Op::DeleteTenant => {
                self.require(tenant, &[]).await?;
                self.stop_pending_subscriptions(tenant, "").await?;
                sqlx::query("UPDATE tenants SET deleting_at=COALESCE(deleting_at,$2),auto_suspended=true WHERE id=$1").bind(uuid(tenant)?).bind(self.now).execute(&mut *self.tx).await?;
                self.queue_deletion_cancel(tenant).await?;
                self.audit(tenant, "tenant.delete", "").await?;
                Ok(json!({}))
            }
            Op::Withdraw => {
                if self.user.is_empty() {
                    return Err(forbidden());
                }
                let user = self.user.clone();
                let owns:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tenants WHERE owner_id=$1 AND NOT personal AND deleted_at IS NULL)").bind(uuid(&user)?).fetch_one(&mut *self.tx).await?;
                if owns {
                    return Err(precondition("transfer or delete owned organizations first"));
                }
                self.stop_pending_subscriptions("", &user).await?;
                sqlx::query("UPDATE users SET withdrawing_at=$2 WHERE id=$1")
                    .bind(uuid(&user)?)
                    .bind(self.now)
                    .execute(&mut *self.tx)
                    .await?;
                sqlx::query("UPDATE tenants SET deleting_at=COALESCE(deleting_at,$2),auto_suspended=true WHERE owner_id=$1 AND personal").bind(uuid(&user)?).bind(self.now).execute(&mut *self.tx).await?;
                let tenants:Vec<String>=sqlx::query_scalar("SELECT id::text FROM tenants WHERE owner_id=$1 AND personal AND external_subscription_id<>'' AND NOT EXISTS(SELECT 1 FROM jobs WHERE tenant_id=tenants.id AND kind='cancel' AND state IN ('pending','running','reconcile'))").bind(uuid(&user)?).fetch_all(&mut *self.tx).await?;
                for tenant in tenants {
                    self.queue_deletion_cancel(&tenant).await?;
                }
                sqlx::query("DELETE FROM members WHERE user_id=$1 AND tenant_id IN (SELECT id FROM tenants WHERE NOT personal)").bind(uuid(&user)?).execute(&mut *self.tx).await?;
                self.audit("", "user.withdraw", "").await?;
                Ok(json!({}))
            }
            Op::SetTenantBlocked => {
                self.operator(string(q, "reason"))?;
                let n = sqlx::query(
                    "UPDATE tenants SET operator_block=$2 WHERE id=$1 AND deleted_at IS NULL",
                )
                .bind(uuid(tenant)?)
                .bind(q["blocked"].as_bool().unwrap_or(false))
                .execute(&mut *self.tx)
                .await?
                .rows_affected();
                if n != 1 {
                    return Err(precondition("tenant unavailable"));
                }
                self.audit(tenant, "tenant.block", string(q, "reason"))
                    .await?;
                Ok(json!({}))
            }
            Op::ListJobs | Op::ListAudit => self.list_billing_records(op, q).await,
            Op::RetryJob => {
                self.operator(string(q, "reason"))?;
                // An unknown payment outcome must reconcile before another charge.
                let row=sqlx::query("UPDATE jobs SET state=CASE WHEN kind IN ('email','webhook','metering') OR error_category='provider_rejected' THEN 'pending' ELSE 'reconcile' END,attempts=0,available_at=$2,updated_at=$2 WHERE id=$1 AND state='failed' RETURNING kind,error_category").bind(uuid(string(q,"jobId"))?).bind(self.now).fetch_optional(&mut *self.tx).await?.ok_or_else(||precondition("job is not failed"))?;
                self.audit("", "job.retry", string(q, "reason")).await?;
                let kind: String = row.try_get("kind")?;
                tracing::info!(
                    job_id = string(q, "jobId"),
                    kind,
                    failure_category = row.try_get::<String, _>("error_category")?,
                    "failed job returned to processing"
                );
                Ok(json!({"_retryKind":kind}))
            }
            _ => Err(invalid("unsupported operation")),
        }
    }

    async fn queue_deletion_cancel(&mut self, tenant: &str) -> crate::Result<()> {
        let needed:bool=sqlx::query_scalar("SELECT external_subscription_id<>'' AND NOT EXISTS(SELECT 1 FROM jobs WHERE tenant_id=t.id AND kind='cancel' AND state IN ('pending','running','reconcile')) FROM tenants t WHERE id=$1").bind(uuid(tenant)?).fetch_one(&mut *self.tx).await?;
        if needed {
            self.job(
                tenant,
                "cancel",
                &format!("deletion_cancel:{tenant}"),
                &json!({"effective_from":"next_billing_period"}),
            )
            .await?;
        }
        Ok(())
    }
    async fn stop_pending_subscriptions(&mut self, tenant: &str, owner: &str) -> crate::Result<()> {
        let uncertain:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs j JOIN tenants t ON t.id=j.tenant_id WHERE (t.id::text=$1 OR (t.owner_id::text=$2 AND t.personal AND t.deleted_at IS NULL)) AND j.kind='subscription' AND j.state<>'done' AND NOT (j.state='pending' AND j.attempts=0 AND j.external_id='') AND NOT (j.state='failed' AND j.error_category='provider_rejected' AND j.external_id=''))").bind(tenant).bind(owner).fetch_one(&mut *self.tx).await?;
        if uncertain {
            return Err(precondition(
                "reconcile the pending subscription before requesting deletion",
            ));
        }
        let stopped=sqlx::query("UPDATE jobs j SET state='done',error_category=CASE WHEN j.error_category='provider_rejected' THEN j.error_category ELSE 'deletion_requested' END,updated_at=$3 FROM tenants t WHERE t.id=j.tenant_id AND (t.id::text=$1 OR (t.owner_id::text=$2 AND t.personal AND t.deleted_at IS NULL)) AND j.kind='subscription' AND j.external_id='' AND ((j.state='pending' AND j.attempts=0) OR (j.state='failed' AND j.error_category='provider_rejected'))").bind(tenant).bind(owner).bind(self.now).execute(&mut *self.tx).await?.rows_affected();
        if stopped > 0 {
            tracing::info!(
                stopped_jobs = stopped,
                "subscription intents stopped for deletion"
            );
        }
        Ok(())
    }
    async fn list_billing_records(&mut self, op: Op, q: &Value) -> crate::Result<Value> {
        if !self.identity.operator {
            return Err(forbidden());
        }
        let (n, after) = page(&q["page"])?;
        let audit = matches!(op, Op::ListAudit);
        let sql = if audit {
            "SELECT id::text,actor_id,action,reason,created_at FROM audit WHERE id>$1 ORDER BY id LIMIT $2"
        } else {
            "SELECT id::text,kind,state,error_category,attempts FROM jobs WHERE id>$1 ORDER BY id LIMIT $2"
        };
        let rows = sqlx::query(sql)
            .bind(uuid(&after)?)
            .bind(n + 1)
            .fetch_all(&mut *self.tx)
            .await?;
        let more = rows.len() > n as usize;
        let mut items = Vec::new();
        for row in rows.into_iter().take(n as usize) {
            items.push(if audit{json!({"id":row.try_get::<String,_>("id")?,"actorId":row.try_get::<String,_>("actor_id")?,"action":row.try_get::<String,_>("action")?,"reason":row.try_get::<String,_>("reason")?,"createdAt":row.try_get::<DateTime<Utc>,_>("created_at")?.to_rfc3339()})}else{
                let state=match row.try_get::<&str,_>("state")?{"pending"=>1,"running"=>2,"reconcile"=>3,"done"=>4,"failed"=>5,_=>0};
                json!({"id":row.try_get::<String,_>("id")?,"kind":row.try_get::<String,_>("kind")?,"state":state,"errorCategory":row.try_get::<String,_>("error_category")?,"attempts":row.try_get::<i32,_>("attempts")?})
            });
        }
        let next = if more {
            items.last().map(|v| string(v, "id")).unwrap_or("")
        } else {
            ""
        }
        .to_owned();
        Ok(if audit {
            json!({"entries":items,"nextAfterId":next})
        } else {
            json!({"jobs":items,"nextAfterId":next})
        })
    }
}

impl Call<'_> {
    async fn change_subscription(&mut self, q: &Value) -> crate::Result<Value> {
        let tenant = string(q, "tenantId");
        self.require(tenant, &[3]).await?;
        let plan = string(q, "planId");
        let prices: Vec<String> = q["priceIds"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().unwrap_or("").to_owned())
                    .collect()
            })
            .unwrap_or_default();
        if uuid(plan).is_err()
            || prices.is_empty()
            || prices.len() > 100
            || !self.config.allows_return(string(q, "returnUrl"))
        {
            return Err(invalid("invalid plan, prices or return URL"));
        }
        let t = self.tenant(tenant).await?;
        if t["deleting"].as_bool().unwrap_or(false) {
            return Err(precondition("tenant is deleting"));
        }
        let old_rank: i32 = sqlx::query_scalar(&format!(
            "SELECT {} FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1",
            postgres::selected_plan_rank_sql("t", "p")
        ))
        .bind(uuid(tenant)?)
        .fetch_one(&mut *self.tx)
        .await?;
        let (new_rank, free): (i32, bool) =
            sqlx::query_as("SELECT rank,free FROM plans WHERE id=$1")
                .bind(uuid(plan)?)
                .fetch_optional(&mut *self.tx)
                .await?
                .ok_or_else(|| invalid("unknown plan"))?;
        let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE tenant_id=$1 AND kind IN ('subscription','cancel') AND state IN ('pending','running','reconcile'))").bind(uuid(tenant)?).fetch_one(&mut *self.tx).await?;
        if pending {
            return Err(precondition("a subscription change is already pending"));
        }
        let mut external = Vec::new();
        let mut seen = BTreeSet::new();
        let mut new_addons = BTreeSet::new();
        let mut requested_prices =
            BTreeMap::<String, Vec<String>>::from([(plan.to_owned(), vec![])]);
        let mut usage_meters = BTreeMap::<String, Decimal>::new();
        let mut addon_recurring = BTreeMap::<String, i32>::new();
        let mut base_recurring = 0;
        for price in &prices {
            if uuid(price).is_err() || !seen.insert(price.clone()) {
                return Err(invalid("invalid or repeated price"));
            }
            let row=sqlx::query("SELECT external_price_id,COALESCE(plan_id::text,'') AS owner_plan,COALESCE(addon_id::text,'') AS addon,kind,service_id::text,meter,unit_price::text FROM prices WHERE id=$1").bind(uuid(price)?).fetch_optional(&mut *self.tx).await?.ok_or_else(||invalid("unknown price"))?;
            let current:bool=sqlx::query_scalar("SELECT CASE WHEN ce.id IS NOT NULL THEN ce.active_version_id=selected.id ELSE NOT EXISTS(SELECT 1 FROM prices newer WHERE newer.plan_id IS NOT DISTINCT FROM selected.plan_id AND newer.addon_id IS NOT DISTINCT FROM selected.addon_id AND newer.service_id=selected.service_id AND newer.meter=selected.meter AND newer.kind=selected.kind AND (newer.created_at,newer.id)>(selected.created_at,selected.id)) END FROM prices selected LEFT JOIN catalog_versions cv ON cv.id=selected.id LEFT JOIN catalog_entities ce ON ce.id=cv.entity_id WHERE selected.id=$1").bind(uuid(price)?).fetch_one(&mut *self.tx).await?;
            if !current {
                return Err(invalid("explicit changes require current price versions"));
            }
            let addon: String = row.try_get("addon")?;
            if addon.is_empty() && row.try_get::<String, _>("owner_plan")? != plan {
                return Err(invalid("price belongs to another plan"));
            }
            let product = if addon.is_empty() {
                plan.to_owned()
            } else {
                new_addons.insert(addon.clone());
                addon.clone()
            };
            requested_prices
                .entry(product)
                .or_default()
                .push(price.clone());
            let kind: i32 = row.try_get("kind")?;
            if kind == 1 {
                let key = format!(
                    "{}\0{}",
                    row.try_get::<String, _>("service_id")?,
                    row.try_get::<String, _>("meter")?
                );
                let rate = Decimal::parse(&row.try_get::<String, _>("unit_price")?)
                    .map_err(anyhow::Error::from)?;
                if usage_meters.get(&key).is_some_and(|v| v != &rate) {
                    return Err(invalid("conflicting rates for the same service meter"));
                }
                usage_meters.insert(key, rate);
            }
            if kind == 2 {
                let ext: String = row.try_get("external_price_id")?;
                if ext.is_empty() {
                    return Err(precondition("Paddle recurring price mapping required"));
                }
                external.push(ext);
                if addon.is_empty() {
                    base_recurring += 1;
                } else {
                    *addon_recurring.entry(addon).or_default() += 1;
                }
            }
        }
        if (!free && base_recurring != 1) || (free && base_recurring != 0) {
            return Err(invalid(
                "paid base plans require one recurring base price; free base plans require none",
            ));
        }
        if new_addons
            .iter()
            .any(|a| addon_recurring.get(a) != Some(&1))
        {
            return Err(invalid("one recurring price is required for each add-on"));
        }
        let versions = postgres::active_product_versions(
            &mut *self.tx,
            plan,
            &new_addons.iter().cloned().collect::<Vec<_>>(),
        )
        .await?;
        for (product, requested) in &requested_prices {
            let version = postgres::active_catalog_version(
                &mut *self.tx,
                if product == plan { "plan" } else { "addon" },
                product,
            )
            .await?;
            if version.is_empty() {
                continue;
            }
            let active = postgres::product_version_prices(&mut *self.tx, product, &version).await?;
            // Legacy prices have no catalog version. Compare catalog prices only.
            let requested_ids = requested
                .iter()
                .map(|v| uuid(v))
                .collect::<crate::Result<Vec<_>>>()?;
            let active_ids = active
                .iter()
                .map(|v| uuid(v))
                .collect::<crate::Result<Vec<_>>>()?;
            let wanted: Vec<String> = sqlx::query_scalar(
                "SELECT id::text FROM catalog_versions WHERE id=ANY($1) ORDER BY id::text",
            )
            .bind(requested_ids)
            .fetch_all(&mut *self.tx)
            .await?;
            let available: Vec<String> = sqlx::query_scalar(
                "SELECT id::text FROM catalog_versions WHERE id=ANY($1) ORDER BY id::text",
            )
            .bind(active_ids)
            .fetch_all(&mut *self.tx)
            .await?;
            if wanted != available {
                return Err(invalid(
                    "selected prices must match the active product version",
                ));
            }
        }
        if postgres::conflicting_feature(&mut *self.tx, &versions).await? {
            return Err(invalid("base plan and add-on define the same feature"));
        }
        let old_addons:BTreeSet<String>=sqlx::query_scalar::<_,String>("SELECT DISTINCT p.addon_id::text FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=$1 AND en.ends_at IS NULL AND p.addon_id IS NOT NULL").bind(uuid(tenant)?).fetch_all(&mut *self.tx).await?.into_iter().collect();
        let added = new_addons.difference(&old_addons).next().is_some();
        let removed = old_addons.difference(&new_addons).next().is_some();
        if (added && removed) || (new_rank > old_rank && removed) || (new_rank < old_rank && added)
        {
            return Err(invalid(
                "submit immediate additions and renewal removals as separate changes",
            ));
        }
        let timing = if new_rank < old_rank || removed {
            "do_not_bill"
        } else {
            "prorated_immediately"
        };
        let mut local_refresh = false;
        if plan == string(&t, "planId") && !added && !removed && !external.is_empty() {
            let old:Vec<String>=sqlx::query_scalar("SELECT p.external_price_id FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=$1 AND en.ends_at IS NULL AND p.kind=2 ORDER BY p.external_price_id").bind(uuid(tenant)?).fetch_all(&mut *self.tx).await?;
            external.sort();
            // Only a non-decreasing rank may refresh locally. A rank decrease keeps the provider
            // subscription and must wait for the confirmed renewal through the paid
            // `do_not_bill` job, even when the recurring price set is unchanged.
            local_refresh = old == external && new_rank >= old_rank;
        }
        let key = format!("request:{}", string(q, "requestKey"));
        if (free && external.is_empty()) || local_refresh {
            let(subscription,start,end):(String,DateTime<Utc>,DateTime<Utc>)=sqlx::query_as("SELECT external_subscription_id,period_start,period_end FROM tenants WHERE id=$1").bind(uuid(tenant)?).fetch_one(&mut *self.tx).await?;
            if !subscription.is_empty() && !local_refresh {
                return Err(precondition(
                    "use cancellation to return to the default free plan",
                ));
            }
            if new_rank < old_rank {
                let job=self.job(tenant,"subscription",&key,&json!({"plan_id":plan,"price_ids":prices,"version_ids":versions,"timing":"free_next_period"})).await?;
                sqlx::query("UPDATE jobs SET available_at=$2 WHERE id=$1")
                    .bind(uuid(&job)?)
                    .bind(end)
                    .execute(&mut *self.tx)
                    .await?;
                return Ok(json!({"jobId":job}));
            }
            sqlx::query("UPDATE tenants SET plan_id=$2 WHERE id=$1")
                .bind(uuid(tenant)?)
                .bind(uuid(plan)?)
                .execute(&mut *self.tx)
                .await?;
            postgres::replace_tenant_selections(&mut *self.tx, tenant, &versions, self.now).await?;
            let previous = postgres::current_allowances(&mut *self.tx, tenant).await?;
            sqlx::query("UPDATE entitlements SET ends_at=$2 WHERE tenant_id=$1 AND ends_at IS NULL AND starts_at<$2").bind(uuid(tenant)?).bind(self.now).execute(&mut *self.tx).await?;
            for price in &prices {
                sqlx::query(
                    "INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2,$3,$4)",
                )
                .bind(uuid(&id())?)
                .bind(uuid(tenant)?)
                .bind(uuid(price)?)
                .bind(self.now)
                .execute(&mut *self.tx)
                .await?;
            }
            postgres::grant_allowances(
                &mut *self.tx,
                tenant,
                start,
                end,
                self.now,
                &format!("free_refresh:{}", string(q, "requestKey")),
                Some(&previous),
            )
            .await?;
            let job = self
                .job(
                    tenant,
                    "subscription",
                    &key,
                    &json!({"plan_id":plan,"price_ids":prices,"version_ids":versions,"amount":"0"}),
                )
                .await?;
            sqlx::query("UPDATE jobs SET state='done' WHERE id=$1")
                .bind(uuid(&job)?)
                .execute(&mut *self.tx)
                .await?;
            self.audit(tenant, "subscription.free_refresh", "").await?;
            return Ok(json!({"jobId":job}));
        }
        let job=self.job(tenant,"subscription",&key,&json!({"plan_id":plan,"price_ids":prices,"version_ids":versions,"external_price_ids":external,"timing":timing,"return_url":string(q,"returnUrl")})).await?;
        Ok(json!({"jobId":job}))
    }
}
