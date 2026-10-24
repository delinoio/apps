// SPDX-License-Identifier: FSL-1.1-ALv2
use chrono::{DateTime, Datelike, Duration, Months, TimeZone, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug)]
pub struct PeriodUnavailable;
impl std::fmt::Display for PeriodUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("historical billing period unavailable")
    }
}
impl std::error::Error for PeriodUnavailable {}

/// Only trusted SQL aliases and expressions may be passed to these helpers.
pub fn selected_plan_bool_sql(tenant: &str, plan: &str, field: &str) -> String {
    assert!(
        matches!(field, "free" | "postpaid"),
        "invalid plan policy field"
    );
    format!(
        "COALESCE((SELECT (cv.definition->>'{field}')::boolean FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.tenant_id={tenant}.id AND cs.product_id={tenant}.plan_id AND cs.ends_at IS NULL),{plan}.{field})"
    )
}
/// Reports whether the tenant's plan was postpaid at the instant `at`.
///
/// A billing period records its policy when it is created, so a period recovered from a late
/// payment must record the policy that applied when it began, not the policy of today's plan:
/// the plan selection pinned at `at` wins, then the plan owning a recurring price the tenant held
/// at `at` (tenants without pinned history), then the tenant's current plan.
pub async fn postpaid_at(
    db: &mut PgConnection,
    tenant: &str,
    at: DateTime<Utc>,
) -> anyhow::Result<bool> {
    sqlx::query_scalar("SELECT COALESCE((SELECT (cv.definition->>'postpaid')::boolean FROM tenant_catalog_selections cs JOIN catalog_entities ce ON ce.id=cs.product_id JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.tenant_id=t.id AND ce.kind='plan' AND cs.starts_at<=$2 AND (cs.ends_at IS NULL OR cs.ends_at>$2) ORDER BY cs.starts_at DESC LIMIT 1),(SELECT pl.postpaid FROM entitlements en JOIN prices pr ON pr.id=en.price_id JOIN plans pl ON pl.id=pr.plan_id WHERE en.tenant_id=t.id AND pr.kind=2 AND en.starts_at<=$2 AND (en.ends_at IS NULL OR en.ends_at>$2) ORDER BY en.starts_at DESC LIMIT 1),p.postpaid) FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1")
        .bind(Uuid::parse_str(tenant)?).bind(at).fetch_one(db).await.map_err(Into::into)
}
pub fn selected_plan_rank_sql(tenant: &str, plan: &str) -> String {
    format!(
        "COALESCE((SELECT (cv.definition->>'rank')::integer FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.tenant_id={tenant}.id AND cs.product_id={tenant}.plan_id AND cs.ends_at IS NULL),{plan}.rank)"
    )
}
pub fn period_expired_sql(tenant: &str, plan: &str, now: &str) -> String {
    format!(
        "({tenant}.period_end<={now} AND (NOT {} OR {tenant}.external_subscription_id<>''))",
        selected_plan_bool_sql(tenant, plan, "free")
    )
}
/// Resolves the billing period that applies to `tenant` at the instant `at`.
///
/// Applicability is established from pinned history before any recorded Paddle period is
/// accepted: a recorded period only proves that coverage was paid for, not that the paid
/// subscription still applies at `at` (a confirmed early cancellation ends the selection and
/// the entitlements while the recorded row is retained for historical accounting).
/// - A free base plan without an active recurring add-on entitlement uses the free period of
///   [`free_period`]: the UTC calendar month, or the remainder of that month from the
///   cancellation instant when a retained paid period starts at the month start.
///   Recurring prices owned by the free base plan itself are not add-ons: tenant creation and
///   cancellation activation grant them as entitlements, and they require no Paddle coverage.
/// - A paid base plan, or a free base plan with an active recurring add-on, uses the recorded
///   Paddle period that contains `at`, and fails with `PeriodUnavailable` when none exists.
///
/// Starts are inclusive and ends are exclusive, matching the other history queries.
pub async fn billing_period(
    db: &mut PgConnection,
    tenant: &str,
    at: DateTime<Utc>,
) -> anyhow::Result<(DateTime<Utc>, DateTime<Utc>)> {
    let tenant_id = Uuid::parse_str(tenant)?;
    // Legacy tenants without pinned history fall back to `plans.free`; current tenant state
    // never substitutes for historical evidence. A tenant that has pinned history but is asked
    // about an instant before its first pinned plan selection (the legacy snapshot only
    // backdates to the current period) is paid at that instant when a paid plan entitlement
    // covers it, so its retained recorded periods apply instead of the current plan's calendar
    // month.
    let applicability: Option<(bool, bool)> = sqlx::query_as("SELECT COALESCE((SELECT (cv.definition->>'free')::boolean FROM tenant_catalog_selections cs JOIN catalog_entities ce ON ce.id=cs.product_id JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.tenant_id=t.id AND ce.kind='plan' AND cs.starts_at<=$2 AND (cs.ends_at IS NULL OR cs.ends_at>$2) ORDER BY cs.starts_at DESC LIMIT 1),CASE WHEN EXISTS(SELECT 1 FROM tenant_catalog_selections cs JOIN catalog_entities ce ON ce.id=cs.product_id WHERE cs.tenant_id=t.id AND ce.kind='plan') AND NOT EXISTS(SELECT 1 FROM tenant_catalog_selections cs JOIN catalog_entities ce ON ce.id=cs.product_id WHERE cs.tenant_id=t.id AND ce.kind='plan' AND cs.starts_at<=$2) AND EXISTS(SELECT 1 FROM entitlements en JOIN prices pr ON pr.id=en.price_id JOIN plans pl ON pl.id=pr.plan_id WHERE en.tenant_id=t.id AND pl.free IS FALSE AND en.starts_at<=$2 AND (en.ends_at IS NULL OR en.ends_at>$2)) THEN false ELSE p.free END), EXISTS(SELECT 1 FROM entitlements en JOIN prices pr ON pr.id=en.price_id WHERE en.tenant_id=t.id AND pr.addon_id IS NOT NULL AND pr.kind=2 AND en.starts_at<=$2 AND (en.ends_at IS NULL OR en.ends_at>$2)) FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1")
        .bind(tenant_id).bind(at).fetch_optional(&mut *db).await?;
    let Some((free, recurring)) = applicability else {
        return Err(PeriodUnavailable.into());
    };
    if !free || recurring {
        return sqlx::query_as("SELECT starts_at,ends_at FROM billing_periods WHERE tenant_id=$1 AND starts_at<=$2 AND ends_at>$2 ORDER BY starts_at DESC LIMIT 1")
            .bind(tenant_id).bind(at).fetch_optional(db).await?
            .ok_or_else(|| PeriodUnavailable.into());
    }
    let period = free_period(db, tenant, at).await?;
    Ok((period.start, period.end))
}

