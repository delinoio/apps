// SPDX-License-Identifier: FSL-1.1-ALv2
//! Background jobs that run in the API process. Each job takes a PostgreSQL advisory lock so
//! that only one replica runs it at a time; schedules are shared through `worker_job_runs`.
use crate::{
    Server,
    rpc::{DeletionKind, SOURCE_CONNECTION, SOURCE_DEVICE, e16},
    time,
};
use buffa::Enumeration;
use chrono::{DateTime, Duration, Utc};
use delino_protocol::anydash::v1::{
    DeviceCredentialState, ParserState, Provider, SharingEndReason, SpaceKind,
};
use server_common::authority::TenantState;
use sqlx::{PgConnection, Row};
use std::time::Duration as StdDuration;
use uuid::Uuid;

/// Last start, success and failure times of a job.
type JobTimes = (
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
);
/// Job ID, kind, target, user, tenant and request time of a deletion job.
type DeletionRow = (
    Uuid,
    i16,
    Option<Uuid>,
    Option<Uuid>,
    Option<Uuid>,
    DateTime<Utc>,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    LeaseExpiry,
    Deletion,
    Retention,
    TenantLifecycle,
    CredentialExpiry,
    PriceSync,
}

impl Job {
    pub const ALL: [Job; 6] = [
        Job::LeaseExpiry,
        Job::Deletion,
        Job::Retention,
        Job::TenantLifecycle,
        Job::CredentialExpiry,
        Job::PriceSync,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Job::LeaseExpiry => "lease_expiry",
            Job::Deletion => "deletion",
            Job::Retention => "retention",
            Job::TenantLifecycle => "tenant_lifecycle",
            Job::CredentialExpiry => "credential_expiry",
            Job::PriceSync => "price_sync",
        }
    }
    fn lock_key(self) -> i64 {
        0x4144_0000 + self as i64
    }
    fn every(self) -> Duration {
        match self {
            Job::LeaseExpiry => Duration::seconds(30),
            Job::Deletion => Duration::seconds(60),
            Job::Retention | Job::TenantLifecycle => Duration::hours(1),
            Job::CredentialExpiry => Duration::days(1),
            Job::PriceSync => Duration::hours(1),
        }
    }
}

/// The most recent daily price sync boundary (03:00 UTC) at or before `now`.
pub fn price_boundary(now: DateTime<Utc>) -> DateTime<Utc> {
    let today = now
        .date_naive()
        .and_hms_opt(3, 0, 0)
        .expect("valid boundary")
        .and_utc();
    if now >= today {
        today
    } else {
        today - Duration::days(1)
    }
}

/// Whether a job is due. The price sync runs once per daily 03:00 UTC window and retries
/// hourly after a failure; other jobs run at their fixed cadence.
pub fn due(
    job: Job,
    now: DateTime<Utc>,
    started: Option<DateTime<Utc>>,
    success: Option<DateTime<Utc>>,
    failure: Option<DateTime<Utc>>,
) -> bool {
    if job == Job::PriceSync {
        let boundary = price_boundary(now);
        if success.is_some_and(|s| s >= boundary) {
            return false;
        }
        return failure.is_none_or(|f| f < boundary || now - f >= Duration::hours(1));
    }
    // Allow a little jitter so a 30-second loop does not skip a 30-second job.
    started.is_none_or(|s| now - s >= job.every() - Duration::seconds(2))
}

pub fn spawn(server: Server) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(StdDuration::from_secs(15));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            for job in Job::ALL {
                if let Err(e) = run(&server, job, time::now(), false).await {
                    tracing::warn!(job = job.name(), error = %e, "worker job could not run");
                }
            }
        }
    })
}

/// Runs one job when it is due (or always with `force`) under its advisory lock. Returns
/// whether the job ran.
pub async fn run(s: &Server, job: Job, now: DateTime<Utc>, force: bool) -> anyhow::Result<bool> {
    let mut conn = s.db.acquire().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(job.lock_key())
        .fetch_one(&mut *conn)
        .await?;
    if !locked {
        return Ok(false);
    }
    let result = run_locked(s, job, now, force, &mut conn).await;
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(job.lock_key())
        .execute(&mut *conn)
        .await?;
    result
}

