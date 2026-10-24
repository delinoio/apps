// SPDX-License-Identifier: FSL-1.1-ALv2
mod cancellation;
mod maintenance;
mod metering;
mod notices;
mod rating;
mod reconcile;
mod renewal;
mod retention;
mod rounding;
mod settlement;
mod settlement_refunds;
mod webhook;
use crate::{
    config::Config,
    postgres,
    provider::{self, Client, Error},
};
use anyhow::Result;
use chrono::{Duration, Utc};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
pub struct Worker {
    pub pool: PgPool,
    pub provider: Client,
    pub config: Config,
    pub metrics: std::sync::Arc<crate::telemetry::Metrics>,
    step_lock: tokio::sync::Mutex<()>,
}
#[derive(Clone)]
struct Job {
    id: String,
    tenant: String,
    kind: String,
    state: String,
    external_id: String,
    attempts: i64,
    payload: Value,
}
impl Job {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            tenant: row.try_get("tenant")?,
            kind: row.try_get("kind")?,
            state: row.try_get("state")?,
            attempts: i64::from(row.try_get::<i32, _>("attempts")?),
            external_id: row.try_get("external_id")?,
            payload: row.try_get("payload")?,
        })
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Payload {
    minor_units: String,
    return_url: String,
    external_price_ids: Vec<String>,
    timing: String,
}
/// Phase marker kept in `jobs.error_category` while a cancellation is `running`.
///
/// A deleting tenant's cancellation waits for a final check before it may contact the provider, so
/// a running cancellation is in one of two phases. Claiming a pending cancellation records
/// `NOT_SENT`; the worker overwrites it with `STARTED` immediately before the provider request.
/// Every other write to the job (`finish`) replaces the marker with a real category. Recovery
/// trusts only these markers, so any interruption before the request (lock-wait timeout, shutdown
/// dropping the step, a crash) returns the job to pending, while anything that may have reached
/// the provider is reconciled. Reconciliation claims never get a marker: they only read.
mod cancel_phase {
    pub const NOT_SENT: &str = "request_not_sent";
    pub const STARTED: &str = "request_started";
}
pub(super) fn uid(s: &str) -> Result<uuid::Uuid> {
    Ok(uuid::Uuid::parse_str(s)?)
}
impl Worker {
    pub fn new(pool: PgPool, config: Config) -> Result<Self> {
        Ok(Self {
            pool,
            provider: Client::new(config.clone())?,
            config,
            metrics: std::sync::Arc::new(crate::telemetry::Metrics::default()),
            step_lock: tokio::sync::Mutex::new(()),
        })
    }
    pub async fn recover(&self) -> Result<()> {
        // A running cancellation still marked as not sent never reached the provider, even if its
        // step was dropped by shutdown or the process died: rerun its final check instead of
        // reconciling a request that was never made, and refund the attempt the claim counted.
        sqlx::query("UPDATE jobs SET state=CASE WHEN kind IN ('email','webhook','metering') OR (kind='cancel' AND error_category=$1) THEN 'pending' ELSE 'reconcile' END,attempts=CASE WHEN kind='cancel' AND error_category=$1 THEN greatest(attempts-1,0) ELSE attempts END,error_category=CASE WHEN kind='cancel' AND error_category=$1 THEN 'final_settlement_pending' ELSE error_category END,available_at=now(),updated_at=now() WHERE state='running'").bind(cancel_phase::NOT_SENT).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn run(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) -> Result<()> {
        if *shutdown.borrow() {
            return Ok(());
        }
        // Keep shutdown in the select while a step is in flight. Dropping a
        // claimed metering transaction rolls it back; external payment intents
        // remain durable and recover as reconciliation work on the next start.
        tokio::select! {
            biased;
            _ = shutdown.changed() => Ok(()),
            result = async {
                self.recover().await?;
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    if self.step().await.is_err() {
                        tracing::error!(category="database", "background step failed");
                    }
                }
            } => result,
        }
    }

