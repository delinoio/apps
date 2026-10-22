// SPDX-License-Identifier: FSL-1.1-ALv2

use super::{Worker, rating::rate_metering};
use crate::{events, telemetry::Outcome};
use anyhow::{Result, anyhow};
use chrono::{Duration, Utc};
use sqlx::Row;
use std::time::Instant;

/// Bound each step so a metering backlog cannot starve other durable work.
pub const METERING_BATCH: usize = 100;

#[derive(Debug)]
struct InvalidPayload;
impl std::fmt::Display for InvalidPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid metering job payload")
    }
}
impl std::error::Error for InvalidPayload {}

// This guard also records an interrupted attempt when its future is dropped.
// It owns no metric update: cancellation must not consume the job retry budget.
struct AttemptSpan {
    span: tracing::Span,
    completed: bool,
}
impl Drop for AttemptSpan {
    fn drop(&mut self) {
        if !self.completed {
            self.span.record("delcore.outcome", "deferred");
            self.span.record("delcore.failure_category", "shutdown");
        }
    }
}

impl Worker {
    pub async fn process_metering(&self, limit: usize) -> Result<usize> {
        let mut processed = 0;
        while processed < limit && self.process_metering_job().await? {
            processed += 1;
        }
        Ok(processed)
    }

    async fn process_metering_job(&self) -> Result<bool> {
        let started = Instant::now();
        let mut claimed: Option<(String, String, i32)> = None;
        let mut attempt: Option<AttemptSpan> = None;
        // Cancellation drops the transaction and leaves the retry budget intact.
        // A processing timeout is different: record it using a fresh deadline.
        let result: Result<Option<(events::Applied, usize)>> = match tokio::time::timeout(self.config.timeout(), async {
            let mut tx = crate::postgres::begin(&self.pool).await?;
            let row = sqlx::query("SELECT id::text,COALESCE(tenant_id::text,''),attempts,payload FROM jobs WHERE kind=$1 AND state='pending' AND available_at<=now() ORDER BY available_at,id LIMIT 1 FOR UPDATE SKIP LOCKED")
                .bind(events::JOB_KIND).fetch_optional(&mut *tx).await?;
            let Some(row) = row else { return Ok(None) };
            let job_id: String = row.try_get(0)?;
            let tenant: String = row.try_get(1)?;
            let attempts: i32 = row.try_get(2)?;
            claimed = Some((job_id.clone(), tenant, attempts));
            attempt = Some(AttemptSpan { span: tracing::info_span!(
                "delcore.metering.process",
                "delcore.job_id" = job_id.as_str(),
                "delcore.job.attempt" = attempts + 1,
                "delcore.outcome" = tracing::field::Empty,
                "delcore.failure_category" = tracing::field::Empty,
                elapsed_seconds = tracing::field::Empty
            ), completed: false });
            let payload: serde_json::Value = row.try_get(3)?;
            let event_id = payload.get("event_id").and_then(serde_json::Value::as_str)
                .filter(|id| !id.is_empty()).ok_or(InvalidPayload)?;
            let now = Utc::now();
            let applied = events::process(&mut tx, now, event_id).await?;
            let entries = rate_metering(&mut tx, now, &applied).await?;
            sqlx::query("UPDATE jobs SET state='done',error_category='',attempts=attempts+1,updated_at=now() WHERE id=$1::text::uuid")
                .bind(&job_id).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(Some((applied, entries)))
        }).await {
            Ok(result) => result,
            Err(_) => Err(anyhow!("metering processing timed out")),
        };
        if matches!(result, Ok(None)) {
            return Ok(false);
        }
        let span = if let Some(attempt) = &mut attempt {
            attempt.completed = true;
            attempt
                .span
                .record("elapsed_seconds", started.elapsed().as_secs_f64());
            attempt.span.clone()
        } else {
            tracing::Span::none()
        };
        let error = match result {
            Ok(Some((applied, entries))) => {
                self.metrics
                    .metering_job(Outcome::Applied, "", started.elapsed());
                span.record("delcore.outcome", "applied");
                tracing::debug!(parent: &span, category="metering_processing", from_revision=applied.from_revision, to_revision=applied.to_revision, changed_groups=applied.changes.len(), rating_differences=entries, "metering work applied");
                return Ok(true);
            }
            Err(error) => error,
            Ok(None) => unreachable!(),
        };
        let Some((job_id, tenant, attempts)) = claimed else {
            span.record("delcore.outcome", "failed");
            span.record("delcore.failure_category", "database");
            return Err(error);
        };
        let (state, category) = if error.downcast_ref::<InvalidPayload>().is_some() {
            ("failed", "invalid_payload")
        } else if error
            .downcast_ref::<crate::Fault>()
            .is_some_and(|error| error.permanent)
        {
            ("failed", "metering_invalid")
        } else if i64::from(attempts) + 1 >= self.config.retry_limit {
            ("failed", "retry_exhausted")
        } else {
            ("pending", "metering_transient")
        };
        self.metrics.metering_job(
            if state == "failed" {
                Outcome::Failed
            } else {
                Outcome::Deferred
            },
            category,
            started.elapsed(),
        );
        span.record(
            "delcore.outcome",
            if state == "failed" {
                "failed"
            } else {
                "deferred"
            },
        );
        span.record("delcore.failure_category", category);
        tracing::warn!(parent: &span, category, state, job_id, attempts=attempts+1, "metering work deferred");
        tokio::time::timeout(self.config.timeout(), async {
            let mut tx = crate::postgres::begin(&self.pool).await?;
            let count = sqlx::query("UPDATE jobs SET state=$2,error_category=$3,attempts=attempts+1,available_at=$4,updated_at=now(),failed_at=now() WHERE id=$1::text::uuid AND state='pending'")
                .bind(&job_id).bind(state).bind(category).bind(Utc::now()+Duration::seconds(self.config.retry_seconds))
                .execute(&mut *tx).await?.rows_affected();
            if state == "failed" && count == 1 {
                // Every UUID column is bound as text and cast explicitly: sqlx sends a Rust
                // String as TEXT, which PostgreSQL will not assign to a uuid column.
                sqlx::query("INSERT INTO audit(id,actor_id,tenant_id,action,reason,correlation_id,created_at) VALUES($1::text::uuid,'system:metering',NULLIF($2,'')::uuid,'metering.failed',$3,$4::text::uuid,$5)")
                    .bind(uuid::Uuid::now_v7().to_string()).bind(&tenant)
                    .bind(format!("category={category} attempts={}", attempts+1))
                    .bind(uuid::Uuid::now_v7().to_string()).bind(Utc::now()).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            Ok::<_, anyhow::Error>(())
        }).await??;
        Ok(true)
    }
}