async fn run_locked(
    s: &Server,
    job: Job,
    now: DateTime<Utc>,
    force: bool,
    conn: &mut PgConnection,
) -> anyhow::Result<bool> {
    let row: Option<JobTimes> = sqlx::query_as(
        "SELECT last_started_at,last_success_at,last_failure_at FROM worker_job_runs WHERE job=$1",
    )
    .bind(job.name())
    .fetch_optional(&mut *conn)
    .await?;
    let (started, success, failure) = row.unwrap_or((None, None, None));
    if !force && !due(job, now, started, success, failure) {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO worker_job_runs(job,last_started_at) VALUES($1,$2) ON CONFLICT (job) DO UPDATE SET last_started_at=EXCLUDED.last_started_at",
    )
    .bind(job.name())
    .bind(now)
    .execute(&mut *conn)
    .await?;
    let correlation = crate::new_id().to_string();
    let span = tracing::info_span!("job", job = job.name(), correlation_id = %correlation);
    let outcome = {
        use tracing::Instrument;
        execute(s, job, now, &correlation).instrument(span).await
    };
    let ok = matches!(outcome, Ok(true));
    sqlx::query(&format!(
        "UPDATE worker_job_runs SET {}=$2 WHERE job=$1",
        if ok {
            "last_success_at"
        } else {
            "last_failure_at"
        }
    ))
    .bind(job.name())
    .bind(time::now())
    .execute(&mut *conn)
    .await?;
    s.metrics
        .job_runs
        .with_label_values(&[
            job.name(),
            crate::metrics::JOB_OUTCOMES[if ok { 0 } else { 1 }],
        ])
        .inc();
    if ok {
        s.metrics
            .job_last_success
            .with_label_values(&[job.name()])
            .set(time::now().timestamp() as f64);
    } else {
        match &outcome {
            Err(e) => tracing::warn!(job = job.name(), error = %e, "worker job failed"),
            Ok(_) => tracing::warn!(job = job.name(), "worker job recorded a handled failure"),
        }
    }
    Ok(true)
}

/// Executes a job; `Ok(false)` records a handled failure such as a rejected price document.
async fn execute(
    s: &Server,
    job: Job,
    now: DateTime<Utc>,
    correlation: &str,
) -> anyhow::Result<bool> {
    match job {
        Job::LeaseExpiry => lease_expiry(s, now).await.map(|_| true),
        Job::Deletion => deletion(s, now).await,
        Job::Retention => retention(s, now).await.map(|_| true),
        Job::TenantLifecycle => tenant_lifecycle(s, now, correlation).await.map(|_| true),
        Job::CredentialExpiry => credential_expiry(s, now).await.map(|_| true),
        Job::PriceSync => {
            let outcome = crate::pricing::sync(&s.db, &s.pricing, &s.metrics, now).await?;
            Ok(matches!(
                outcome,
                crate::pricing::Outcome::Created | crate::pricing::Outcome::Unchanged
            ))
        }
    }
}

/// Bounded metric label from a protocol enum name.
pub fn label<E: Enumeration>(e: E, prefix: &str) -> String {
    e.proto_name()
        .trim_start_matches(prefix)
        .to_ascii_lowercase()
}

async fn lease_expiry(s: &Server, now: DateTime<Utc>) -> anyhow::Result<()> {
    let expired: Vec<i16> = sqlx::query_scalar(
        "UPDATE connections SET last_holder_device_id=lease_device_id,lease_device_id=NULL,lease_expires_at=NULL \
         WHERE lease_device_id IS NOT NULL AND lease_expires_at<=$1 RETURNING provider",
    )
    .bind(now)
    .fetch_all(&s.db)
    .await?;
    for provider in expired {
        let p: Provider = crate::rpc::enum_of(provider);
        s.metrics
            .lease_failovers
            .with_label_values(&[&label(p, "PROVIDER_")])
            .inc();
    }
    Ok(())
}