    pub async fn step(&self) -> Result<()> {
        let _lock = self.step_lock.lock().await;
        self.recover().await?;
        self.maintain(Utc::now()).await?;
        self.process_metering(metering::METERING_BATCH).await?;
        let Some(j) = self.claim_next().await? else {
            return Ok(());
        };
        self.run_claimed(&j).await
    }
    /// Claims the next ready non-metering job and marks it running. A claimed pending cancellation
    /// also records that no provider request was sent (see `cancel_phase`).
    async fn claim_next(&self) -> Result<Option<Job>> {
        let mut tx = postgres::begin(&self.pool).await?;
        let row=sqlx::query("SELECT id::text,COALESCE(tenant_id::text,'') AS tenant,kind,state,attempts,external_id,payload FROM jobs WHERE state IN ('pending','reconcile') AND kind<>'metering' AND available_at<=now() ORDER BY available_at,id LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            // End the read transaction now instead of leaving it open until the connection is reused.
            tx.commit().await?;
            return Ok(None);
        };
        let j = Job::from_row(&row)?;
        sqlx::query(
            "UPDATE jobs SET state='running',attempts=attempts+1,error_category=CASE WHEN kind='cancel' AND state='pending' THEN $2 ELSE error_category END,updated_at=now() WHERE id=$1",
        )
        .bind(uid(&j.id)?)
        .bind(cancel_phase::NOT_SENT)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(j))
    }
    /// Claims the next ready job exactly as `step` does and returns its id without running it, for
    /// integration tests that interrupt a claimed job before its body finishes.
    #[doc(hidden)]
    pub async fn claim_next_job(&self) -> Result<Option<uuid::Uuid>> {
        self.claim_next().await?.map(|j| uid(&j.id)).transpose()
    }
    /// Runs the already claimed job with the given id, for integration tests that must hold
    /// the state-transition lock between the claim and the job body. `step` cannot do that
    /// because every phase before the job body takes the same lock.
    #[doc(hidden)]
    pub async fn run_running_job(&self, id: uuid::Uuid) -> Result<()> {
        let row=sqlx::query("SELECT id::text,COALESCE(tenant_id::text,'') AS tenant,kind,state,attempts,external_id,payload FROM jobs WHERE id=$1 AND state='running'").bind(id).fetch_one(&self.pool).await?;
        let mut j = Job::from_row(&row)?;
        // The claim recorded the state the job was claimed from; tests claim pending work.
        j.state = "pending".into();
        self.run_claimed(&j).await
    }
    async fn run_claimed(&self, j: &Job) -> Result<()> {
        let request_started = std::sync::atomic::AtomicBool::new(false);
        // Finishing is outside the timed request so an ambiguous outcome stays durable.
        match tokio::time::timeout(self.config.timeout(), self.execute_job(j, &request_started))
            .await
        {
            Ok(result) => result,
            // A deleting tenant's cancellation first waits on the state-transition lock for its
            // final check. Timing out there happens before any provider request, so the outcome
            // is not ambiguous: keep the cancellation pending, refund the attempt (see `finish`)
            // and rerun the check on the next pass instead of reconciling a request never sent.
            // Shutdown and crashes in the same phase are handled by `recover` through the
            // durable `cancel_phase` marker; this branch only covers the in-process timeout.
            Err(_)
                if j.kind == "cancel"
                    && j.state != "reconcile"
                    && !request_started.load(std::sync::atomic::Ordering::SeqCst) =>
            {
                tracing::warn!(
                    kind = j.kind,
                    "cancellation timed out before any provider request"
                );
                self.finish(j, "pending", "final_settlement_pending", "")
                    .await
            }
            Err(_) => {
                self.retry(
                    j,
                    if matches!(j.kind.as_str(), "webhook" | "email") {
                        Error::Transient
                    } else {
                        Error::Ambiguous
                    },
                    j.state == "reconcile",
                )
                .await
            }
        }
    }
    /// Reports whether the subscription cancellation of a deleting tenant must keep waiting.
    ///
    /// While deleting, accepted metering work (pending, deferred, failed or running) must be rated
    /// and settled before the subscription is canceled; otherwise recovered usage would produce
    /// postpaid debt that has no subscription left to collect it.
    ///
    /// The check takes the same transaction lock as event acceptance and reads the clock only after
    /// holding it. A reporting request admitted before this check commits its metering job first,
    /// so the check sees it. A request that waits for this transaction takes its own timestamp
    /// afterwards (see `Server::execute`), finds the grace period ended and is rejected. The
    /// provider request that follows stays outside the lock. Public so integration tests can
    /// exercise the lock ordering directly.
    pub async fn deleting_cancellation_waits(&self, tenant: uuid::Uuid) -> Result<bool> {
        let grace = self.config.reporting_grace_seconds;
        tokio::time::timeout(self.config.timeout(), async {
            let mut tx = postgres::begin(&self.pool).await?;
            let cutoff = Utc::now() - Duration::seconds(grace);
            let waiting: bool = sqlx::query_scalar("SELECT deleting_at IS NOT NULL AND (deleting_at>$2 OR COALESCE((SELECT sum(amount) FROM ledger WHERE tenant_id=t.id AND kind IN ('postpaid','settlement')),0)<>0 OR EXISTS(SELECT 1 FROM jobs WHERE tenant_id=t.id AND kind='settlement' AND state<>'done') OR EXISTS(SELECT 1 FROM jobs WHERE tenant_id=t.id AND kind='metering' AND state<>'done')) FROM tenants t WHERE id=$1")
                .bind(tenant)
                .bind(cutoff)
                .fetch_one(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok::<_, anyhow::Error>(waiting)
        })
        .await?
    }
    async fn execute_job(
        &self,
        j: &Job,
        request_started: &std::sync::atomic::AtomicBool,
    ) -> Result<()> {
        if j.kind == "webhook" {
            let Some(event) = j.payload.get("event_id").and_then(Value::as_str) else {
                return self.finish(j, "failed", "invalid_payload", "").await;
            };
            if self.process_webhook(event).await.is_err() {
                return self.retry(j, Error::Transient, false).await;
            }
            return self.finish(j, "done", "", "").await;
        }
        if j.kind == "email" {
            let field = |name: &str| {
                j.payload
                    .get(name)
                    .or_else(|| j.payload.get(name.to_ascii_lowercase()))
                    .and_then(Value::as_str)
                    .unwrap_or("")
            };
            if !j.payload.is_object() {
                return self.finish(j, "failed", "invalid_payload", "").await;
            }
            return match self
                .provider
                .send_email(field("To"), field("Subject"), field("Text"))
                .await
            {
                Ok(()) => self.finish(j, "done", "", "").await,
                Err(e) => self.retry(j, e, false).await,
            };
        }
        if j.state == "reconcile" {
            return self.reconcile(j.clone()).await;
        }
        let p: Payload = match serde_json::from_value(j.payload.clone()) {
            Ok(p) => p,
            Err(_) => return self.finish(j, "failed", "invalid_payload", "").await,
        };
        let (customer, subscription): (String, String) = sqlx::query_as(
            "SELECT external_customer_id,external_subscription_id FROM tenants WHERE id=$1",
        )
        .bind(uid(&j.tenant)?)
        .fetch_one(&self.pool)
        .await?;
        let custom = json!({"delcore_job_id":j.id,"delcore_tenant_id":j.tenant});
        let mut body = json!({"custom_data":custom});
        let mut path = "/transactions".to_string();
        let mut method = Method::POST;
        match j.kind.as_str() {
            "topup" | "auto_topup" | "settlement" => {
                body["items"] = json!([{"quantity":1,"price":{"description":"Delcore usage credits","product_id":self.config.paddle_credit_product_id,"custom_data":custom,"unit_price":{"amount":p.minor_units,"currency_code":"USD"}}}]);
                if !customer.is_empty() {
                    body["customer_id"] = json!(customer)
                }
                body["collection_mode"] = json!("automatic");
                if j.kind != "topup" {
                    if subscription.is_empty() {
                        return self
                            .finish(j, "failed", "payment_method_required", "")
                            .await;
                    }
                    let (end, failed): (chrono::DateTime<Utc>, bool) =
                        sqlx::query_as("SELECT period_end,renewal_failed FROM tenants WHERE id=$1")
                            .bind(uid(&j.tenant)?)
                            .fetch_one(&self.pool)
                            .await?;
                    if failed || end <= Utc::now() + Duration::minutes(30) {
                        return self
                            .finish(j, "pending", "provider_billing_window", "")
                            .await;
                    }
                    path = format!(
                        "/subscriptions/{}/charge",
                        provider::escape_path(&subscription)
                    );
                    let object = body.as_object_mut().unwrap();
                    object.remove("custom_data");
                    object.remove("customer_id");
                    object.remove("collection_mode");
                    body["effective_from"] = json!("immediately");
                    if j.kind == "settlement" {
                        let deleting: bool = sqlx::query_scalar(
                            "SELECT deleting_at IS NOT NULL FROM tenants WHERE id=$1",
                        )
                        .bind(uid(&j.tenant)?)
                        .fetch_one(&self.pool)
                        .await?;
                        if !deleting {
                            body["effective_from"] = json!("next_billing_period")
                        }
                    }
                    if body["effective_from"] == "immediately" {
                        body["on_payment_failure"] = json!("prevent_change")
                    }
                } else {
                    body["checkout"] = json!({"url":p.return_url})
                }
            }
            "subscription" => {
                body["items"] = json!(
                    p.external_price_ids
                        .iter()
                        .map(|price| json!({"price_id":price,"quantity":1}))
                        .collect::<Vec<_>>()
                );
                if !subscription.is_empty() {
                    method = Method::PATCH;
                    path = format!("/subscriptions/{}", provider::escape_path(&subscription));
                    body["proration_billing_mode"] = json!(p.timing);
                    body["on_payment_failure"] = json!("prevent_change")
                } else {
                    body["collection_mode"] = json!("automatic");
                    body["checkout"] = json!({"url":p.return_url});
                    if !customer.is_empty() {
                        body["customer_id"] = json!(customer)
                    }
                }
            }
            "cancel" => {
                let waiting = self.deleting_cancellation_waits(uid(&j.tenant)?).await?;
                if waiting {
                    return self
                        .finish(j, "pending", "final_settlement_pending", "")
                        .await;
                }
                if subscription.is_empty() {
                    return self.finish(j, "failed", "subscription_required", "").await;
                }
                path = format!(
                    "/subscriptions/{}/cancel",
                    provider::escape_path(&subscription)
                );
                body = json!({"effective_from":"next_billing_period"})
            }
            _ => return self.finish(j, "failed", "unknown_job_kind", "").await,
        }
        if j.kind == "cancel" {
            // Persist the phase change first: an interruption after this write may hide a request
            // the provider accepted, so recovery must reconcile instead of resending.
            sqlx::query("UPDATE jobs SET error_category=$2 WHERE id=$1 AND state='running'")
                .bind(uid(&j.id)?)
                .bind(cancel_phase::STARTED)
                .execute(&self.pool)
                .await?;
        }
        // From here on a timeout may hide a request the provider already accepted.
        request_started.store(true, std::sync::atomic::Ordering::SeqCst);
        let raw = match self.provider.request(method, &path, body).await {
            Ok(v) => v,
            Err(e) => return self.retry(j, e, false).await,
        };
        let result: provider::Transaction = match serde_json::from_value(raw) {
            Ok(t) => t,
            Err(_) => return self.retry(j, Error::Ambiguous, false).await,
        };
        if result.id.is_empty() {
            return self.retry(j, Error::Ambiguous, false).await;
        }
        if j.kind == "cancel" || (j.kind == "subscription" && !subscription.is_empty()) {
            return self
                .finish(j, "reconcile", "awaiting_provider_event", &subscription)
                .await;
        }
        if !self.save_checkout_url(j, &result.checkout.url).await? {
            return self.retry(j, Error::Ambiguous, false).await;
        }
        self.finish(j, "reconcile", "", &result.id).await?;
        if result.status == "completed" {
            self.apply_transaction(result, Utc::now()).await?
        }
        Ok(())
    }
    /// Saves the provider checkout link of an unpaid payment on the job, so billing history can
    /// offer it. Returns `false` without writing when the link is nonempty but unsafe (not HTTPS,
    /// malformed or carrying credentials); the caller applies its ambiguity policy. An empty link
    /// is not an error and never clears a link that is already stored, because the provider may
    /// omit the field on a later read.
    async fn save_checkout_url(&self, j: &Job, url: &str) -> Result<bool> {
        if url.is_empty() {
            return Ok(true);
        }
        let valid = url::Url::parse(url).is_ok_and(|u| {
            u.scheme() == "https" && u.username().is_empty() && u.password().is_none()
        });
        if !valid {
            tracing::warn!(kind = j.kind, "provider checkout link is not safe to store");
            return Ok(false);
        }
        sqlx::query("UPDATE jobs SET checkout_url=$2 WHERE id=$1 AND state='running'")
            .bind(uid(&j.id)?)
            .bind(url)
            .execute(&self.pool)
            .await?;
        Ok(true)
    }
    async fn finish(&self, j: &Job, state: &str, category: &str, external: &str) -> Result<()> {
        self.finish_scan(j, state, category, external, reconcile::ScanWrite::Keep)
            .await
    }
    /// Finishes a claimed job like `finish` and updates the transaction scan checkpoint in the same
    /// statement, so progress and job state never disagree after a crash.
    async fn finish_scan(
        &self,
        j: &Job,
        state: &str,
        category: &str,
        external: &str,
        scan: reconcile::ScanWrite,
    ) -> Result<()> {
        let (mode, checkpoint) = match scan {
            reconcile::ScanWrite::Keep => ("keep", Value::Null),
            reconcile::ScanWrite::Clear => ("clear", Value::Null),
            reconcile::ScanWrite::Set(value) => ("set", value),
        };
        // A scan continues immediately; other deferrals wait for the retry interval.
        let available = if category == reconcile::SCAN_PENDING {
            Utc::now()
        } else {
            Utc::now() + Duration::seconds(self.config.retry_seconds)
        };
        tokio::time::timeout(self.config.timeout(),async{let mut tx=postgres::begin(&self.pool).await?;sqlx::query("UPDATE jobs SET payload=CASE $6::text WHEN 'clear' THEN (CASE WHEN jsonb_typeof(payload)='object' THEN payload-'reconcile_scan' ELSE payload END) WHEN 'set' THEN (CASE WHEN jsonb_typeof(payload)='object' THEN payload ELSE '{}'::jsonb END)||jsonb_build_object('reconcile_scan',$7::jsonb) ELSE payload END,state=$2,error_category=CASE WHEN $3='provider_billing_window' AND error_category='provider_rejected' THEN error_category ELSE $3 END,external_id=CASE WHEN $4='' THEN external_id ELSE $4 END,available_at=$5,attempts=CASE WHEN $3 IN ('provider_billing_window','final_settlement_pending','reconcile_scan_pending') THEN greatest(attempts-1,0) ELSE attempts END,updated_at=now() WHERE id=$1 AND state='running'").bind(uid(&j.id)?).bind(state).bind(category).bind(external).bind(available).bind(mode).bind(checkpoint).execute(&mut *tx).await?;if j.kind=="auto_topup"&&state=="failed"{sqlx::query("UPDATE tenants SET auto_suspended=true WHERE id=$1").bind(uid(&j.tenant)?).execute(&mut *tx).await?;maintenance::notify(&mut tx,&j.tenant,&format!("auto_failed:{}",j.id),"Automatic top-up failed","Automatic top-ups are suspended. Review your payment settings.",Utc::now()).await?}tx.commit().await?;Ok::<_,anyhow::Error>(())}).await?
    }
    async fn retry(&self, j: &Job, error: Error, reconcile: bool) -> Result<()> {
        let (mut state, mut category) = ("pending", "provider_transient");
        if reconcile || error == Error::Ambiguous {
            state = "reconcile";
            category = "payment_ambiguous"
        }
        if error == Error::Permanent || j.attempts + 1 >= self.config.retry_limit {
            state = "failed";
            category = "retry_exhausted";
            if error == Error::Permanent {
                category = if reconcile {
                    "unknown_payment_outcome"
                } else {
                    "provider_rejected"
                }
            }
        }
        tracing::warn!(kind = j.kind, state, category, "provider work deferred");
        self.finish(j, state, category, &j.external_id).await
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    #[tokio::test]
    async fn shutdown_cancels_in_flight_database_work() {
        let values: std::collections::HashMap<_, _> =
            dotenvy::from_read_iter(include_bytes!("../../.env.example").as_slice())
                .map(|entry| entry.unwrap())
                .collect();
        let config =
            Config::from_getter(|key| values.get(key).cloned().unwrap_or_default()).unwrap();
        // An unreachable fixture endpoint keeps recovery waiting for a connection.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://fixture:fixture@127.0.0.1:1/fixture")
            .unwrap();
        let worker = Worker::new(pool, config).unwrap();
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let run = worker.run(stopped);
        tokio::pin!(run);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut run)
                .await
                .is_err()
        );
        stop.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut run)
            .await
            .unwrap()
            .unwrap();
    }
}
