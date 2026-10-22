// SPDX-License-Identifier: FSL-1.1-ALv2
pub(super) use super::renewal::matches_prices;
use super::{Worker, maintenance::notify};
use crate::{
    postgres,
    provider::{Event, Transaction},
};
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::{PgConnection, Row};

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct PaymentPayload {
    pub amount: String,
    pub minor_units: String,
    pub plan_id: String,
    pub price_ids: Vec<String>,
    pub version_ids: Vec<String>,
    pub external_price_ids: Vec<String>,
}
fn valid_job(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|v| v.get_version_num() == 7)
}

impl Worker {
    /// Persist a verified provider notification before acknowledging it.
    pub async fn ingest_webhook(&self, signature: &str, raw: &[u8]) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        if raw.len() > self.config.max_payload_bytes {
            return StatusCode::PAYLOAD_TOO_LARGE;
        }
        if !crate::provider::verify_signature(
            &self.config.paddle_webhook_secret,
            signature,
            raw,
            Utc::now(),
        ) {
            return StatusCode::UNAUTHORIZED;
        }
        let event: Event = match serde_json::from_slice(raw) {
            Ok(v) => v,
            Err(_) => return StatusCode::BAD_REQUEST,
        };
        if event.id.is_empty()
            || event.id.len() > 200
            || (event.occurred_at.timestamp() == -62_135_596_800
                && event.occurred_at.timestamp_subsec_nanos() == 0)
        {
            return StatusCode::BAD_REQUEST;
        }
        let result = tokio::time::timeout(self.config.timeout(), async {
            let mut tx = postgres::begin(&self.pool).await?;
            let rows = sqlx::query("INSERT INTO webhooks(event_id,occurred_at,payload,received_at) VALUES($1,$2,$3,now()) ON CONFLICT DO NOTHING").bind(&event.id).bind(event.occurred_at).bind(serde_json::from_slice::<serde_json::Value>(raw)?).execute(&mut *tx).await?.rows_affected();
            if rows > 0 {
                sqlx::query("INSERT INTO jobs(id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1,'webhook',$2,$3,now(),now(),now())").bind(uuid::Uuid::now_v7()).bind(format!("webhook:{}",event.id)).bind(serde_json::json!({"event_id":event.id})).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            Ok::<_, anyhow::Error>(())
        }).await;
        match result {
            Ok(Ok(())) => StatusCode::ACCEPTED,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    pub async fn process_webhook(&self, selected_id: &str) -> Result<()> {
        let mut tx = postgres::begin(&self.pool).await?;
        let Some(row) = sqlx::query("SELECT event_id,payload FROM webhooks WHERE processed_at IS NULL AND event_id=$1 FOR UPDATE").bind(selected_id).fetch_optional(&mut *tx).await? else { return Ok(()); };
        let event_id: String = row.try_get(0)?;
        let raw: serde_json::Value = row.try_get(1)?;
        let event: Event = serde_json::from_value(raw)?;
        match event.r#type.as_str() {
            "transaction.completed" => {
                self.apply_transaction_in(
                    &mut tx,
                    serde_json::from_value(event.data.clone())?,
                    event.occurred_at,
                )
                .await?;
            }
            "transaction.payment_failed" => {
                let t: Transaction = serde_json::from_value(event.data.clone())?;
                let t = t.attribute_charge(&self.config.paddle_credit_product_id);
                if t.origin == "subscription_recurring"
                    && let Some(period) = &t.billing_period
                {
                    let marked = sqlx::query("UPDATE tenants SET renewal_failed=true WHERE external_subscription_id=$1 AND external_customer_id=$2 AND period_end<=$3 AND NOT EXISTS(SELECT 1 FROM provider_transactions WHERE external_id=$4)").bind(&t.subscription_id).bind(&t.customer_id).bind(period.starts_at).bind(&t.id).execute(&mut *tx).await?.rows_affected();
                    if marked > 0 {
                        // Keep the occurrence so an older renewal payment delivered later cannot clear it.
                        self.retain_subscription_failure(&mut tx, &t.subscription_id, &event)
                            .await?;
                    }
                } else if valid_job(&t.custom.job_id) {
                    sqlx::query("UPDATE jobs SET state='failed',error_category='payment_failed',updated_at=now() WHERE id=$1::text::uuid AND tenant_id::text=$2 AND state<>'done'").bind(&t.custom.job_id).bind(&t.custom.tenant_id).execute(&mut *tx).await?;
                    let changed = sqlx::query("UPDATE tenants SET auto_suspended=true WHERE id::text=$1 AND EXISTS(SELECT 1 FROM jobs WHERE id=$2::text::uuid AND kind='auto_topup' AND state='failed')").bind(&t.custom.tenant_id).bind(&t.custom.job_id).execute(&mut *tx).await?.rows_affected();
                    if changed > 0 {
                        notify(
                            &mut tx,
                            &t.custom.tenant_id,
                            &format!("auto_failed:{}", t.custom.job_id),
                            "Automatic top-up failed",
                            "Automatic top-ups are suspended. Review your payment settings.",
                            event.occurred_at,
                        )
                        .await?;
                    }
                }
            }
            kind if kind.starts_with("subscription.") => {
                self.subscription_event(&mut tx, &event).await?
            }
            kind if kind.starts_with("adjustment.") => {
                self.adjustment_event(&mut tx, &event).await?
            }
            _ => {}
        }
        sqlx::query("UPDATE webhooks SET processed_at=now() WHERE event_id=$1")
            .bind(&event_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE jobs SET state='done',error_category='',updated_at=now() WHERE kind='webhook' AND payload->>'event_id'=$1").bind(&event_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn apply_transaction(&self, t: Transaction, at: DateTime<Utc>) -> Result<()> {
        let mut tx = postgres::begin(&self.pool).await?;
        self.apply_transaction_in(&mut tx, t, at).await?;
        tx.commit().await?;
        Ok(())
    }
    async fn apply_transaction_in(
        &self,
        conn: &mut PgConnection,
        t: Transaction,
        mut at: DateTime<Utc>,
    ) -> Result<()> {
        let mut t = t.attribute_charge(&self.config.paddle_credit_product_id);
        if t.status != "completed" {
            return Ok(());
        }
        if t.origin == "subscription_recurring" {
            self.apply_renewal_settlements(conn, &t, at).await?;
            t.items.retain(|item| {
                item.price.product_id != self.config.paddle_credit_product_id
                    || item.price.custom.job_id.is_empty()
            });
            // Order the renewal by when the provider billed it, like the initial payment, so a
            // reconciled payment is not compared with newer failures by its processing time.
            let paid_at = t.confirmed_at(at);
            return self.renew(conn, &t, paid_at).await;
        }
        if !valid_job(&t.custom.job_id) {
            return Ok(());
        }
        let Some(row)=sqlx::query("SELECT id::text,tenant_id::text,kind,state,external_id,payload FROM jobs WHERE id=$1::text::uuid").bind(&t.custom.job_id).fetch_optional(&mut *conn).await? else { return Ok(()); };
        let job: String = row.try_get(0)?;
        let tenant: String = row.try_get(1)?;
        let kind: String = row.try_get(2)?;
        let state: String = row.try_get(3)?;
        let external: String = row.try_get(4)?;
        if state == "done" {
            return Ok(());
        }
        let p: PaymentPayload = serde_json::from_value(row.try_get(5)?)?;
        if t.custom.tenant_id != tenant
            || t.currency != "USD"
            || (!external.is_empty() && external != t.id && external != t.subscription_id)
            || (!p.minor_units.is_empty() && t.details.totals.subtotal != p.minor_units)
        {
            sqlx::query("UPDATE jobs SET state='failed',error_category='payment_mismatch',updated_at=now() WHERE id=$1::text::uuid").bind(&job).execute(&mut *conn).await?;
            if kind == "auto_topup" {
                sqlx::query("UPDATE tenants SET auto_suspended=true WHERE id=$1::text::uuid")
                    .bind(&tenant)
                    .execute(&mut *conn)
                    .await?;
                notify(
                    conn,
                    &tenant,
                    &format!("auto_failed:{job}"),
                    "Automatic top-up requires review",
                    "Automatic top-ups are suspended until the completed payment is reconciled.",
                    at,
                )
                .await?;
            }
            return Ok(());
        }
        match kind.as_str() {
            "topup" | "auto_topup" => {
                if p.amount.is_empty() {
                    bail!("top-up amount missing");
                }
                sqlx::query("INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2::text::uuid,'credit',$3::text::numeric,$4,$5) ON CONFLICT(source_key) DO NOTHING").bind(uuid::Uuid::now_v7()).bind(&tenant).bind(&p.amount).bind(format!("payment:{}",t.id)).bind(at).execute(&mut *conn).await?;
            }
            "settlement" => {
                for line in &t.details.line_items {
                    if line.id.is_empty() || line.totals.subtotal != p.minor_units {
                        continue;
                    }
                    for item in &t.items {
                        if item.price.id == line.price_id
                            && item.price.product_id == self.config.paddle_credit_product_id
                            && item.price.custom.job_id == job
                            && item.price.custom.tenant_id == tenant
                        {
                            super::settlement_refunds::record_settlement_line(
                                conn,
                                &t.id,
                                &line.id,
                                &job,
                                &p.amount,
                                &p.minor_units,
                            )
                            .await?;
                        }
                    }
                }
                sqlx::query("INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2::text::uuid,'settlement',$3::text::numeric,$4,$5) ON CONFLICT(source_key) DO NOTHING").bind(uuid::Uuid::now_v7()).bind(&tenant).bind(&p.amount).bind(format!("payment:{}",t.id)).bind(at).execute(&mut *conn).await?;
            }
            "subscription" => {
                // Provider payment time orders retained status notifications, even during late reconciliation.
                at = t.confirmed_at(at);
                if !matches_prices(&t, &p.external_price_ids) {
                    sqlx::query("UPDATE jobs SET state='failed',error_category='payment_mismatch',updated_at=now() WHERE id=$1::text::uuid").bind(&job).execute(&mut *conn).await?;
                    return Ok(());
                }
                if t.subscription_id.is_empty()
                    || t.billing_period
                        .as_ref()
                        .is_none_or(|v| v.ends_at <= v.starts_at)
                {
                    sqlx::query("UPDATE jobs SET state='reconcile',external_id=$2,error_category='subscription_period_missing',available_at=now() WHERE id=$1::text::uuid").bind(&job).bind(&t.id).execute(&mut *conn).await?;
                    return Ok(());
                }
                let period = t.billing_period.as_ref().unwrap();
                let row=sqlx::query("SELECT external_subscription_id,period_start,period_end FROM tenants WHERE id=$1::text::uuid").bind(&tenant).fetch_one(&mut *conn).await?;
                let old: String = row.try_get(0)?;
                let mut start: DateTime<Utc> = row.try_get(1)?;
                let mut end: DateTime<Utc> = row.try_get(2)?;
                let previous = if old == t.subscription_id && end > at {
                    Some(postgres::allowances::current_allowances(conn, &tenant).await?)
                } else {
                    start = period.starts_at;
                    end = period.ends_at;
                    None
                };
                sqlx::query("UPDATE tenants SET plan_id=$2::text::uuid,external_customer_id=$3,external_subscription_id=$4,period_start=$5,period_end=$6 WHERE id=$1::text::uuid").bind(&tenant).bind(&p.plan_id).bind(&t.customer_id).bind(&t.subscription_id).bind(start).bind(end).execute(&mut *conn).await?;
                postgres::catalog::replace_tenant_selections(conn, &tenant, &p.version_ids, at)
                    .await?;
                sqlx::query(&format!("INSERT INTO billing_periods SELECT t.id,$2,$3,{} FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1::text::uuid ON CONFLICT DO NOTHING",postgres::period::selected_plan_bool_sql("t","p","postpaid"))).bind(&tenant).bind(start).bind(end).execute(&mut *conn).await?;
                sqlx::query("UPDATE entitlements SET ends_at=$2 WHERE tenant_id=$1::text::uuid AND ends_at IS NULL AND starts_at<$2").bind(&tenant).bind(at).execute(&mut *conn).await?;
                for price in &p.price_ids {
                    sqlx::query("INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2::text::uuid,$3::text::uuid,$4)").bind(uuid::Uuid::now_v7()).bind(&tenant).bind(price).bind(at).execute(&mut *conn).await?;
                }
                postgres::allowances::grant_allowances(
                    conn,
                    &tenant,
                    start,
                    end,
                    at,
                    &format!("payment:{}", t.id),
                    previous.as_ref(),
                )
                .await?;
                self.apply_retained_subscription_state(conn, &tenant, &t.subscription_id, at)
                    .await?;
            }
            _ => return Ok(()),
        }
        sqlx::query("INSERT INTO provider_transactions VALUES($1,$2::text::uuid,$3,$4::text::numeric,$5,$6) ON CONFLICT DO NOTHING").bind(&t.id).bind(&tenant).bind(&kind).bind(if p.amount.is_empty(){"0"}else{&p.amount}).bind(&t.currency).bind(at).execute(&mut *conn).await?;
        sqlx::query("UPDATE jobs SET state='done',external_id=$2,error_category='',updated_at=now() WHERE id=$1::text::uuid").bind(&job).bind(&t.id).execute(&mut *conn).await?;
        Ok(())
    }
    pub(super) async fn subscription_event(
        &self,
        conn: &mut PgConnection,
        event: &Event,
    ) -> Result<()> {
        #[derive(Deserialize)]
        struct State {
            #[serde(default)]
            id: String,
            #[serde(default)]
            status: String,
        }
        let v: State = serde_json::from_value(event.data.clone())?;
        if v.id.is_empty() {
            return Ok(());
        }
        let previous: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT occurred_at FROM provider_objects WHERE external_id=$1")
                .bind(&v.id)
                .fetch_optional(&mut *conn)
                .await?;
        let tenant: Option<String> =
            sqlx::query_scalar("SELECT id::text FROM tenants WHERE external_subscription_id=$1")
                .bind(&v.id)
                .fetch_optional(&mut *conn)
                .await?;
        if previous.is_some_and(|p| event.occurred_at <= p) {
            if v.status == "past_due" {
                match &tenant {
                    None => self.retain_subscription_failure(conn, &v.id, event).await?,
                    Some(tenant) => {
                        // A later informational status leaves this watermark newer than the
                        // failure, which only a confirmed payment at or after it can resolve.
                        let paid_since: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_transactions WHERE tenant_id=$1::text::uuid AND kind='subscription' AND created_at>=$2)")
                            .bind(tenant)
                            .bind(event.occurred_at)
                            .fetch_one(&mut *conn)
                            .await?;
                        if !paid_since {
                            sqlx::query(
                                "UPDATE tenants SET renewal_failed=true WHERE id=$1::text::uuid",
                            )
                            .bind(tenant)
                            .execute(&mut *conn)
                            .await?;
                            self.retain_subscription_failure(conn, &v.id, event).await?;
                        }
                    }
                }
            }
            return Ok(());
        }
        sqlx::query("INSERT INTO provider_objects VALUES($1,$2,$3) ON CONFLICT(external_id) DO UPDATE SET occurred_at=EXCLUDED.occurred_at,state=EXCLUDED.state").bind(&v.id).bind(event.occurred_at).bind(&v.status).execute(&mut *conn).await?;
        let Some(tenant) = tenant else {
            if v.status == "past_due" {
                self.retain_subscription_failure(conn, &v.id, event).await?;
            }
            tracing::info!(subscription_id=%v.id,event_id=%event.id,state=%v.status,"retained subscription state before tenant association");
            return Ok(());
        };
        match v.status.as_str() {
            "past_due" => {
                sqlx::query("UPDATE tenants SET renewal_failed=true WHERE id=$1::text::uuid")
                    .bind(&tenant)
                    .execute(&mut *conn)
                    .await?;
                // Keep the occurrence so an older renewal payment delivered later cannot clear it.
                self.retain_subscription_failure(conn, &v.id, event).await?;
            }
            "canceled" => self.activate_free(conn, &tenant, event.occurred_at).await?,
            _ => {}
        }
        // An active status alone cannot grant entitlements or clear payment failure.
        Ok(())
    }
    async fn retain_subscription_failure(
        &self,
        conn: &mut PgConnection,
        subscription: &str,
        event: &Event,
    ) -> Result<()> {
        // Store failure separately so a later informational active status cannot erase it.
        sqlx::query("INSERT INTO provider_objects VALUES($1,$2,'past_due') ON CONFLICT(external_id) DO UPDATE SET occurred_at=EXCLUDED.occurred_at WHERE provider_objects.occurred_at<EXCLUDED.occurred_at").bind(format!("failure:{subscription}")).bind(event.occurred_at).execute(&mut *conn).await?;
        tracing::info!(subscription_id=subscription,event_id=%event.id,"retained unresolved subscription payment failure");
        Ok(())
    }
    async fn adjustment_event(&self, conn: &mut PgConnection, event: &Event) -> Result<()> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Totals {
            subtotal: String,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Adjustment {
            id: String,
            action: String,
            status: String,
            transaction_id: String,
            currency_code: String,
            totals: Totals,
        }
        let v: Adjustment = serde_json::from_value(event.data.clone())?;
        if v.id.is_empty() || v.currency_code != "USD" {
            return Ok(());
        }
        let Some(row)=sqlx::query("SELECT tenant_id::text,kind,amount::text FROM provider_transactions WHERE external_id=$1").bind(&v.transaction_id).fetch_optional(&mut *conn).await? else {bail!("adjustment payment attribution is pending");};
        let tenant: String = row.try_get(0)?;
        let kind: String = row.try_get(1)?;
        let principal: String = row.try_get(2)?;
        // Reverse notifications update the original adjustment; they do not apply another amount.
        if v.action.ends_with("_reverse") {
            return Ok(());
        }
        let prior_row = sqlx::query(
            "SELECT amount::text,occurred_at FROM adjustment_effects WHERE external_id=$1",
        )
        .bind(&v.id)
        .fetch_optional(&mut *conn)
        .await?;
        let prior = if let Some(row) = prior_row {
            let previous: DateTime<Utc> = row.try_get(1)?;
            if event.occurred_at <= previous {
                return Ok(());
            }
            row.try_get::<String, _>(0)?
        } else {
            "0".to_owned()
        };
        let mut effect = "0".to_owned();
        if v.status == "approved" && (v.action == "refund" || v.action == "chargeback") {
            let amount = crate::accounting::Decimal::parse(&v.totals.subtotal)?;
            if amount < crate::accounting::Decimal::zero() {
                bail!("invalid adjustment amount");
            }
            effect = sqlx::query_scalar("SELECT ($1::text::numeric/power(10::numeric,2))::text")
                .bind(amount.to_string())
                .fetch_one(&mut *conn)
                .await?;
            let exceeds:bool=sqlx::query_scalar("SELECT $1::text::numeric+COALESCE(sum(amount),0)>$2::text::numeric FROM adjustment_effects WHERE transaction_id=$3 AND external_id<>$4").bind(&effect).bind(&principal).bind(&v.transaction_id).bind(&v.id).fetch_one(&mut *conn).await?;
            if (kind == "topup" || kind == "auto_topup") && exceeds {
                bail!("adjustment exceeds purchased principal");
            }
        }
        if v.action == "chargeback" || v.action == "chargeback_warning" {
            sqlx::query("INSERT INTO disputes(external_id,tenant_id,active,transaction_id) VALUES($1,$2::text::uuid,$3,$4) ON CONFLICT(external_id) DO UPDATE SET active=EXCLUDED.active").bind(&v.id).bind(&tenant).bind(v.status!="reversed"&&v.status!="rejected").bind(&v.transaction_id).execute(&mut *conn).await?;
        }
        let delta: String =
            sqlx::query_scalar("SELECT ($1::text::numeric-$2::text::numeric)::text")
                .bind(&prior)
                .bind(&effect)
                .fetch_one(&mut *conn)
                .await?;
        let nonzero: bool = sqlx::query_scalar("SELECT $1::text::numeric<>0")
            .bind(&delta)
            .fetch_one(&mut *conn)
            .await?;
        if nonzero {
            // Subscription and invoice refunds are payment history, not purchased credit.
            let ledger_kind = if kind == "topup" || kind == "auto_topup" {
                "refund"
            } else {
                "payment_refund"
            };
            sqlx::query("INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2::text::uuid,$6,$3::text::numeric,$4,$5) ON CONFLICT(source_key) DO NOTHING").bind(uuid::Uuid::now_v7()).bind(&tenant).bind(&delta).bind(format!("adjustment:{}:{}",v.id,event.occurred_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi,true))).bind(event.occurred_at).bind(ledger_kind).execute(&mut *conn).await?;
        }
        if v.action == "refund" {
            self.apply_settlement_refund(conn, event, &tenant, &v.id, &v.transaction_id, &v.status)
                .await?;
        }
        sqlx::query("INSERT INTO adjustment_effects VALUES($1,$2,$3::text::numeric,$4) ON CONFLICT(external_id) DO UPDATE SET amount=EXCLUDED.amount,occurred_at=EXCLUDED.occurred_at").bind(&v.id).bind(&v.transaction_id).bind(&effect).bind(event.occurred_at).execute(&mut *conn).await?;
        Ok(())
    }
    async fn apply_renewal_settlements(
        &self,
        conn: &mut PgConnection,
        t: &Transaction,
        at: DateTime<Utc>,
    ) -> Result<()> {
        for item in &t.items {
            if item.price.product_id != self.config.paddle_credit_product_id
                || item.quantity != 1
                || !valid_job(&item.price.custom.job_id)
            {
                continue;
            }
            let job = &item.price.custom.job_id;
            let line = t
                .details
                .line_items
                .iter()
                .rev()
                .find(|line| line.price_id == item.price.id);
            let (subtotal, item_id) = line
                .map(|line| (line.totals.subtotal.as_str(), line.id.as_str()))
                .unwrap_or(("", ""));
            let Some(row)=sqlx::query("SELECT j.tenant_id::text,j.payload->>'amount' FROM jobs j JOIN tenants t ON t.id=j.tenant_id WHERE j.id=$1::text::uuid AND j.kind='settlement' AND j.state<>'done' AND j.tenant_id::text=$2 AND j.payload->>'minor_units'=$3 AND t.external_subscription_id=$4 AND t.external_customer_id=$5 AND j.payload->>'currency'=$6").bind(job).bind(&item.price.custom.tenant_id).bind(subtotal).bind(&t.subscription_id).bind(&t.customer_id).bind(&t.currency).fetch_optional(&mut *conn).await? else {continue;};
            let tenant: String = row.try_get(0)?;
            let amount: String = row.try_get(1)?;
            if !item_id.is_empty() {
                super::settlement_refunds::record_settlement_line(
                    conn, &t.id, item_id, job, &amount, subtotal,
                )
                .await?;
            }
            sqlx::query("INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2::text::uuid,'settlement',$3::text::numeric,$4,$5) ON CONFLICT(source_key) DO NOTHING").bind(uuid::Uuid::now_v7()).bind(&tenant).bind(&amount).bind(format!("settlement-job:{job}")).bind(at).execute(&mut *conn).await?;
            sqlx::query("UPDATE jobs SET state='done',external_id=$2,error_category='',updated_at=now() WHERE id=$1::text::uuid").bind(job).bind(&t.id).execute(&mut *conn).await?;
        }
        Ok(())
    }
    async fn apply_retained_subscription_state(
        &self,
        conn: &mut PgConnection,
        tenant: &str,
        subscription: &str,
        paid_at: DateTime<Utc>,
    ) -> Result<()> {
        let state =
            sqlx::query("SELECT occurred_at,state FROM provider_objects WHERE external_id=$1")
                .bind(subscription)
                .fetch_optional(&mut *conn)
                .await?;
        let failed: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT occurred_at FROM provider_objects WHERE external_id=$1")
                .bind(format!("failure:{subscription}"))
                .fetch_optional(&mut *conn)
                .await?;
        let (occurred, state) = match state {
            Some(row) => (
                Some(row.try_get::<DateTime<Utc>, _>(0)?),
                row.try_get::<String, _>(1)?,
            ),
            None => (None, String::new()),
        };
        // A failure applied to the tenant stays unresolved, so a recurring payment that occurred
        // before it but is delivered after the association cannot clear it.
        let mut keep_failure = false;
        if state == "canceled" && occurred.is_some_and(|at| at > paid_at) {
            tracing::warn!(
                tenant_id = tenant,
                subscription_id = subscription,
                "applying retained subscription cancellation"
            );
            self.activate_free(conn, tenant, occurred.unwrap()).await?;
        } else if failed.is_some_and(|at| at > paid_at)
            || (state == "past_due" && occurred.is_some_and(|at| at > paid_at))
        {
            tracing::warn!(
                tenant_id = tenant,
                subscription_id = subscription,
                "applying retained subscription payment failure"
            );
            sqlx::query("UPDATE tenants SET renewal_failed=true WHERE id=$1::text::uuid")
                .bind(tenant)
                .execute(&mut *conn)
                .await?;
            keep_failure = failed.is_some();
        }
        // The payment becomes the ordering watermark; the tenant now carries any retained failure.
        sqlx::query("INSERT INTO provider_objects VALUES($1,$2,'active') ON CONFLICT(external_id) DO UPDATE SET occurred_at=EXCLUDED.occurred_at,state='active' WHERE provider_objects.occurred_at<EXCLUDED.occurred_at").bind(subscription).bind(paid_at).execute(&mut *conn).await?;
        if !keep_failure {
            sqlx::query("DELETE FROM provider_objects WHERE external_id=$1")
                .bind(format!("failure:{subscription}"))
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}
