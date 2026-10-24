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
//! 2. Call the Gateway, racing the call against cancellation and the execution deadline.
//! 3. A transient failure (network, 5xx, 429) retries until the retry budget is spent.
//! 4. A completed response with confirmed usage is stored in the outbox *before* its content is
//!    interpreted, so failed, invalid or canceled-after-response attempts are still billed.
//!    Missing or invalid usage is counted and logged, never estimated.
//! 5. Output is validated; invalid output is final (no repair retry, no model substitution).
//!
//! Cancellation has two sources: the caller (handler drop) and the service-wide fence that closes
//! when the process loses the instance lock. Each job's token is a child of the fence, so both end
//! provider calls and retries the same way, and neither blocks the storing of confirmed usage; the
//! finite persistence budget (`DEPENDENCY_TIMEOUT`) bounds that storing so the old process can exit.

use crate::{
    Inner,
    auth::Principal,
    error::ApiError,
    extract::Extraction,
    limits::Permit,
    models::ModelEntry,
    retry::Failure,
    telemetry::Procedure,
    translate::{self, Translation},
    usage::{self, UsageReading},
};
use chrono::{DateTime, Utc};
use std::{sync::Arc, time::Duration};
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

/// Maximum tries to store the events of one attempt, within one `dependency_timeout` budget.
const PERSIST_ATTEMPTS: u32 = 3;

enum Stop {
    Canceled,
    TimedOut,
}

impl Inner {
    /// Sleeps for `wait` unless canceled or past the deadline.
    async fn pause(&self, job: &Job, wait: Duration) -> Result<(), Stop> {
        tokio::select! {
            biased;
            _ = job.cancel.cancelled() => Err(Stop::Canceled),
            _ = tokio::time::sleep_until(job.deadline) => Err(Stop::TimedOut),
            _ = tokio::time::sleep(wait) => Ok(()),
        }
    }

    fn stopped(&self, stop: Stop) -> ApiError {
        match stop {
            Stop::Canceled => {
                self.metrics
                    .gateway_attempts
                    .with_label_values(&["canceled"])
                    .inc();
                ApiError::canceled()
            }
            Stop::TimedOut => {
                self.metrics
                    .gateway_attempts
                    .with_label_values(&["timeout"])
                    .inc();
                ApiError::timeout()
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
                    biased;
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
            // No attempt may start once the request deadline has passed; check it explicitly rather
            // than relying on the poll order of the `select!` below.
            if Instant::now() >= job.deadline {
                return Err(self.stopped(Stop::TimedOut));
            }
            // Biased: cancellation, including the instance fence, is observed before the Gateway
            // future is first polled, so no attempt starts once the job is canceled.
            let outcome = tokio::select! {
                biased;
                _ = job.cancel.cancelled() => return Err(self.stopped(Stop::Canceled)),
                _ = tokio::time::sleep_until(job.deadline) => return Err(self.stopped(Stop::TimedOut)),
                result = self.gateway.complete(&request) => result,
            };
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
        // One finite budget covers every try, backoff, acquisition and commit. It starts when
        // persistence begins and ignores caller cancellation, the instance fence and the provider
        // deadline, so a held lock cannot keep the tenant permit or a pooled connection
        // indefinitely. Because the budget is `DEPENDENCY_TIMEOUT`, confirmed usage is stored
        // whatever cancels the job, and a fenced process finishes this step within the same bound;
        // the rows already written stay in the outbox for the replacement to deliver.
        let deadline = Instant::now() + self.config.dependency_timeout;
        for tries in 0..PERSIST_ATTEMPTS {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self
                .outbox
                .insert_attempt(
                    tenant,
                    job.request_id,
                    attempt,
                    job.started_at,
                    &events,
                    server_common::now(),
                    remaining,
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
                Err(error) => {
                    tracing::warn!(
                        category = "usage_persist",
                        attempt,
                        try_number = tries + 1,
                        error = %error,
                        "storing confirmed usage failed"
                    );
                    if tries + 1 < PERSIST_ATTEMPTS {
                        let backoff = Duration::from_millis(50 * (u64::from(tries) + 1));
                        tokio::time::sleep(
                            backoff.min(deadline.saturating_duration_since(Instant::now())),
                        )
                        .await;
                    }
                }
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
