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
#[derive(Default, Deserialize)]
#[serde(default)]
struct Payload {
    minor_units: String,
    return_url: String,
    external_price_ids: Vec<String>,
    timing: String,
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
        sqlx::query("UPDATE jobs SET state=CASE WHEN kind IN ('email','webhook','metering') THEN 'pending' ELSE 'reconcile' END,available_at=now(),updated_at=now() WHERE state='running'").execute(&self.pool).await?;
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
        let mut tx = postgres::begin(&self.pool).await?;
        let row=sqlx::query("SELECT id::text,COALESCE(tenant_id::text,'') AS tenant,kind,state,attempts,external_id,payload FROM jobs WHERE state IN ('pending','reconcile') AND kind<>'metering' AND available_at<=now() ORDER BY available_at,id LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await?;
        let Some(row) = row else { return Ok(()) };
        let j = Job {
            id: row.try_get("id")?,
            tenant: row.try_get("tenant")?,
            kind: row.try_get("kind")?,
            state: row.try_get("state")?,
            attempts: i64::from(row.try_get::<i32, _>("attempts")?),
            external_id: row.try_get("external_id")?,
            payload: row.try_get("payload")?,
        };
        sqlx::query(
            "UPDATE jobs SET state='running',attempts=attempts+1,updated_at=now() WHERE id=$1",
        )
        .bind(uid(&j.id)?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        // Finishing is outside the timed request so an ambiguous outcome stays durable.
        match tokio::time::timeout(self.config.timeout(), self.execute_job(&j)).await {
            Ok(result) => result,
            Err(_) => {
                self.retry(
                    &j,
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
    async fn execute_job(&self, j: &Job) -> Result<()> {
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
        if !result.checkout.url.is_empty() {
            let valid = url::Url::parse(&result.checkout.url).is_ok_and(|u| {
                u.scheme() == "https" && u.username().is_empty() && u.password().is_none()
            });
            if !valid {
                return self.retry(j, Error::Ambiguous, false).await;
            }
            sqlx::query("UPDATE jobs SET checkout_url=$2 WHERE id=$1")
                .bind(uid(&j.id)?)
                .bind(&result.checkout.url)
                .execute(&self.pool)
                .await?;
        }
        self.finish(j, "reconcile", "", &result.id).await?;
        if result.status == "completed" {
            self.apply_transaction(result, Utc::now()).await?
        }
        Ok(())
    }
    async fn finish(&self, j: &Job, state: &str, category: &str, external: &str) -> Result<()> {
        tokio::time::timeout(self.config.timeout(),async{let mut tx=postgres::begin(&self.pool).await?;sqlx::query("UPDATE jobs SET state=$2,error_category=CASE WHEN $3='provider_billing_window' AND error_category='provider_rejected' THEN error_category ELSE $3 END,external_id=CASE WHEN $4='' THEN external_id ELSE $4 END,available_at=$5,attempts=CASE WHEN $3 IN ('provider_billing_window','final_settlement_pending') THEN greatest(attempts-1,0) ELSE attempts END,updated_at=now() WHERE id=$1 AND state='running'").bind(uid(&j.id)?).bind(state).bind(category).bind(external).bind(Utc::now()+Duration::seconds(self.config.retry_seconds)).execute(&mut *tx).await?;if j.kind=="auto_topup"&&state=="failed"{sqlx::query("UPDATE tenants SET auto_suspended=true WHERE id=$1").bind(uid(&j.tenant)?).execute(&mut *tx).await?;maintenance::notify(&mut tx,&j.tenant,&format!("auto_failed:{}",j.id),"Automatic top-up failed","Automatic top-ups are suspended. Review your payment settings.",Utc::now()).await?}tx.commit().await?;Ok::<_,anyhow::Error>(())}).await?
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
