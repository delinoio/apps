// SPDX-License-Identifier: FSL-1.1-ALv2
//! Provider execution: attempts, retries, cancellation and usage persistence.
//!
//! An execution runs in its own task so that a caller disconnect cannot interrupt the steps that
//! must complete: cancellation aborts only the in-flight provider call and further retries, while
//! usage already confirmed is stored before the task ends. The task keeps its tenant permit until
//! it finishes.
//!
//! Attempt loop for one request:
//!
//! 1. For attempts after the first, wait the backoff and re-run the authorization check; a
//!    revoked key, lost permission or billing block ends the request.
//! 2. Call the Gateway, racing the call against cancellation and the execution deadline. A stop
//!    is counted as a `canceled` or `timeout` Gateway attempt only if the call had started; stops
//!    during backoff, the authorization recheck or before the first poll are not attempts.
//! 3. A transient failure (network, 5xx, 429) retries until the retry budget is spent.
//! 4. A completed response with confirmed usage is stored in the outbox *before* its content is
//!    interpreted, so failed, invalid or canceled-after-response attempts are still billed.
//!    Missing or invalid usage is counted and logged, never estimated.
//! 5. Output is validated; invalid output is final (no repair retry, no model substitution).

use crate::{
    Inner,
    auth::Principal,
    error::ApiError,
    extract::Extraction,
    gateway::{ChatRequest, Completion},
    limits::Permit,
    models::ModelEntry,
    retry::Failure,
    telemetry::Procedure,
    translate::{self, Translation},
    usage::{self, UsageReading},
};
use chrono::{DateTime, Utc};
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use uuid::Uuid;

/// A validated unit of work.
pub enum Task {
    Translate(Translation),
    Extract(Extraction),
}

/// Successful result of a task.
#[derive(Debug, PartialEq, Eq)]
pub enum Output {
    Translations(Vec<String>),
    Json(String),
}

impl Task {
    pub fn procedure(&self) -> Procedure {
        match self {
            Self::Translate(_) => Procedure::Translate,
            Self::Extract(_) => Procedure::Extract,
        }
    }
    fn parse(&self, content: &str) -> Result<Output, ApiError> {
        match self {
            Self::Translate(t) => {
                translate::parse_output(content, t.texts.len()).map(Output::Translations)
            }
            Self::Extract(e) => e.parse_output(content).map(Output::Json),
        }
    }
}

/// Everything an execution needs. Dropping it releases the tenant permit.
pub struct Job {
    pub request_id: Uuid,
    pub principal: Principal,
    pub task: Task,
    pub model: ModelEntry,
    pub started_at: DateTime<Utc>,
    pub deadline: Instant,
    pub cancel: CancellationToken,
    pub permit: Permit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Canceled,
    TimedOut,
}

/// A provider call that was stopped by cancellation or the execution deadline.
struct Interrupted {
    stop: Stop,
    /// True only if the provider future was polled at least once, so a provider call may have been
    /// issued. A stop that wins before the first poll (for example an already-canceled or expired
    /// job) never started an attempt.
    started: bool,
}

/// Races one provider call against cancellation and the execution deadline.
///
/// Constructing the call future does not start a provider attempt; the attempt starts when the
/// future is first polled. A thin wrapper records that first poll, and stops that are already
/// pending before it are reported without `started`. The call future is dropped when a stop wins.
async fn race_provider<F: Future>(
    cancel: &CancellationToken,
    deadline: Instant,
    call: F,
) -> Result<F::Output, Interrupted> {
    // An atomic keeps the execution future `Send`; the flag is only used from this task.
    let started = AtomicBool::new(false);
    if cancel.is_cancelled() {
        return Err(Interrupted {
            stop: Stop::Canceled,
            started: false,
        });
    }
    if Instant::now() >= deadline {
        return Err(Interrupted {
            stop: Stop::TimedOut,
            started: false,
        });
    }
    let call = async {
        started.store(true, Ordering::Relaxed);
        call.await
    };
    tokio::select! {
        _ = cancel.cancelled() => Err(Interrupted { stop: Stop::Canceled, started: started.load(Ordering::Relaxed) }),
        _ = tokio::time::sleep_until(deadline) => Err(Interrupted { stop: Stop::TimedOut, started: started.load(Ordering::Relaxed) }),
        result = call => Ok(result),
    }
}

impl Inner {
    /// Sleeps for `wait` unless canceled or past the deadline.
    async fn pause(&self, job: &Job, wait: Duration) -> Result<(), Stop> {
        tokio::select! {
            _ = job.cancel.cancelled() => Err(Stop::Canceled),
            _ = tokio::time::sleep_until(job.deadline) => Err(Stop::TimedOut),
            _ = tokio::time::sleep(wait) => Ok(()),
        }
    }