async fn deletion(s: &Server, now: DateTime<Utc>) -> anyhow::Result<bool> {
    let jobs: Vec<DeletionRow> = sqlx::query_as(
        "SELECT id,kind,target_id,user_id,tenant_id,requested_at FROM deletion_jobs WHERE completed_at IS NULL ORDER BY deadline LIMIT 200",
    )
    .fetch_all(&s.db)
    .await?;
    let mut all_ok = true;
    for (id, kind, target, user, tenant, requested) in jobs {
        let mut tx = s.db.begin().await?;
        let result = purge(&mut tx, kind, target, user, tenant, requested).await;
        match result {
            Ok(()) => {
                sqlx::query(
                    "UPDATE deletion_jobs SET completed_at=$2,attempts=attempts+1 WHERE id=$1",
                )
                .bind(id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                tracing::info!(deletion_job_id = %id, kind, "deletion completed");
            }
            Err(e) => {
                drop(tx);
                all_ok = false;
                tracing::error!(deletion_job_id = %id, kind, error = %e, "deletion failed");
                sqlx::query("UPDATE deletion_jobs SET attempts=attempts+1,failure_category='persistence' WHERE id=$1")
                    .bind(id)
                    .execute(&s.db)
                    .await?;
            }
        }
    }
    Ok(all_ok)
}

/// Physically deletes the data of one deletion job from the primary database.
pub async fn purge(
    db: &mut PgConnection,
    kind: i16,
    target: Option<Uuid>,
    user: Option<Uuid>,
    tenant: Option<Uuid>,
    requested: DateTime<Utc>,
) -> anyhow::Result<()> {
    let k = |d: DeletionKind| d as i16;
    if kind == k(DeletionKind::Connection)
        || kind == k(DeletionKind::Device)
        || kind == k(DeletionKind::Dashboard)
    {
        let target = target.ok_or_else(|| anyhow::anyhow!("missing target"))?;
        let (table, source) = if kind == k(DeletionKind::Connection) {
            ("connections", Some(SOURCE_CONNECTION))
        } else if kind == k(DeletionKind::Device) {
            ("devices", Some(SOURCE_DEVICE))
        } else {
            ("dashboards", None)
        };
        if let Some(source) = source {
            sqlx::query("DELETE FROM widget_sources WHERE source_kind=$1 AND source_id=$2")
                .bind(source)
                .bind(target)
                .execute(&mut *db)
                .await?;
        }
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE id=$1 AND deleted_at IS NOT NULL"
        ))
        .bind(target)
        .execute(&mut *db)
        .await?;
        return Ok(());
    }
    if kind == k(DeletionKind::Tenant) {
        sqlx::query("DELETE FROM dashboards WHERE tenant_id=$1 AND deleted_at IS NOT NULL")
            .bind(tenant)
            .execute(&mut *db)
            .await?;
        return Ok(());
    }
    let user = user.ok_or_else(|| anyhow::anyhow!("missing user"))?;
    let withdrawal = kind == k(DeletionKind::Withdrawal);
    // Entities blocked by this request; withdrawal removes everything of the user.
    let cutoff = if withdrawal {
        DateTime::<Utc>::MAX_UTC
    } else {
        requested
    };
    sqlx::query(
        "DELETE FROM widget_sources s USING connections c WHERE s.source_kind=$1 AND s.source_id=c.id AND c.user_id=$2 AND c.deleted_at<=$3",
    )
    .bind(SOURCE_CONNECTION)
    .bind(user)
    .bind(cutoff)
    .execute(&mut *db)
    .await?;
    sqlx::query(
        "DELETE FROM widget_sources s USING devices d WHERE s.source_kind=$1 AND s.source_id=d.id AND d.user_id=$2 AND d.deleted_at<=$3",
    )
    .bind(SOURCE_DEVICE)
    .bind(user)
    .bind(cutoff)
    .execute(&mut *db)
    .await?;
    sqlx::query("DELETE FROM connections WHERE user_id=$1 AND deleted_at<=$2")
        .bind(user)
        .bind(cutoff)
        .execute(&mut *db)
        .await?;
    sqlx::query("DELETE FROM devices WHERE user_id=$1 AND deleted_at<=$2")
        .bind(user)
        .bind(cutoff)
        .execute(&mut *db)
        .await?;
    sqlx::query(
        "DELETE FROM dashboards WHERE space_kind=$3 AND deleted_at<=$2 AND tenant_id=(SELECT personal_tenant_id FROM accounts WHERE user_id=$1)",
    )
    .bind(user)
    .bind(cutoff)
    .bind(e16(SpaceKind::Personal))
    .execute(&mut *db)
    .await?;
    // Every team widget the user owned at the request, whatever its sharing state: one that had
    // already ended for another reason (revocation, source deletion, leaving) keeps that reason
    // but must still be deleted. Widgets added after the request are new data and stay.
    sqlx::query(
        "DELETE FROM widgets w USING dashboards d WHERE w.dashboard_id=d.id AND d.space_kind=$2 AND w.data_owner_user_id=$1 AND w.created_at<=$3",
    )
    .bind(user)
    .bind(e16(SpaceKind::Team))
    .bind(cutoff)
    .execute(&mut *db)
    .await?;
    // Activity visible only to the user; the deletion request event itself remains until expiry.
    sqlx::query("DELETE FROM audit_events WHERE subject_user_id=$1 AND tenant_id IS NULL AND (occurred_at<$2 OR $3)")
        .bind(user)
        .bind(requested)
        .bind(withdrawal)
        .execute(&mut *db)
        .await?;
    sqlx::query("DELETE FROM request_ids WHERE caller_id=$1")
        .bind(format!("user:{user}"))
        .execute(&mut *db)
        .await?;
    if withdrawal {
        sqlx::query("DELETE FROM accounts WHERE user_id=$1")
            .bind(user)
            .execute(&mut *db)
            .await?;
    }
    Ok(())
}

