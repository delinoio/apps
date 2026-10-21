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
pub async fn billing_period(
    db: &mut PgConnection,
    tenant: &str,
    at: DateTime<Utc>,
) -> anyhow::Result<(DateTime<Utc>, DateTime<Utc>)> {
    let tenant = Uuid::parse_str(tenant)?;
    if let Some(period) = sqlx::query_as("SELECT starts_at,ends_at FROM billing_periods WHERE tenant_id=$1 AND starts_at<=$2 AND ends_at>$2 ORDER BY starts_at DESC LIMIT 1")
        .bind(tenant).bind(at).fetch_optional(&mut *db).await? { return Ok(period); }
    let free: Option<bool> = sqlx::query_scalar("SELECT COALESCE((SELECT (cv.definition->>'free')::boolean FROM tenant_catalog_selections cs JOIN catalog_entities ce ON ce.id=cs.product_id JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.tenant_id=t.id AND ce.kind='plan' AND cs.starts_at<=$2 AND (cs.ends_at IS NULL OR cs.ends_at>$2) ORDER BY cs.starts_at DESC LIMIT 1),p.free) FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1")
        .bind(tenant).bind(at).fetch_optional(db).await?;
    if free != Some(true) {
        return Err(PeriodUnavailable.into());
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
