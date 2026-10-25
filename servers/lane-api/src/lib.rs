// SPDX-License-Identifier: FSL-1.1-ALv2
use serde_json::Value;
pub use server_common::{Fault, Result, canonical_id, id, now, string};
use sqlx::PgConnection;

/// Identity established by the authenticated adapter, never by a request body.
#[derive(Clone, Debug)]
pub struct Principal {
    pub account_id: String,
    pub tenant_id: String,
    pub session_id: String,
    pub device_id: String,
    pub device_name: String,
    pub origin_kind: String,
}
impl Principal {
    pub fn origin(&self) -> Value {
        serde_json::json!({"kind": self.origin_kind, "accountId": self.account_id,
            "deviceId": self.device_id, "deviceName": self.device_name})
    }
}
pub fn hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}
pub async fn account_lock(db: &mut PgConnection, account: &str) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(account)
        .execute(db)
        .await?;
    Ok(())
}

pub mod config;
pub mod metrics;
pub mod rate;

pub mod auth;
pub mod control;
pub mod entities;
pub mod error;
pub mod http;
pub mod mcp;
pub mod migration;
pub mod push;
pub mod rpc;
pub mod sync;
pub mod webhooks;

#[derive(Clone)]
pub struct Server {
    pub db: sqlx::PgPool,
    pub auth: std::sync::Arc<auth::Auth>,
    pub config: std::sync::Arc<config::Config>,
    pub metrics: std::sync::Arc<metrics::Metrics>,
    pub push: std::sync::Arc<push::Push>,
    pub webhooks: std::sync::Arc<webhooks::WebhookConfig>,
}
impl Server {
    pub fn max_request_bytes(&self) -> usize {
        self.config
            .max_item_bytes
            .saturating_mul(1000)
            .saturating_add(65536)
    }
}

/// Clear only Lane content in the account deletion transaction; identity and deletion directives remain.
pub async fn clear_account_content(db: &mut PgConnection, account: &str) -> Result<()> {
    webhooks::delete_account(db, account).await?;
    for table in [
        "lane_push_deliveries",
        "lane_push_registrations",
        "lane_entity_receipts",
        "lane_version_identities",
        "lane_entity_cursors",
        "lane_repository_mappings",
        "lane_entities",
        "lane_changes",
        "lane_sync_rejections",
        "lane_control_receipts",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE account_id=$1"))
            .bind(account)
            .execute(&mut *db)
            .await?;
    }
    Ok(())
}
