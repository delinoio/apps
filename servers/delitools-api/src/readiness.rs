// SPDX-License-Identifier: FSL-1.1-ALv2
//! Readiness rule: the service is ready only when the database schema is compatible and the
//! catalog holds at least one synchronized price. Later price refresh failures preserve readiness
//! because the last known prices stay in use without an age limit. Layers add further checks with
//! [`crate::platform::Platform::add_readiness_check`].
use crate::store;
use async_trait::async_trait;
use sqlx::PgPool;

#[async_trait]
pub trait ReadinessCheck: Send + Sync {
    /// Stable lowercase identifier used only in logs.
    fn name(&self) -> &'static str;
    async fn ready(&self, pool: &PgPool) -> bool;
}

/// The embedded migrations are applied and unchanged.
pub struct SchemaReadiness;
#[async_trait]
impl ReadinessCheck for SchemaReadiness {
    fn name(&self) -> &'static str {
        "schema"
    }
    async fn ready(&self, pool: &PgPool) -> bool {
        store::check_schema(pool).await.is_ok()
    }
}

/// At least one model currently has a catalog price. The price synchronization task of the execution layer
/// populates `catalog_prices`; until a synchronization succeeds, no paid execution is possible and
/// the service reports not ready.
pub struct CatalogPriceReadiness;
#[async_trait]
impl ReadinessCheck for CatalogPriceReadiness {
    fn name(&self) -> &'static str {
        "catalog_price"
    }
    async fn ready(&self, pool: &PgPool) -> bool {
        sqlx::query_scalar::<_, bool>(&format!(
            "SELECT {}",
            crate::store::exec::PRICE_AVAILABLE_SQL
        ))
        .fetch_one(pool)
        .await
        .unwrap_or(false)
    }
}
