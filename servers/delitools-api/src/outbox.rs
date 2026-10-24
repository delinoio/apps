// SPDX-License-Identifier: FSL-1.1-ALv2
//! Durable usage outbox and its delivery worker.
//!
//! Confirmed provider usage is written to `usage_outbox` before the RPC result is returned, so
//! a crash, restart, cancellation or Delcore outage cannot lose billable usage. The delivery
//! worker reports each row with `IngestEvent` using its stable event key; Delcore's
//! service-scoped idempotency makes retries and duplicate deliveries safe. A row is removed when
//! Delcore accepts it (an identical replay counts as accepted), when Delcore reports the tenant
//! finally deleted, or 30 days after its first creation, whichever comes first. Retries never
//! extend the expiry, which a database trigger and check constraint also enforce.
//!
//! Row state machine ([`transition`]):
//!
//! | state     | outcome        | next                                                    |
//! |-----------|----------------|---------------------------------------------------------|
//! | `pending` | delivered      | row deleted                                             |
//! | `pending` | retryable      | `pending`, attempts + 1, exponential backoff            |
//! | `pending` | poison         | `poisoned`, never delivered again, kept until expiry    |
//! | `pending` | tenant deleted | row deleted                                             |
//! | any       | 30 days elapse | row deleted                                             |

use crate::{
    config::{OUTBOX_RETENTION, OutboxSettings},
    delcore::{DelcoreClient, IngestFailure, UsageEvent},
    telemetry::Metrics,
    usage::{Direction, NewEvent},
};
use chrono::{DateTime, Utc};
use server_common::authority::TenantState;
use sqlx::{PgPool, Row};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Result of one delivery attempt of one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Delivered,
    Retry(&'static str),
    Poison(&'static str),
    TenantDeleted,
}

/// A retry deadline that cannot be represented, so the row cannot be rescheduled.
///
/// Carries no content: the category is bounded and the configured values are never printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduleError;

impl ScheduleError {
    /// Bounded log category of this failure.
    pub const CATEGORY: &'static str = "usage_schedule";
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("usage retry deadline is not representable")
    }
}

impl std::error::Error for ScheduleError {}

/// Failure of one delivery cycle.
#[derive(Debug)]
pub enum CycleError {
    /// PostgreSQL access failed.
    Database(sqlx::Error),
    /// A retry deadline could not be formed; the affected row stays pending.
    Schedule(ScheduleError),
}

impl std::fmt::Display for CycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => write!(f, "usage outbox database error: {e}"),
            Self::Schedule(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CycleError {}

impl From<sqlx::Error> for CycleError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

impl From<ScheduleError> for CycleError {
    fn from(e: ScheduleError) -> Self {
        Self::Schedule(e)
    }
}

/// What to persist after an [`Outcome`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transition {
    Delete,
    Reschedule {
        delivery_attempts: i32,
        next_attempt_at: DateTime<Utc>,
        category: &'static str,
    },
    Poison {
        category: &'static str,
    },
}

/// Exponential backoff after `failed_deliveries` failed deliveries, within the configured bounds.
pub fn backoff(failed_deliveries: i32, settings: &OutboxSettings) -> Duration {
    let exponent = failed_deliveries.clamp(0, 30) as u32;
    settings
        .backoff_min
        .saturating_mul(1u32.checked_shl(exponent).unwrap_or(u32::MAX))
        .min(settings.backoff_max)
}

/// Pure state machine of a pending row. `delivery_attempts` counts deliveries tried before this one.
///
/// A retry fails with [`ScheduleError`] when its backoff cannot be converted to a calendar
/// delta or added to `now`. Startup validation rejects such bounds, so this is a defense that
/// keeps the worker alive and the row pending instead of panicking.
pub fn transition(
    outcome: Outcome,
    delivery_attempts: i32,
    now: DateTime<Utc>,
    settings: &OutboxSettings,
) -> Result<Transition, ScheduleError> {
    Ok(match outcome {
        Outcome::Delivered | Outcome::TenantDeleted => Transition::Delete,
        Outcome::Poison(category) => Transition::Poison { category },
        Outcome::Retry(category) => {
            let delta = chrono::Duration::from_std(backoff(delivery_attempts, settings))
                .map_err(|_| ScheduleError)?;
            Transition::Reschedule {
                delivery_attempts: delivery_attempts.saturating_add(1),
                next_attempt_at: now.checked_add_signed(delta).ok_or(ScheduleError)?,
                category,
            }
        }
    })
}

/// True when a row created at `created_at` is expired at `now`. The boundary instant is expired.
pub fn expired(created_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now >= created_at + OUTBOX_RETENTION
}

/// A pending row selected for delivery.
#[derive(Clone, Debug)]
pub struct DueRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event: UsageEvent,
    pub delivery_attempts: i32,
}

