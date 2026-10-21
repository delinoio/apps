// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{BackgroundTask, TaskContext, TaskName};
use crate::{
    settings::Settings,
    store::{
        audit::{self, EventCategory, Severity},
        retention,
    },
};
use async_trait::async_trait;
use std::time::Duration;

/// Cadence of retention sweeps. This is an internal maintenance interval, not a product limit:
/// records live at most this long past the 30-day boundary.
const SWEEP_INTERVAL: Duration = Duration::from_secs(300);

/// Deletes idempotency records, attempts, requests, audit records and operational events 30 days
/// after their immutable creation time, regardless of processing state.
pub struct RetentionTask;

#[async_trait]
impl BackgroundTask for RetentionTask {
    fn name(&self) -> TaskName {
        TaskName::Retention
    }
    fn interval(&self, _settings: &Settings) -> Duration {
        SWEEP_INTERVAL
    }
    async fn run_once(&self, context: &TaskContext) -> anyhow::Result<()> {
        let platform = &context.platform;
        let report = retention::sweep(&platform.store.pool, context.now)
            .await
            .map_err(|_| anyhow::anyhow!("retention sweep failed"))?;
        platform.metrics.retention_runs.inc();
        for (table, deleted) in &report.deleted {
            platform
                .metrics
                .retention_deleted
                .with_label_values(&[table.as_str()])
                .inc_by(*deleted);
        }
        if report.total() > 0 {
            let counts: serde_json::Map<_, _> = report
                .deleted
                .iter()
                .map(|(table, n)| (table.as_str().to_owned(), serde_json::json!(n)))
                .collect();
            tracing::info!(deleted = report.total(), "expired records deleted");
            audit::record_event(
                &platform.store.pool,
                EventCategory::RetentionSweep,
                Severity::Info,
                None,
                uuid::Uuid::now_v7(),
                serde_json::Value::Object(counts),
            )
            .await;
        }
        Ok(())
    }
}
