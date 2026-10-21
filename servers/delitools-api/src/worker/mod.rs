// SPDX-License-Identifier: FSL-1.1-ALv2
//! Background worker framework. API serving and background work share one process.
//!
//! The worker runs a set of [`BackgroundTask`]s, each on its own cadence taken from the live
//! operator settings. The foundation registers retention and backlog sampling. The execution layer
//! registers usage delivery, Gateway usage reconciliation and catalog price synchronization with
//! [`Worker::register`] before [`Worker::run`]. A failing run is logged, counted and retried at
//! the next tick; it never stops the other tasks.
mod retention;
mod sampler;

use crate::{platform::Platform, settings::Settings, telemetry::FailureKind};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinSet};

pub use retention::RetentionTask;
pub use sampler::SamplerTask;

/// Bounded task identifiers, used as metric labels and in logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskName {
    Retention,
    BacklogSampler,
    /// Delivery of known usage to delcore (execution layer).
    UsageDelivery,
    /// Gateway usage lookup for attempts with unknown usage (execution layer).
    UsageReconciliation,
    /// Public catalog price synchronization (execution layer).
    PriceSync,
}
impl TaskName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Retention => "retention",
            Self::BacklogSampler => "backlog_sampler",
            Self::UsageDelivery => "usage_delivery",
            Self::UsageReconciliation => "usage_reconciliation",
            Self::PriceSync => "price_sync",
        }
    }
}

/// What a task run can use. `settings` is the snapshot taken when the run started.
pub struct TaskContext {
    pub platform: Arc<Platform>,
    pub settings: Arc<Settings>,
    pub now: DateTime<Utc>,
}

#[async_trait]
pub trait BackgroundTask: Send + Sync + 'static {
    fn name(&self) -> TaskName;
    /// Delay between the end of one run and the start of the next. Evaluated after every run
    /// against the current settings, so interval changes apply without a restart.
    fn interval(&self, settings: &Settings) -> Duration;
    /// One unit of work. Errors are logged and counted; messages must not contain content.
    async fn run_once(&self, context: &TaskContext) -> anyhow::Result<()>;
}

pub struct Worker {
    platform: Arc<Platform>,
    tasks: Vec<Arc<dyn BackgroundTask>>,
}

impl Worker {
    /// A worker with the foundation tasks registered.
    pub fn new(platform: Arc<Platform>) -> Self {
        Self {
            platform,
            tasks: vec![Arc::new(RetentionTask), Arc::new(SamplerTask)],
        }
    }

    /// Add a task before [`Worker::run`]. This is the extension point of the execution layer.
    pub fn register(&mut self, task: Arc<dyn BackgroundTask>) {
        self.tasks.push(task);
    }

    pub fn task_names(&self) -> Vec<TaskName> {
        self.tasks.iter().map(|t| t.name()).collect()
    }

    /// Run until `shutdown` turns true. Returns an error only when a task panics.
    pub async fn run(&self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        let mut set = JoinSet::new();
        for task in &self.tasks {
            set.spawn(run_task(
                self.platform.clone(),
                task.clone(),
                shutdown.clone(),
            ));
        }
        let mut failure = None;
        while let Some(joined) = set.join_next().await {
            if let Err(error) = joined {
                tracing::error!(
                    category = "worker_panic",
                    "background task terminated abnormally"
                );
                failure.get_or_insert(anyhow::anyhow!("background task failed: {error}"));
                set.abort_all();
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

async fn run_task(
    platform: Arc<Platform>,
    task: Arc<dyn BackgroundTask>,
    mut shutdown: watch::Receiver<bool>,
) {
    let name = task.name();
    loop {
        if *shutdown.borrow() {
            return;
        }
        let context = TaskContext {
            platform: platform.clone(),
            settings: platform.settings(),
            now: crate::store::now(),
        };
        let started = std::time::Instant::now();
        let outcome = task.run_once(&context).await;
        let label = if outcome.is_ok() { "ok" } else { "error" };
        platform
            .metrics
            .task_runs
            .with_label_values(&[name.as_str(), label])
            .inc();
        if let Err(error) = outcome {
            platform.metrics.failure(FailureKind::Worker);
            tracing::error!(
                task = name.as_str(),
                category = "worker_run_failed",
                %error,
                "background task run failed"
            );
        } else {
            tracing::debug!(
                task = name.as_str(),
                duration_ms = started.elapsed().as_millis() as u64,
                "background task run complete"
            );
        }
        let delay = task.interval(&platform.settings());
        tokio::select! {
            _ = shutdown.changed() => {}
            _ = tokio::time::sleep(delay) => {}
        }
    }
}
