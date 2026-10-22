// SPDX-License-Identifier: FSL-1.1-ALv2
//! Usage delivery, Gateway usage reconciliation and public catalog price synchronization.
//!
//! * **Delivery.** Each measured attempt is reported to delcore with `IngestEvent`, using the
//!   attempt UUID as request key and event key. It is sent right after its request ends and then
//!   retried by a background task with capped exponential backoff until the 30-day record
//!   expires. A repeated delivery of the same attempt is answered by delcore from its retained
//!   identity, so a lost response or a restart never creates a second ledger effect.
//! * **Reconciliation.** An attempt whose usage is unknown (cancelled, timed out or an incomplete
//!   stream) is looked up at the Gateway by generation ID. Usage is only ever taken from the
//!   Gateway's measured counts; nothing is estimated.
//! * **Price synchronization.** The public catalog price of every allowlisted translation model
//!   and of the fixed extraction model is stored as an effective-dated version. A failure keeps
//!   the last known prices indefinitely.
use crate::{
    delcore::DelcoreAuthority,
    gateway::{CatalogPrice, EXTRACTION_MODEL, Endpoint, Gateway, Lookup},
    platform::Platform,
    pricing,
    store::{
        self,
        audit::{self, EventCategory, Severity},
        exec::{self as db, DueAttempt, PriceWrite, Work},
    },
    telemetry::FailureKind,
    worker::{BackgroundTask, TaskContext, TaskName},
};
use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use uuid::Uuid;

/// Attempts claimed per batch and batches per task run.
const BATCH: i64 = 50;
const MAX_BATCHES: usize = 20;

pub struct UsageService {
    platform: Arc<Platform>,
    gateway: Arc<Gateway>,
    delcore: Arc<DelcoreAuthority>,
    service_id: Uuid,
}

impl UsageService {
    pub fn new(
        platform: Arc<Platform>,
        gateway: Arc<Gateway>,
        delcore: Arc<DelcoreAuthority>,
        service_id: Uuid,
    ) -> Arc<Self> {
        Arc::new(Self {
            platform,
            gateway,
            delcore,
            service_id,
        })
    }

    /// A claim outlives one report round trip, so the periodic task and an immediate delivery
    /// never send the same attempt concurrently.
    fn lease(&self) -> ChronoDuration {
        ChronoDuration::from_std(self.platform.config.timeout() * 2 + Duration::from_secs(30))
            .unwrap_or_else(|_| ChronoDuration::seconds(150))
    }

    /// Deliver the known usage of a finished request right away, in the background.
    pub fn deliver_request(self: &Arc<Self>, request_id: Uuid) {
        let service = self.clone();
        tokio::spawn(async move {
            let pool = &service.platform.store.pool;
            let Ok(ids) = db::deliverable_attempts(pool, request_id).await else {
                return;
            };
            for id in ids {
                if let Ok(Some(attempt)) =
                    db::claim_one(pool, id, store::now(), service.lease()).await
                {
                    service.deliver(attempt).await;
                }
            }
        });
    }

    /// Send one claimed attempt and record the result.
    async fn deliver(&self, attempt: DueAttempt) {
        let platform = &self.platform;
        let pool = &platform.store.pool;
        let settings = platform.settings();
        let now = store::now();
        match self.delcore.report(self.service_id, &attempt).await {
            Ok(()) => {
                if db::mark_delivered(pool, attempt.id, now).await.is_ok() {
                    platform
                        .metrics
                        .usage_deliveries
                        .with_label_values(&["delivered"])
                        .inc();
                    tracing::info!(attempt_id = %attempt.id, "usage delivered");
                }
            }
            Err(error) => {
                let failures = attempt.delivery_failures.saturating_add(1);
                let delay = pricing::backoff(
                    settings.delivery_retry_seconds,
                    settings.max_backoff_seconds,
                    attempt.delivery_failures,
                );
                let _ = db::mark_retry(
                    pool,
                    attempt.id,
                    failures,
                    now + ChronoDuration::from_std(delay)
                        .unwrap_or_else(|_| ChronoDuration::hours(1)),
                    now,
                )
                .await;
                platform.metrics.failure(FailureKind::UsageDelivery);
                platform
                    .metrics
                    .usage_deliveries
                    .with_label_values(&["failed"])
                    .inc();
                tracing::warn!(
                    category = "usage_delivery_failed",
                    attempt_id = %attempt.id,
                    cause = error.category(),
                    failures,
                    retry_in_seconds = delay.as_secs(),
                    "usage delivery failed; it will be retried until the record expires"
                );
                // Record the first failure and then every tenth, not each retry.
                if failures == 1 || failures.is_multiple_of(10) {
                    audit::record_event(
                        pool,
                        EventCategory::UsageDeliveryFailure,
                        Severity::Warn,
                        None,
                        attempt.id,
                        serde_json::json!({"cause": error.category(), "failures": failures}),
                    )
                    .await;
                }
            }
        }
    }