/// Counters read after each cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub pending: i64,
    pub poisoned: i64,
    pub oldest_pending_age_seconds: i64,
}

/// PostgreSQL access to the outbox.
#[derive(Clone)]
pub struct Outbox {
    pool: PgPool,
}

impl Outbox {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Stores the confirmed usage events of one attempt in one transaction.
    ///
    /// Replays of the same event key are ignored so a repeated call cannot create a second charge.
    pub async fn insert_attempt(
        &self,
        tenant_id: Uuid,
        request_id: Uuid,
        attempt: u32,
        occurred_at: DateTime<Utc>,
        events: &[NewEvent],
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        for event in events {
            sqlx::query(
                "INSERT INTO usage_outbox(id,tenant_id,request_id,attempt,direction,event_key,delivery_key,meter_key,quantity,occurred_at,state,next_attempt_at,created_at,expires_at)
                 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'pending',$11,$11,$12)
                 ON CONFLICT (event_key) DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(tenant_id)
            .bind(request_id)
            .bind(attempt as i32)
            .bind(match event.direction {
                Direction::Input => "in",
                Direction::Output => "out",
            })
            .bind(&event.event_key)
            .bind(Uuid::now_v7())
            .bind(&event.meter_key)
            .bind(event.quantity)
            .bind(occurred_at)
            .bind(now)
            .bind(now + OUTBOX_RETENTION)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// Pending, unexpired rows whose retry time has come, oldest first.
    pub async fn due(&self, now: DateTime<Utc>, limit: i64) -> Result<Vec<DueRow>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id,tenant_id,event_key,delivery_key,meter_key,quantity,occurred_at,delivery_attempts
             FROM usage_outbox
             WHERE state='pending' AND next_attempt_at<=$1 AND expires_at>$1
             ORDER BY created_at,id LIMIT $2",
        )
        .bind(now)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let tenant_id: Uuid = r.get("tenant_id");
                DueRow {
                    id: r.get("id"),
                    tenant_id,
                    delivery_attempts: r.get("delivery_attempts"),
                    event: UsageEvent {
                        request_key: r.get::<Uuid, _>("delivery_key").to_string(),
                        tenant_id: tenant_id.to_string(),
                        event_key: r.get("event_key"),
                        meter_key: r.get("meter_key"),
                        quantity: r.get("quantity"),
                        occurred_at: r.get("occurred_at"),
                    },
                }
            })
            .collect())
    }

    /// Applies a [`Transition`] to a row.
    pub async fn apply(
        &self,
        id: Uuid,
        transition: &Transition,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        match transition {
            Transition::Delete => {
                sqlx::query("DELETE FROM usage_outbox WHERE id=$1")
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
            }
            Transition::Reschedule {
                delivery_attempts,
                next_attempt_at,
                category,
            } => {
                sqlx::query("UPDATE usage_outbox SET delivery_attempts=$2,next_attempt_at=$3,last_attempt_at=$4,last_failure=$5 WHERE id=$1")
                    .bind(id)
                    .bind(delivery_attempts)
                    .bind(next_attempt_at)
                    .bind(now)
                    .bind(category)
                    .execute(&self.pool)
                    .await?;
            }
            Transition::Poison { category } => {
                sqlx::query("UPDATE usage_outbox SET state='poisoned',delivery_attempts=delivery_attempts+1,last_attempt_at=$2,last_failure=$3 WHERE id=$1")
                    .bind(id)
                    .bind(now)
                    .bind(category)
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// Removes every row of a tenant that Delcore reports as finally deleted.
    pub async fn delete_tenant(&self, tenant_id: Uuid) -> Result<u64, sqlx::Error> {
        Ok(sqlx::query("DELETE FROM usage_outbox WHERE tenant_id=$1")
            .bind(tenant_id)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }

    /// Removes rows 30 days after their first creation, whatever their state.
    pub async fn expire(&self, now: DateTime<Utc>) -> Result<u64, sqlx::Error> {
        Ok(sqlx::query("DELETE FROM usage_outbox WHERE expires_at<=$1")
            .bind(now)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }

    pub async fn stats(&self, now: DateTime<Utc>) -> Result<Stats, sqlx::Error> {
        let row = sqlx::query(
            "SELECT count(*) FILTER (WHERE state='pending') AS pending,
                    count(*) FILTER (WHERE state='poisoned') AS poisoned,
                    min(created_at) FILTER (WHERE state='pending') AS oldest
             FROM usage_outbox",
        )
        .fetch_one(&self.pool)
        .await?;
        let oldest: Option<DateTime<Utc>> = row.get("oldest");
        Ok(Stats {
            pending: row.get("pending"),
            poisoned: row.get("poisoned"),
            oldest_pending_age_seconds: oldest.map_or(0, |at| (now - at).num_seconds().max(0)),
        })
    }
}

/// Delivery worker. One instance runs inside the single serving process.
pub struct Delivery {
    /// Closes when the process loses the instance lock; no cycle phase or `IngestEvent` starts
    /// afterwards and a running call is dropped.
    pub fence: CancellationToken,
    pub outbox: Outbox,
    pub delcore: Arc<DelcoreClient>,
    pub settings: OutboxSettings,
    pub metrics: Arc<Metrics>,
}

impl Delivery {
    /// Runs one expiry, tenant-lifecycle and delivery cycle at `now`.
    pub async fn cycle(&self, now: DateTime<Utc>) -> Result<Stats, CycleError> {
        if self.fence.is_cancelled() {
            return Ok(self.outbox.stats(now).await?);
        }
        let expired = self.outbox.expire(now).await?;
        if expired > 0 {
            self.metrics.expired.inc_by(expired);
            tracing::warn!(
                category = "usage_expired",
                rows = expired,
                "undelivered usage expired after 30 days"
            );
        }
        let mut rows = self.outbox.due(now, self.settings.batch).await?;
        if !rows.is_empty() {
            let tenants: Vec<String> = rows
                .iter()
                .map(|r| r.event.tenant_id.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            // A failed lifecycle lookup only skips this optimization; delivery still runs.
            let states = tokio::select! {
                biased;
                _ = self.fence.cancelled() => return Ok(self.outbox.stats(now).await?),
                states = self.delcore.tenant_states(&tenants) => states,
            };
            if let Ok(states) = states {
                for (tenant, state) in states {
                    if state == TenantState::Deleted
                        && let Ok(id) = Uuid::parse_str(&tenant)
                    {
                        let removed = self.outbox.delete_tenant(id).await?;
                        self.metrics
                            .deliveries
                            .with_label_values(&["tenant_deleted"])
                            .inc_by(removed);
                        tracing::info!(
                            category = "usage_tenant_deleted",
                            rows = removed,
                            "usage dropped for deleted tenant"
                        );
                        rows.retain(|r| r.tenant_id != id);
                    }
                }
            }
        }
        for row in rows {
            // An ingest whose answer is lost to the fence is simply retried by the replacement;
            // Delcore's service-scoped idempotency makes that replay safe.
            let ingest = tokio::select! {
                biased;
                _ = self.fence.cancelled() => return Ok(self.outbox.stats(now).await?),
                ingest = self.delcore.ingest_event(&row.event) => ingest,
            };
            let outcome = match ingest {
                Ok(()) => Outcome::Delivered,
                Err(IngestFailure::Retryable(category)) => Outcome::Retry(category),
                Err(IngestFailure::Poison(category)) => Outcome::Poison(category),
            };
            let label = match outcome {
                Outcome::Delivered => "delivered",
                Outcome::Retry(_) => "retry",
                Outcome::Poison(_) => "poison",
                Outcome::TenantDeleted => "tenant_deleted",
            };
            self.metrics.deliveries.with_label_values(&[label]).inc();
            match outcome {
                Outcome::Poison(category) => {
                    tracing::error!(category = "usage_poison", failure = category, alert = true, event_key = %row.event.event_key, "Delcore will never accept this usage event");
                }
                Outcome::Retry(category) => {
                    tracing::warn!(category = "usage_delivery_retry", failure = category, attempts = row.delivery_attempts + 1, event_key = %row.event.event_key, "usage delivery failed; retrying with backoff");
                }
                _ => {}
            }
            // An impossible deadline leaves the row untouched and ends this cycle; the next
            // cycle runs normally.
            let next = transition(outcome, row.delivery_attempts, now, &self.settings)
                .inspect_err(|_| {
                    tracing::error!(
                        category = ScheduleError::CATEGORY,
                        alert = true,
                        "usage retry deadline is not representable; the row stays pending"
                    );
                })?;
            self.outbox.apply(row.id, &next, now).await?;
        }
        let stats = self.outbox.stats(now).await?;
        self.publish(stats);
        Ok(stats)
    }

    fn publish(&self, stats: Stats) {
        self.metrics.outbox_pending.set(stats.pending);
        self.metrics.outbox_poisoned.set(stats.poisoned);
        self.metrics
            .outbox_oldest_pending_age_seconds
            .set(stats.oldest_pending_age_seconds);
        let alert = stats.pending > self.settings.alert_backlog
            || stats.oldest_pending_age_seconds as u64 > self.settings.alert_age.as_secs();
        self.metrics.outbox_alert.set(i64::from(alert));
        if alert {
            tracing::error!(
                category = "usage_backlog",
                alert = true,
                pending = stats.pending,
                oldest_age_seconds = stats.oldest_pending_age_seconds,
                "usage delivery backlog exceeds the alert threshold"
            );
        }
        if stats.poisoned > 0 {
            tracing::warn!(
                category = "usage_poison",
                poisoned = stats.poisoned,
                "poisoned usage rows await operator attention"
            );
        }
    }

    /// Runs cycles until the task is aborted or the fence closes.
    pub async fn run(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(self.settings.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = self.fence.cancelled() => return,
                _ = ticker.tick() => {}
            }
            if self.fence.is_cancelled() {
                return;
            }
            if self.cycle(server_common::now()).await.is_err() {
                tracing::error!(category = "usage_cycle", "usage delivery cycle failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> OutboxSettings {
        OutboxSettings {
            interval: Duration::from_secs(1),
            batch: 10,
            backoff_min: Duration::from_secs(5),
            backoff_max: Duration::from_secs(60),
            alert_backlog: 100,
            alert_age: Duration::from_secs(600),
        }
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + secs, 0).unwrap()
    }

    #[test]
    fn backoff_doubles_within_bounds() {
        let s = settings();
        assert_eq!(backoff(0, &s), Duration::from_secs(5));
        assert_eq!(backoff(1, &s), Duration::from_secs(10));
        assert_eq!(backoff(2, &s), Duration::from_secs(20));
        assert_eq!(backoff(3, &s), Duration::from_secs(40));
        assert_eq!(backoff(4, &s), Duration::from_secs(60));
        assert_eq!(backoff(500, &s), Duration::from_secs(60));
        assert_eq!(backoff(-4, &s), Duration::from_secs(5));
    }

    #[test]
    fn delivered_and_deleted_tenants_remove_the_row() {
        let s = settings();
        assert_eq!(
            transition(Outcome::Delivered, 3, at(0), &s).unwrap(),
            Transition::Delete
        );
        assert_eq!(
            transition(Outcome::TenantDeleted, 0, at(0), &s).unwrap(),
            Transition::Delete
        );
    }

    #[test]
    fn retryable_failures_reschedule_with_growing_backoff() {
        let s = settings();
        assert_eq!(
            transition(Outcome::Retry("unavailable"), 0, at(100), &s).unwrap(),
            Transition::Reschedule {
                delivery_attempts: 1,
                next_attempt_at: at(105),
                category: "unavailable"
            }
        );
        assert_eq!(
            transition(Outcome::Retry("unavailable"), 2, at(100), &s).unwrap(),
            Transition::Reschedule {
                delivery_attempts: 3,
                next_attempt_at: at(120),
                category: "unavailable"
            }
        );
    }

    #[test]
    fn unrepresentable_retry_deadlines_are_errors_not_panics() {
        for secs in [10_000_000_000_000_u64, 10_000_000_000_000_000, u64::MAX] {
            let mut s = settings();
            s.backoff_min = Duration::from_secs(secs);
            s.backoff_max = Duration::from_secs(secs);
            assert_eq!(
                transition(Outcome::Retry("unavailable"), 0, server_common::now(), &s),
                Err(ScheduleError),
                "{secs}"
            );
            // Non-retry outcomes never need a deadline.
            assert_eq!(
                transition(Outcome::Delivered, 0, server_common::now(), &s),
                Ok(Transition::Delete)
            );
        }
    }

    #[test]
    fn large_representable_backoffs_still_schedule() {
        let mut s = settings();
        s.backoff_min = Duration::from_secs(60 * 86_400);
        s.backoff_max = s.backoff_min;
        assert_eq!(
            transition(Outcome::Retry("unavailable"), 0, at(100), &s),
            Ok(Transition::Reschedule {
                delivery_attempts: 1,
                next_attempt_at: at(100 + 60 * 86_400),
                category: "unavailable"
            })
        );
    }

    #[test]
    fn poison_stops_delivery_without_rescheduling() {
        let s = settings();
        assert_eq!(
            transition(Outcome::Poison("identity_conflict"), 1, at(0), &s).unwrap(),
            Transition::Poison {
                category: "identity_conflict"
            }
        );
    }

    #[test]
    fn expiry_is_thirty_days_after_first_creation_and_includes_the_boundary() {
        let created = at(0);
        let day = chrono::Duration::days(1);
        assert!(!expired(created, created + day * 29));
        assert!(!expired(
            created,
            created + day * 30 - chrono::Duration::microseconds(1)
        ));
        assert!(expired(created, created + day * 30));
        assert!(expired(created, created + day * 31));
    }

    #[test]
    fn retries_never_move_the_expiry() {
        // The transition carries no expiry field; only the retry time moves.
        let s = settings();
        let first = at(0);
        let mut deliveries = 0;
        for _ in 0..40 {
            match transition(Outcome::Retry("unavailable"), deliveries, first, &s).unwrap() {
                Transition::Reschedule {
                    delivery_attempts, ..
                } => deliveries = delivery_attempts,
                other => panic!("{other:?}"),
            }
        }
        assert!(expired(first, first + chrono::Duration::days(30)));
    }
}