    /// The terminal error of a stopped request. Pure: Gateway attempt metrics are recorded only
    /// by [`Inner::provider_attempt`], because a stop between attempts is not a provider attempt.
    fn stopped(&self, stop: Stop) -> ApiError {
        match stop {
            Stop::Canceled => ApiError::canceled(),
            Stop::TimedOut => ApiError::timeout(),
        }
    }

    /// Runs one Gateway call and counts it as `canceled` or `timeout` only when the call was
    /// interrupted after it had started (see [`race_provider`]).
    async fn provider_attempt(
        &self,
        job: &Job,
        request: &ChatRequest,
    ) -> Result<Result<Completion, Failure>, ApiError> {
        match race_provider(&job.cancel, job.deadline, self.gateway.complete(request)).await {
            Ok(outcome) => Ok(outcome),
            Err(interrupted) => {
                if interrupted.started {
                    let label = match interrupted.stop {
                        Stop::Canceled => "canceled",
                        Stop::TimedOut => "timeout",
                    };
                    self.metrics
                        .gateway_attempts
                        .with_label_values(&[label])
                        .inc();
                }
                Err(self.stopped(interrupted.stop))
            }
        }
    }

    /// Runs the attempt loop to completion.
    pub async fn execute(self: Arc<Self>, job: Job) -> Result<Output, ApiError> {
        let span = tracing::info_span!("delitools.execute", request_id = %job.request_id, procedure = job.task.procedure().name());
        self.run_attempts(&job).instrument(span).await
    }

    async fn run_attempts(&self, job: &Job) -> Result<Output, ApiError> {
        let request = match &job.task {
            Task::Translate(t) => t.chat_request(self.config.gateway.max_output_tokens),
            Task::Extract(e) => e.chat_request(self.config.gateway.max_output_tokens),
        };
        let max_attempts = self.config.retry.max_attempts();
        let mut hint = None;
        for attempt in 1..=max_attempts {
            if attempt > 1 {
                let wait = self
                    .config
                    .retry
                    .delay(attempt - 1, hint, rand::random::<f64>());
                self.pause(job, wait).await.map_err(|s| self.stopped(s))?;
                // Revocation, expiry, permission loss or a billing block stops further attempts.
                tokio::select! {
                    _ = job.cancel.cancelled() => return Err(self.stopped(Stop::Canceled)),
                    _ = tokio::time::sleep_until(job.deadline) => return Err(self.stopped(Stop::TimedOut)),
                    checked = self.auth.recheck(&job.principal) => {
                        if let Err(error) = checked {
                            tracing::warn!(category = "retry_authorization", attempt, reason = error.label(), "authorization no longer allows provider retries");
                            return Err(error);
                        }
                    }
                }
            }
            let outcome = self.provider_attempt(job, &request).await?;
            let completion = match outcome {
                Ok(completion) => completion,
                Err(Failure::Transient { retry_after }) => {
                    self.metrics
                        .gateway_attempts
                        .with_label_values(&["transient"])
                        .inc();
                    tracing::warn!(
                        category = "gateway",
                        attempt,
                        outcome = "transient",
                        "provider attempt failed transiently"
                    );
                    hint = retry_after;
                    if attempt == max_attempts {
                        return Err(ApiError::provider_unavailable());
                    }
                    continue;
                }
                Err(Failure::Permanent) => {
                    self.metrics
                        .gateway_attempts
                        .with_label_values(&["permanent"])
                        .inc();
                    tracing::error!(
                        category = "gateway",
                        attempt,
                        outcome = "permanent",
                        "provider rejected the request"
                    );
                    return Err(ApiError::provider_rejected());
                }
            };
            // Confirmed usage is billable whatever the output turns out to be.
            match completion.usage {
                UsageReading::Known(tokens) => {
                    tracing::info!(
                        category = "usage",
                        attempt,
                        input_tokens = tokens.input,
                        output_tokens = tokens.output,
                        "provider usage confirmed"
                    );
                    self.record_usage(job, attempt, tokens).await?;
                }
                UsageReading::Unknown(reason) => {
                    self.metrics
                        .unknown_usage
                        .with_label_values(&[reason.label()])
                        .inc();
                    tracing::warn!(
                        category = "unknown_usage",
                        attempt,
                        reason = reason.label(),
                        "provider usage is unknown; nothing is charged"
                    );
                }
            }
            if completion.truncated {
                self.metrics
                    .gateway_attempts
                    .with_label_values(&["invalid_output"])
                    .inc();
                return Err(ApiError::invalid_output(
                    "the model output was cut off at the token limit",
                ));
            }
            let Some(content) = completion.content else {
                self.metrics
                    .gateway_attempts
                    .with_label_values(&["invalid_output"])
                    .inc();
                return Err(ApiError::invalid_output(
                    "the model returned no text output",
                ));
            };
            return match job.task.parse(&content) {
                Ok(output) => {
                    self.metrics
                        .gateway_attempts
                        .with_label_values(&["ok"])
                        .inc();
                    Ok(output)
                }
                Err(error) => {
                    // A missing-information answer is a valid model response; only malformed or
                    // nonconforming output counts as invalid.
                    let outcome = if error.reason == delino_protocol::delitools::v1::ErrorReason::ERROR_REASON_EXTRACTION_FAILED {
                        "ok"
                    } else {
                        "invalid_output"
                    };
                    self.metrics
                        .gateway_attempts
                        .with_label_values(&[outcome])
                        .inc();
                    Err(error)
                }
            };
        }
        Err(ApiError::provider_unavailable())
    }