async fn retention(s: &Server, now: DateTime<Utc>) -> anyhow::Result<()> {
    let days90 = now - Duration::days(time::RETENTION_DAYS);
    let statements: [(&str, DateTime<Utc>); 10] = [
        (
            "DELETE FROM usage_buckets WHERE bucket_start<$1",
            time::retention_start(now),
        ),
        ("DELETE FROM metric_hourly WHERE hour_start<$1", days90),
        (
            "DELETE FROM device_rotated_refresh_tokens WHERE rotated_at<$1",
            days90,
        ),
        (
            "DELETE FROM audit_events WHERE occurred_at<$1",
            now - Duration::days(30),
        ),
        (
            "DELETE FROM price_sync_runs WHERE started_at<$1",
            now - Duration::days(30),
        ),
        (
            "DELETE FROM deletion_jobs WHERE completed_at<$1",
            now - Duration::days(30),
        ),
        (
            "DELETE FROM request_ids WHERE created_at<$1",
            now - Duration::hours(24),
        ),
        (
            "DELETE FROM enrollments WHERE expires_at<$1",
            now - Duration::hours(24),
        ),
        (
            "DELETE FROM rate_limit_windows WHERE window_start<$1",
            now - Duration::minutes(2),
        ),
        (
            "DELETE FROM device_parser_status WHERE reported_at<$1",
            days90,
        ),
    ];
    for (sql, cutoff) in statements {
        let n = sqlx::query(sql)
            .bind(cutoff)
            .execute(&s.db)
            .await?
            .rows_affected();
        if n > 0 {
            tracing::info!(statement = sql, rows = n, "retention cleanup");
        }
    }
    Ok(())
}

