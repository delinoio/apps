// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{BackgroundTask, TaskContext, TaskName};
use crate::{settings::Settings, store::recovery};
use async_trait::async_trait;
use std::time::Duration;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(15);

/// Samples the pending, unknown and near-expiry backlog into gauges and logs a structured warning
/// when an operator-configured alert threshold is reached. Labels stay bounded; no tenant or
/// request identifiers are emitted.
pub struct SamplerTask;

#[async_trait]
impl BackgroundTask for SamplerTask {
    fn name(&self) -> TaskName {
        TaskName::BacklogSampler
    }
    fn interval(&self, _settings: &Settings) -> Duration {
        SAMPLE_INTERVAL
    }
    async fn run_once(&self, context: &TaskContext) -> anyhow::Result<()> {
        let platform = &context.platform;
        let backlog = recovery::backlog(
            &platform.store.pool,
            context.now,
            context.settings.alert_expiry_seconds,
        )
        .await
        .map_err(|_| anyhow::anyhow!("backlog sampling failed"))?;
        let (backlog_alert, expiry_alert) = platform
            .metrics
            .set_backlog(&backlog, context.settings.alert_backlog);
        if backlog_alert {
            tracing::warn!(
                category = "backlog_alert",
                unknown_usage = backlog.unknown_usage,
                pending_delivery = backlog.pending_delivery,
                threshold = context.settings.alert_backlog,
                "pending usage backlog reached the alert threshold"
            );
        }
        if expiry_alert {
            tracing::warn!(
                category = "expiry_alert",
                nearing_expiry = backlog.nearing_expiry,
                window_seconds = context.settings.alert_expiry_seconds,
                "undelivered usage records are nearing the 30-day expiration"
            );
        }
        Ok(())
    }
}