    /// Stores the billable events of one attempt. Failure to store is not swallowed: a result must
    /// not be returned for usage that could be lost.
    async fn record_usage(
        &self,
        job: &Job,
        attempt: u32,
        tokens: usage::TokenUsage,
    ) -> Result<(), ApiError> {
        let events = usage::events_for(
            &job.request_id.to_string(),
            attempt,
            &job.model.input_meter_key,
            &job.model.output_meter_key,
            tokens,
        );
        if events.is_empty() {
            return Ok(());
        }
        let tenant = Uuid::parse_str(&job.principal.tenant_id).map_err(|_| ApiError::internal())?;
        for tries in 0..3u64 {
            match self
                .outbox
                .insert_attempt(
                    tenant,
                    job.request_id,
                    attempt,
                    job.started_at,
                    &events,
                    server_common::now(),
                )
                .await
            {
                Ok(()) => {
                    for event in &events {
                        self.metrics
                            .usage_persisted
                            .with_label_values(&[event.direction.as_str()])
                            .inc();
                    }
                    return Ok(());
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50 * (tries + 1))).await,
            }
        }
        self.metrics.persist_failures.inc();
        tracing::error!(
            category = "usage_persist",
            alert = true,
            attempt,
            "confirmed provider usage could not be stored"
        );
        Err(ApiError::dependency_unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Counts polls and drops of a provider future that never completes.
    struct Probe {
        polls: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }
    impl Future for Probe {
        type Output = ();
        fn poll(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<()> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            std::task::Poll::Pending
        }
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn probe() -> (Probe, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let polls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        (
            Probe {
                polls: polls.clone(),
                drops: drops.clone(),
            },
            polls,
            drops,
        )
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(3600)
    }

    #[tokio::test]
    async fn a_canceled_job_never_starts_the_provider_call() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (call, polls, drops) = probe();
        let error = race_provider(&cancel, far(), call).await.err().unwrap();
        assert_eq!(error.stop, Stop::Canceled);
        assert!(!error.started);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_expired_job_never_starts_the_provider_call() {
        let (call, polls, _) = probe();
        let error = race_provider(&CancellationToken::new(), Instant::now(), call)
            .await
            .err()
            .unwrap();
        assert_eq!(error.stop, Stop::TimedOut);
        assert!(!error.started);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancellation_during_a_pending_call_is_a_started_stop() {
        let cancel = CancellationToken::new();
        let (call, polls, drops) = probe();
        let later = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            later.cancel();
        });
        let error = race_provider(&cancel, far(), call).await.err().unwrap();
        assert_eq!(error.stop, Stop::Canceled);
        assert!(error.started);
        assert!(polls.load(Ordering::SeqCst) >= 1);
        assert_eq!(
            drops.load(Ordering::SeqCst),
            1,
            "the provider future is dropped"
        );
    }

    #[tokio::test]
    async fn the_deadline_during_a_pending_call_is_a_started_stop() {
        let (call, _, drops) = probe();
        let deadline = Instant::now() + Duration::from_millis(30);
        let error = race_provider(&CancellationToken::new(), deadline, call)
            .await
            .err()
            .unwrap();
        assert_eq!(error.stop, Stop::TimedOut);
        assert!(error.started);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_completed_call_returns_its_output() {
        let result = race_provider(&CancellationToken::new(), far(), async { 7 }).await;
        assert_eq!(result.ok(), Some(7));
    }
}