/// The free usage period that applies at an instant, together with the key that identifies the
/// allowance grant covering it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreePeriod {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Source key of the free allowance grant for exactly this period.
    pub source_key: String,
}

/// Resolves the free usage period for `tenant` at the instant `at`.
///
/// Metering aggregates, allowance grants, ledger charges, rated periods and the postpaid
/// policy lookup are keyed by `(tenant, period_start)`. A free period must therefore never
/// share its start with a retained paid period. Every writer and reader of the free period,
/// that is billing-period resolution, cancellation activation and the monthly free renewal,
/// uses this function so the aggregate, the grant and the ledger rows agree on one key.
///
/// - The free period is the UTC calendar month containing `at`.
/// - When a retained paid period of the tenant starts exactly at that month start, the free
///   period starts at the instant paid applicability ended instead (the confirmed
///   cancellation: the end of the last paid plan selection, paid plan entitlement or recurring
///   add-on entitlement) and runs to the end of the same month. It keeps the full free
///   allowance and is not prorated. If that instant is not after the month start, which needs a
///   paid period recorded for a renewal that follows the cancellation, the period starts one
///   microsecond after the month start so the key still differs.
pub async fn free_period(
    db: &mut PgConnection,
    tenant: &str,
    at: DateTime<Utc>,
) -> anyhow::Result<FreePeriod> {
    let month = Utc
        .with_ymd_and_hms(at.year(), at.month(), 1, 0, 0, 0)
        .single()
        .ok_or(PeriodUnavailable)?;
    let end = month
        .checked_add_months(Months::new(1))
        .ok_or(PeriodUnavailable)?;
    let (collides, ended): (bool, Option<DateTime<Utc>>) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM billing_periods WHERE tenant_id=$1 AND starts_at=$2), GREATEST((SELECT max(cs.ends_at) FROM tenant_catalog_selections cs JOIN catalog_entities ce ON ce.id=cs.product_id JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.tenant_id=$1 AND ce.kind='plan' AND (cv.definition->>'free')::boolean IS FALSE AND cs.ends_at<=$3), (SELECT max(en.ends_at) FROM entitlements en JOIN prices pr ON pr.id=en.price_id LEFT JOIN plans pl ON pl.id=pr.plan_id WHERE en.tenant_id=$1 AND en.ends_at<=$3 AND (pr.addon_id IS NOT NULL AND pr.kind=2 OR pl.free IS FALSE)))")
        .bind(Uuid::parse_str(tenant)?)
        .bind(month)
        .bind(at)
        .fetch_one(&mut *db)
        .await?;
    if !collides {
        return Ok(FreePeriod {
            start: month,
            end,
            source_key: format!("free:{tenant}:{}", month.format("%Y-%m")),
        });
    }
    let start = ended
        .filter(|ended| *ended > month)
        .unwrap_or(month + Duration::microseconds(1));
    Ok(FreePeriod {
        start,
        end,
        source_key: format!(
            "free:{tenant}:{}:{}",
            month.format("%Y-%m"),
            start.timestamp_micros()
        ),
    })
}