async fn tenant_lifecycle(s: &Server, now: DateTime<Utc>, correlation: &str) -> anyhow::Result<()> {
    let personal: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT a.user_id,a.personal_tenant_id FROM accounts a WHERE NOT EXISTS \
         (SELECT 1 FROM deletion_jobs j WHERE j.kind=$1 AND j.user_id=a.user_id)",
    )
    .bind(DeletionKind::Withdrawal as i16)
    .fetch_all(&s.db)
    .await?;
    let teams: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT tenant_id FROM dashboards WHERE space_kind=$1 AND deleted_at IS NULL",
    )
    .bind(e16(SpaceKind::Team))
    .fetch_all(&s.db)
    .await?;
    let ids: Vec<String> = personal
        .iter()
        .map(|p| p.1.to_string())
        .chain(teams.iter().map(Uuid::to_string))
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let states = s
        .delcore
        .states(&ids)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for ((user, tenant), state) in personal.iter().zip(&states) {
        if *state != TenantState::Deleted {
            continue;
        }
        let mut tx = s.db.begin().await?;
        crate::rpc::account::block_user_data(
            &mut tx,
            *user,
            now,
            correlation,
            0,
            SharingEndReason::OwnerDataDeleted,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        crate::rpc::enqueue_deletion(
            &mut tx,
            DeletionKind::Withdrawal,
            None,
            Some(*user),
            Some(*tenant),
            now,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        tx.commit().await?;
        tracing::info!(user_id = %user, "user withdrawal detected; AnyDash data deletion enqueued");
    }
    for (tenant, state) in teams.iter().zip(&states[personal.len()..]) {
        if *state != TenantState::Deleted {
            continue;
        }
        let mut tx = s.db.begin().await?;
        sqlx::query(
            "UPDATE dashboards SET deleted_at=$2 WHERE tenant_id=$1 AND deleted_at IS NULL",
        )
        .bind(tenant)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        crate::rpc::enqueue_deletion(
            &mut tx,
            DeletionKind::Tenant,
            None,
            None,
            Some(*tenant),
            now,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        tx.commit().await?;
        tracing::info!(tenant_id = %tenant, "team tenant deleted; team dashboards deletion enqueued");
    }
    Ok(())
}

async fn credential_expiry(s: &Server, now: DateTime<Utc>) -> anyhow::Result<()> {
    let mut tx = s.db.begin().await?;
    let expired: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE devices d SET credential_state=$2 FROM device_credentials c \
         WHERE c.device_id=d.id AND c.refresh_expires_at<=$1 AND d.credential_state=$3 RETURNING d.id",
    )
    .bind(now)
    .bind(e16(DeviceCredentialState::Expired))
    .bind(e16(DeviceCredentialState::Active))
    .fetch_all(&mut *tx)
    .await?;
    for device in &expired {
        crate::rpc::end_device_leases(&mut tx, *device)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    tx.commit().await?;
    if !expired.is_empty() {
        tracing::info!(
            devices = expired.len(),
            "device credentials expired after 90 days without an upload"
        );
    }
    Ok(())
}

/// Refreshes the database-backed gauges before a scrape.
pub async fn refresh_gauges(s: &Server) -> anyhow::Result<()> {
    let now = time::now();
    let (pending, oldest): (i64, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT count(*),min(requested_at) FROM deletion_jobs WHERE completed_at IS NULL",
    )
    .fetch_one(&s.db)
    .await?;
    s.metrics.deletion_pending.set(pending);
    s.metrics.deletion_oldest_age.set(
        oldest
            .map(|o| (now - o).num_milliseconds() as f64 / 1000.0)
            .unwrap_or(0.0),
    );
    let last_price: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT max(started_at) FROM price_sync_runs WHERE outcome IN (1,2)")
            .fetch_one(&s.db)
            .await?;
    s.metrics
        .price_last_success
        .set(last_price.map(|t| t.timestamp() as f64).unwrap_or(0.0));
    let jobs = sqlx::query("SELECT job,last_success_at FROM worker_job_runs")
        .fetch_all(&s.db)
        .await?;
    for r in jobs {
        if let Some(t) = r.get::<Option<DateTime<Utc>>, _>("last_success_at") {
            s.metrics
                .job_last_success
                .with_label_values(&[r.get::<String, _>("job").as_str()])
                .set(t.timestamp() as f64);
        }
    }
    let parsers: Vec<(i16, i16, i64)> = sqlx::query_as(
        "SELECT p.tool,p.state,count(*) FROM device_parser_status p JOIN devices d ON d.id=p.device_id \
         WHERE d.deleted_at IS NULL AND d.last_upload_at>$1 GROUP BY p.tool,p.state",
    )
    .bind(now - Duration::hours(1))
    .fetch_all(&s.db)
    .await?;
    s.metrics.parser_devices.reset();
    for (tool, state, n) in parsers {
        let tool: delino_protocol::anydash::v1::LocalTool = crate::rpc::enum_of(tool);
        let state: ParserState = crate::rpc::enum_of(state);
        s.metrics
            .parser_devices
            .with_label_values(&[&label(tool, "LOCAL_TOOL_"), &label(state, "PARSER_STATE_")])
            .set(n as f64);
    }
    let leases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM connections WHERE lease_device_id IS NOT NULL AND lease_expires_at>$1 AND deleted_at IS NULL",
    )
    .bind(now)
    .fetch_one(&s.db)
    .await?;
    s.metrics.leases_active.set(leases);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    fn at(d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, d, h, 0, 0).unwrap()
    }
    #[test]
    fn price_sync_runs_daily_at_three_and_retries_hourly() {
        assert_eq!(price_boundary(at(5, 2)), at(4, 3));
        assert_eq!(price_boundary(at(5, 3)), at(5, 3));
        // First start: due immediately.
        assert!(due(Job::PriceSync, at(5, 10), None, None, None));
        // Succeeded after today's boundary: not due until tomorrow 03:00.
        assert!(!due(Job::PriceSync, at(5, 23), None, Some(at(5, 3)), None));
        assert!(due(Job::PriceSync, at(6, 3), None, Some(at(5, 3)), None));
        // Failed at 03:00: retry at 04:00, not before.
        assert!(!due(
            Job::PriceSync,
            at(5, 3) + Duration::minutes(30),
            None,
            Some(at(4, 3)),
            Some(at(5, 3))
        ));
        assert!(due(
            Job::PriceSync,
            at(5, 4),
            None,
            Some(at(4, 3)),
            Some(at(5, 3))
        ));
        // Fixed cadence jobs.
        assert!(due(
            Job::LeaseExpiry,
            at(5, 3),
            Some(at(5, 3) - Duration::seconds(30)),
            None,
            None
        ));
        assert!(!due(
            Job::Retention,
            at(5, 3),
            Some(at(5, 3) - Duration::minutes(10)),
            None,
            None
        ));
    }
}
