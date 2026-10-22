// SPDX-License-Identifier: FSL-1.1-ALv2
//! Verification that a model's input and output meters have registered selling prices.
//!
//! DeliTools never reads Gateway prices and never writes Delcore prices. Before executing, it
//! confirms through Delcore's generic catalog that both meters of the model have an active usage
//! price for the DeliTools service. A missing price rejects the request; no other model is
//! substituted. Successful reads are cached briefly to bound catalog traffic.

use crate::{
    delcore::{DelcoreClient, DelcoreError},
    error::ApiError,
    models::ModelEntry,
};
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

/// Minimum age of a cached price list before a miss triggers a refresh.
const MISS_REFRESH: Duration = Duration::from_secs(5);

pub struct PriceCheck {
    delcore: Arc<DelcoreClient>,
    ttl: Duration,
    cache: Mutex<Option<(HashSet<String>, Instant)>>,
}

impl PriceCheck {
    pub fn new(delcore: Arc<DelcoreClient>, ttl: Duration) -> Self {
        Self {
            delcore,
            ttl,
            cache: Mutex::new(None),
        }
    }

    /// Fails closed: an unreadable catalog is `DEPENDENCY_UNAVAILABLE`, a missing price is
    /// `MODEL_UNAVAILABLE`.
    pub async fn verify(&self, model: &ModelEntry) -> Result<(), ApiError> {
        let mut cache = self.cache.lock().await;
        let priced = |set: &HashSet<String>| {
            set.contains(&model.input_meter_key) && set.contains(&model.output_meter_key)
        };
        if let Some((set, at)) = &*cache {
            let age = at.elapsed();
            if age < self.ttl && (priced(set) || age < MISS_REFRESH) {
                return if priced(set) {
                    Ok(())
                } else {
                    Err(ApiError::model_unpriced())
                };
            }
        }
        let fresh = self.delcore.priced_meters().await.map_err(|error| {
            tracing::error!(category = "price_check", failure = ?error_label(error), "the Delcore catalog could not be read");
            ApiError::dependency_unavailable()
        })?;
        let result = if priced(&fresh) {
            Ok(())
        } else {
            Err(ApiError::model_unpriced())
        };
        *cache = Some((fresh, Instant::now()));
        result
    }
}

fn error_label(error: DelcoreError) -> &'static str {
    match error {
        DelcoreError::Unauthenticated => "unauthenticated",
        DelcoreError::PermissionDenied => "permission_denied",
        DelcoreError::Unavailable => "unavailable",
    }
}