    /// Deliver every due attempt. Called by the periodic task.
    pub async fn run_deliveries(&self) -> anyhow::Result<()> {
        let pool = &self.platform.store.pool;
        for _ in 0..MAX_BATCHES {
            let due = db::claim_due(pool, Work::Delivery, store::now(), self.lease(), BATCH)
                .await
                .map_err(|_| anyhow::anyhow!("due usage could not be claimed"))?;
            if due.is_empty() {
                break;
            }
            for attempt in due {
                self.deliver(attempt).await;
            }
        }
        Ok(())
    }

    /// Look up every due unknown-usage attempt at the Gateway. A confirmed lookup stores the
    /// measured counts and delivers them at once; anything else is retried with backoff.
    pub async fn run_reconciliation(&self) -> anyhow::Result<()> {
        let platform = &self.platform;
        let pool = &platform.store.pool;
        for _ in 0..MAX_BATCHES {
            let due = db::claim_due(
                pool,
                Work::Reconciliation,
                store::now(),
                self.lease(),
                BATCH,
            )
            .await
            .map_err(|_| anyhow::anyhow!("unknown usage could not be claimed"))?;
            if due.is_empty() {
                break;
            }
            for attempt in due {
                self.reconcile(attempt).await;
            }
        }
        Ok(())
    }

    async fn reconcile(&self, attempt: DueAttempt) {
        let platform = &self.platform;
        let pool = &platform.store.pool;
        let settings = platform.settings();
        let outcome = match self.gateway.generation(&attempt.generation_id).await {
            Ok(Lookup::Usage { model, tokens }) if model == attempt.model => Ok(tokens),
            Ok(Lookup::NotYet) => Err("not_yet"),
            Ok(_) => Err("unreadable"),
            Err(_) => Err("unavailable"),
        };
        // Read the clock after the lookup: the network call can cross the attempt's 30-day
        // boundary, and the storage updates below only accept an attempt still inside it.
        let now = store::now();
        match outcome {
            Ok(tokens) => {
                platform
                    .metrics
                    .reconciliations
                    .with_label_values(&["confirmed"])
                    .inc();
                tracing::info!(attempt_id = %attempt.id, "unknown usage confirmed by the Gateway");
                match db::set_measured(pool, attempt.id, &tokens, now).await {
                    Ok(true) => {
                        if let Ok(Some(confirmed)) =
                            db::claim_one(pool, attempt.id, now, self.lease()).await
                        {
                            self.deliver(confirmed).await;
                        }
                    }
                    _ => tracing::warn!(
                        attempt_id = %attempt.id,
                        "confirmed usage was not stored; the attempt expired, was already settled or storage failed"
                    ),
                }
            }
            Err(cause) => {
                let failures = attempt.delivery_failures.saturating_add(1);
                let delay = pricing::backoff(
                    settings.delivery_retry_seconds,
                    settings.max_backoff_seconds,
                    attempt.delivery_failures,
                );
                let _ = db::mark_retry(
                    pool,
                    attempt.id,
                    failures,
                    now + ChronoDuration::from_std(delay)
                        .unwrap_or_else(|_| ChronoDuration::hours(1)),
                    now,
                )
                .await;
                platform
                    .metrics
                    .reconciliations
                    .with_label_values(&[cause])
                    .inc();
                if cause != "not_yet" {
                    platform.metrics.failure(FailureKind::Provider);
                }
                tracing::info!(
                    category = "reconciliation_pending",
                    attempt_id = %attempt.id,
                    cause,
                    failures,
                    "Gateway usage is not confirmed yet; it will be looked up again"
                );
            }
        }
    }
}

