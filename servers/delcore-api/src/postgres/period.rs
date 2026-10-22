// SPDX-License-Identifier: FSL-1.1-ALv2
use chrono::{DateTime, Datelike, Months, TimeZone, Utc};
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
/// - A free base plan without an active recurring entitlement uses the UTC calendar month.
/// - A paid base plan, or a free base plan with an active recurring add-on, uses the recorded
///   Paddle period that contains `at`, and fails with `PeriodUnavailable` when none exists.
///
/// Starts are inclusive and ends are exclusive, matching the other history queries.
pub async fn billing_period(
    db: &mut PgConnection,
    tenant: &str,
    at: DateTime<Utc>,
) -> anyhow::Result<(DateTime<Utc>, DateTime<Utc>)> {
    let tenant = Uuid::parse_str(tenant)?;
    // Legacy tenants without pinned history fall back to `plans.free`; current tenant state
    // never substitutes for historical evidence.
    let applicability: Option<(bool, bool)> = sqlx::query_as("SELECT COALESCE((SELECT (cv.definition->>'free')::boolean FROM tenant_catalog_selections cs JOIN catalog_entities ce ON ce.id=cs.product_id JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.tenant_id=t.id AND ce.kind='plan' AND cs.starts_at<=$2 AND (cs.ends_at IS NULL OR cs.ends_at>$2) ORDER BY cs.starts_at DESC LIMIT 1),p.free), EXISTS(SELECT 1 FROM entitlements en JOIN prices pr ON pr.id=en.price_id WHERE en.tenant_id=t.id AND pr.kind=2 AND en.starts_at<=$2 AND (en.ends_at IS NULL OR en.ends_at>$2)) FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1")
        .bind(tenant).bind(at).fetch_optional(&mut *db).await?;
    let Some((free, recurring)) = applicability else {
        return Err(PeriodUnavailable.into());
    };
    if !free || recurring {
        return sqlx::query_as("SELECT starts_at,ends_at FROM billing_periods WHERE tenant_id=$1 AND starts_at<=$2 AND ends_at>$2 ORDER BY starts_at DESC LIMIT 1")
            .bind(tenant).bind(at).fetch_optional(db).await?
            .ok_or_else(|| PeriodUnavailable.into());
    }
    let start = Utc
        .with_ymd_and_hms(at.year(), at.month(), 1, 0, 0, 0)
        .single()
        .ok_or(PeriodUnavailable)?;
    Ok((
        start,
        start
            .checked_add_months(Months::new(1))
            .ok_or(PeriodUnavailable)?,
    ))
}
