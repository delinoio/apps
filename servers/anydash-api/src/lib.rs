// SPDX-License-Identifier: FSL-1.1-ALv2
//! AnyDash Connect API and background worker.
use std::sync::Arc;

pub mod audit;
pub mod auth;
pub mod call;
pub mod config;
pub mod cost;
pub mod delcore;
pub mod error;
pub mod http;
pub mod metrics;
pub mod migration;
pub mod pricing;
pub mod rate;
pub mod rpc;
pub mod time;
pub mod usage;
pub mod validate;
pub mod worker;

pub use error::{ApiError, Result};

/// Response header carrying the server-generated correlation ID of every API response.
pub const CORRELATION_HEADER: &str = "x-anydash-correlation-id";
/// Maximum accepted request body.
pub const MAX_BODY_BYTES: usize = 1 << 20;

#[derive(Clone)]
pub struct Server {
    pub db: sqlx::PgPool,
    pub config: Arc<config::Config>,
    pub verifier: Arc<auth::Verifier>,
    pub delcore: Arc<delcore::Delcore>,
    pub metrics: Arc<metrics::Metrics>,
    pub pricing: Arc<pricing::Fetcher>,
}

impl Server {
    pub fn new(db: sqlx::PgPool, config: config::Config) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.dependency_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let verifier = auth::Verifier::new(http.clone(), &config.logto_issuer);
        let delcore = delcore::Delcore::new(http, &config);
        Ok(Self {
            db,
            verifier: Arc::new(verifier),
            delcore: Arc::new(delcore),
            metrics: Arc::new(metrics::Metrics::new()),
            pricing: Arc::new(pricing::Fetcher::new(&config.pricing_url)?),
            config: Arc::new(config),
        })
    }
}

/// Lowercase hex SHA-256 digest.
pub fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// A new UUID v7.
pub fn new_id() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}