pub struct DeliveryTask(pub Arc<UsageService>);

#[async_trait]
impl BackgroundTask for DeliveryTask {
    fn name(&self) -> TaskName {
        TaskName::UsageDelivery
    }
    fn interval(&self, settings: &crate::settings::Settings) -> Duration {
        Duration::from_secs(u64::from(settings.delivery_retry_seconds))
    }
    async fn run_once(&self, _context: &TaskContext) -> anyhow::Result<()> {
        self.0.run_deliveries().await
    }
}

pub struct ReconciliationTask(pub Arc<UsageService>);

#[async_trait]
impl BackgroundTask for ReconciliationTask {
    fn name(&self) -> TaskName {
        TaskName::UsageReconciliation
    }
    fn interval(&self, settings: &crate::settings::Settings) -> Duration {
        Duration::from_secs(u64::from(settings.delivery_retry_seconds))
    }
    async fn run_once(&self, _context: &TaskContext) -> anyhow::Result<()> {
        self.0.run_reconciliation().await
    }
}

/// Synchronizes the public catalog prices of the models DeliTools can execute.
pub struct PriceSyncTask {
    pub gateway: Arc<Gateway>,
}

/// The unchanged-price revision digest stored with each version.
fn revision(route: &str, price: &CatalogPrice) -> String {
    let mut hasher = Sha256::new();
    for part in [
        route,
        &price.input,
        &price.output,
        price.cache_read.as_deref().unwrap_or("-"),
        price.cache_write.as_deref().unwrap_or("-"),
    ] {
        hasher.update(part.len().to_string());
        hasher.update(b":");
        hasher.update(part);
    }
    hex::encode(hasher.finalize())
}

/// The provider route a flat public price can be pinned to: the alphabetically first active
/// endpoint whose own price equals the public one. An endpoint with context tiers never matches,
/// because requests are billed from the flat model snapshot and would be mispriced once usage
/// crossed an endpoint tier.
fn flat_route(endpoints: &[Endpoint], price: &CatalogPrice) -> Option<String> {
    endpoints
        .iter()
        .filter(|endpoint| {
            endpoint.active
                && endpoint.price.as_ref().is_some_and(|p| {
                    !p.tiered
                        && p.input == price.input
                        && p.output == price.output
                        && p.cache_read == price.cache_read
                        && p.cache_write == price.cache_write
                })
        })
        .map(|endpoint| endpoint.provider.as_str())
        .min()
        .map(str::to_owned)
}

#[async_trait]
impl BackgroundTask for PriceSyncTask {
    fn name(&self) -> TaskName {
        TaskName::PriceSync
    }
    fn interval(&self, settings: &crate::settings::Settings) -> Duration {
        Duration::from_secs(u64::from(settings.price_sync_seconds))
    }