/// Moves free allowance grants away from the start of a paid period that takes over.
///
/// A free grant is keyed by `(tenant, period_start)` like every other row of a period. When a
/// paid period is recorded whose start equals the start of an earlier free grant, for example a
/// tenant that already received its monthly grant before a subscription whose provider period
/// begins at the same UTC month start, rating would sum the free allowance into the paid
/// period. The free period of that month is therefore re-derived the way [`free_period`] derives
/// it once paid coverage is retained: it starts one microsecond after the colliding start and
/// keeps its full amount, so no free allowance is keyed by the paid period start. The grant
/// keeps its identity and amount for the audit trail, and a later cancellation that resolves to
/// the same key replays this grant instead of granting the allowance twice.
///
/// Rated periods of the paid period are reconciled afterwards, so usage that was rated against
/// the free allowance is charged against the paid allowance only. Call this after the paid
/// allowance grant was recorded: the grant already reduces the charge, and this call appends
/// only the remaining difference.
pub async fn separate_free_grants(
    db: &mut PgConnection,
    tenant: &str,
    paid_start: DateTime<Utc>,
) -> anyhow::Result<()> {
    let tenant_id = Uuid::parse_str(tenant)?;
    let prefix = format!("free:{tenant}:");
    let rows: Vec<(Uuid, Uuid, String, String)> = sqlx::query_as("SELECT id,service_id,meter,source_key FROM allowance_grants WHERE tenant_id=$1 AND period_start=$2 AND starts_with(source_key,$3) ORDER BY id")
        .bind(tenant_id)
        .bind(paid_start)
        .bind(&prefix)
        .fetch_all(&mut *db)
        .await?;
    let start = paid_start + Duration::microseconds(1);
    for (id, service, meter, source_key) in rows {
        // Keys read `free:<tenant>:<year-month>[:<period start micros>]:<service>:<meter>`.
        let identity = format!(":{service}:{meter}");
        let Some(base) = source_key.strip_suffix(&identity) else {
            tracing::warn!(
                tenant_id = %tenant,
                grant_id = %id,
                "free allowance grant key does not match its meter; left in place"
            );
            continue;
        };
        let month: Vec<&str> = base.splitn(4, ':').take(3).collect();
        let key = format!("{}:{}", month.join(":"), start.timestamp_micros());
        let moved_key = format!("{key}{identity}");
        let replayed: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM allowance_grants WHERE source_key=$1)")
                .bind(&moved_key)
                .fetch_one(&mut *db)
                .await?;
        if replayed {
            // The grant of the derived period already exists, so this one only repeats it.
            sqlx::query("DELETE FROM allowance_grants WHERE id=$1")
                .bind(id)
                .execute(&mut *db)
                .await?;
        } else {
            sqlx::query("UPDATE allowance_grants SET period_start=$2,source_key=$3 WHERE id=$1")
                .bind(id)
                .bind(start)
                .bind(&moved_key)
                .execute(&mut *db)
                .await?;
        }
        tracing::info!(
            tenant_id = %tenant,
            grant_id = %id,
            replayed,
            "moved free allowance grant off a paid period start"
        );
        super::rating::reconcile_granted_periods(
            db,
            &format!("{id}:retired"),
            tenant,
            &service.to_string(),
            &meter,
            paid_start,
            Utc::now(),
        )
        .await?;
    }
    Ok(())
}