    async fn run_once(&self, context: &TaskContext) -> anyhow::Result<()> {
        let platform = &context.platform;
        let pool = &platform.store.pool;
        let correlation = Uuid::now_v7();
        let fail = |cause: &'static str| {
            platform.metrics.failure(FailureKind::CatalogSync);
            platform
                .metrics
                .price_syncs
                .with_label_values(&["failed"])
                .inc();
            tracing::warn!(
                category = "price_sync_failed",
                correlation_id = %correlation,
                cause,
                "catalog price synchronization failed; last known prices stay in use"
            );
            anyhow::anyhow!("catalog price synchronization failed")
        };
        let catalog = match self.gateway.models().await {
            Ok(catalog) => catalog,
            Err(_) => {
                audit::record_event(
                    pool,
                    EventCategory::PriceSyncFailure,
                    Severity::Warn,
                    None,
                    correlation,
                    serde_json::json!({"cause": "catalog_unavailable"}),
                )
                .await;
                return Err(fail("catalog_unavailable"));
            }
        };
        // Catalog synchronization never enables a model: only allowlisted models and the fixed
        // extraction model are priced.
        let mut wanted: BTreeSet<&str> = context
            .settings
            .translation_models
            .iter()
            .map(String::as_str)
            .collect();
        wanted.insert(EXTRACTION_MODEL);
        let (mut changed, mut refreshed, mut skipped, mut failed) = (0, 0, 0, 0);
        for model in wanted {
            let Some(price) = catalog
                .iter()
                .find(|entry| entry.id == model && entry.language)
                .and_then(|entry| entry.price.as_ref())
            else {
                skipped += 1;
                tracing::warn!(
                    model,
                    category = "price_not_published",
                    "no public price for model"
                );
                continue;
            };
            if price.tiered {
                // Context-tiered pricing has no single flat price; the model stays unpriced.
                skipped += 1;
                tracing::warn!(
                    model,
                    category = "price_tiered",
                    "tiered pricing is not supported"
                );
                continue;
            }
            let route = match self.gateway.endpoints(model).await {
                Ok(endpoints) => match flat_route(&endpoints, price) {
                    Some(route) => route,
                    None => {
                        // No active provider charges the published price: the route and
                        // therefore the price would be ambiguous, so do not price the model.
                        skipped += 1;
                        tracing::warn!(
                            model,
                            category = "price_route_ambiguous",
                            "no active provider endpoint matches the public price"
                        );
                        continue;
                    }
                },
                Err(_) => {
                    failed += 1;
                    continue;
                }
            };
            match db::record_price(
                pool,
                model,
                &route,
                price,
                &revision(&route, price),
                store::now(),
            )
            .await
            {
                Ok(PriceWrite::NewVersion) => changed += 1,
                Ok(PriceWrite::Refreshed) => refreshed += 1,
                Err(_) => failed += 1,
            }
        }
        tracing::info!(
            correlation_id = %correlation,
            changed,
            refreshed,
            skipped,
            failed,
            "catalog price synchronization finished"
        );
        if failed > 0 {
            audit::record_event(
                pool,
                EventCategory::PriceSyncFailure,
                Severity::Warn,
                None,
                correlation,
                serde_json::json!({"cause": "partial", "failed": failed}),
            )
            .await;
            return Err(fail("partial"));
        }
        platform
            .metrics
            .price_syncs
            .with_label_values(&["ok"])
            .inc();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_changes_with_any_price_component() {
        let price = CatalogPrice {
            input: "1".into(),
            output: "2".into(),
            cache_read: None,
            cache_write: Some("3".into()),
            tiered: false,
        };
        let base = revision("openai", &price);
        assert_eq!(base, revision("openai", &price));
        assert_ne!(base, revision("azure", &price));
        for changed in [
            CatalogPrice {
                input: "9".into(),
                ..price.clone()
            },
            CatalogPrice {
                output: "9".into(),
                ..price.clone()
            },
            CatalogPrice {
                cache_read: Some("9".into()),
                ..price.clone()
            },
            CatalogPrice {
                cache_write: None,
                ..price.clone()
            },
        ] {
            assert_ne!(base, revision("openai", &changed));
        }
    }

    fn price(tiered: bool) -> CatalogPrice {
        CatalogPrice {
            input: "0.000003".into(),
            output: "0.000015".into(),
            cache_read: Some("0.0000003".into()),
            cache_write: None,
            tiered,
        }
    }
    fn endpoint(provider: &str, active: bool, price: Option<CatalogPrice>) -> Endpoint {
        Endpoint {
            provider: provider.into(),
            active,
            price,
        }
    }

    #[test]
    fn the_first_active_flat_endpoint_with_the_public_price_is_pinned() {
        let public = price(false);
        let endpoints = [
            endpoint("zeta", true, Some(price(false))),
            endpoint("alpha", false, Some(price(false))),
            endpoint(
                "beta",
                true,
                Some(CatalogPrice {
                    input: "0.000004".into(),
                    ..price(false)
                }),
            ),
            endpoint("gamma", true, None),
            endpoint("eta", true, Some(price(false))),
        ];
        assert_eq!(flat_route(&endpoints, &public).as_deref(), Some("eta"));
        assert_eq!(flat_route(&[], &public), None);
    }

    #[test]
    fn a_tiered_endpoint_never_matches_a_flat_public_price() {
        let public = price(false);
        let tiered_only = [endpoint("alpha", true, Some(price(true)))];
        assert_eq!(flat_route(&tiered_only, &public), None);
        // A flat endpoint is still chosen even when an earlier provider has tiers.
        let mixed = [
            endpoint("alpha", true, Some(price(true))),
            endpoint("beta", true, Some(price(false))),
        ];
        assert_eq!(flat_route(&mixed, &public).as_deref(), Some("beta"));
    }
}
