// SPDX-License-Identifier: FSL-1.1-ALv2
//! Recovery of free calendar-month allowance baselines that no maintenance pass committed.
//!
//! Monthly free renewal only grants the month that contains the maintenance instant. Usage whose
//! original period is an earlier, wholly skipped month keeps that period, so rating finds no
//! allowance for it and would consume purchased credit. [`recover_free_period`] inserts the
//! baseline grants such a month should have received, derived only from the history retained for
//! the month boundary.
use super::{allowances, period};
use chrono::{DateTime, Datelike, Months, TimeZone, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

/// Retained history that a recovery needs is contradictory or malformed. Nothing is granted for
/// the period; callers report the condition instead of guessing a benefit or billing silently
/// without an allowance. Recovery raises this before it writes, so the caller's transaction is
/// still usable afterwards.
#[derive(Debug)]
pub struct HistoryInvalid(pub &'static str);
impl std::fmt::Display for HistoryInvalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "free allowance history is inconsistent: {}", self.0)
    }
}
impl std::error::Error for HistoryInvalid {}

/// Why a period needs no recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// The period is not a whole UTC calendar month. Creation, cancellation and refresh own the
    /// grants of every other free period shape, and paid periods come from provider coverage.
    NotCalendarMonth,
    /// A monthly baseline grant is already recorded for the period.
    AlreadyInitialized,
    /// The tenant was not on a free plan without a recurring add-on when the month began.
    NotFree,
    /// A retained paid period starts with the month, so the free period starts later and is owned
    /// by the cancellation that created it.
    PaidPeriodCollision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// The number of baseline grants recorded by this call. Zero means nothing was held at the
    /// month boundary, or every grant was already recorded.
    Granted(usize),
    Skipped(Skip),
}

/// Records the missing monthly free baseline of the calendar month `[start, end)` for `tenant`.
///
/// The baseline is what the tenant held at the month boundary: the usage allowances of the
/// entitlements whose validity window contains `start`. A meter or product added later, a month
/// before the tenant existed and a month that was paid at its start therefore receive nothing,
/// and mid-month changes keep the prorated difference grants they already recorded. Each grant
/// uses the stable `free:<tenant>:<year-month>:<service>:<meter>` source key with the month start
/// as its application time, so usage of the month can consume it, and the grant helper's
/// conflict handling makes a repeat add nothing. A newly recorded grant re-rates the tenant's
/// already rated periods and appends one period-attributed correction.
pub async fn recover_free_period(
    db: &mut PgConnection,
    tenant: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> anyhow::Result<Recovery> {
    let month = Utc
        .with_ymd_and_hms(start.year(), start.month(), 1, 0, 0, 0)
        .single();
    if month != Some(start) || start.checked_add_months(Months::new(1)) != Some(end) {
        return Ok(Recovery::Skipped(Skip::NotCalendarMonth));
    }
    let tenant_id = Uuid::parse_str(tenant)?;
    let prefix = format!("free:{tenant}:{}:", start.format("%Y-%m"));
    let initialized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM allowance_grants WHERE tenant_id=$1 AND period_start=$2 AND starts_with(source_key,$3))")
        .bind(tenant_id)
        .bind(start)
        .bind(&prefix)
        .fetch_one(&mut *db)
        .await?;
    if initialized {
        return Ok(Recovery::Skipped(Skip::AlreadyInitialized));
    }
    // The tenant must be on a free plan without a recurring add-on at the boundary. A tenant that
    // had no pinned history yet falls back to the plan's free flag, exactly as billing-period
    // resolution does; a tenant that did not exist then holds no entitlement and gets no grant.
    match period::period_applicability(db, tenant, start).await? {
        Some((true, false)) => {}
        _ => return Ok(Recovery::Skipped(Skip::NotFree)),
    }
    let free = period::free_period(db, tenant, start).await?;
    if free.start != start || free.end != end {
        return Ok(Recovery::Skipped(Skip::PaidPeriodCollision));
    }
    let selections: i64 = sqlx::query_scalar("SELECT count(*) FROM tenant_catalog_selections cs JOIN catalog_entities ce ON ce.id=cs.product_id WHERE cs.tenant_id=$1 AND ce.kind='plan' AND cs.starts_at<=$2 AND (cs.ends_at IS NULL OR cs.ends_at>$2)")
        .bind(tenant_id)
        .bind(start)
        .fetch_one(&mut *db)
        .await?;
    if selections > 1 {
        return Err(HistoryInvalid("several plan selections overlap the month start").into());
    }
    // Only the pinned policy is historical evidence. A recurring entitlement without a
    // matching free selection is contradictory and cannot receive a free allowance.
    let paid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM entitlements en JOIN prices pr ON pr.id=en.price_id JOIN plans pl ON pl.id=pr.plan_id WHERE en.tenant_id=$1 AND pr.kind=2 AND (SELECT (cv.definition->>'free')::boolean FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.tenant_id=en.tenant_id AND cs.product_id=pl.id AND cs.starts_at<=$2 AND (cs.ends_at IS NULL OR cs.ends_at>$2) ORDER BY cs.starts_at DESC LIMIT 1) IS NOT TRUE AND en.starts_at<=$2 AND (en.ends_at IS NULL OR en.ends_at>$2))")
        .bind(tenant_id)
        .bind(start)
        .fetch_one(&mut *db)
        .await?;
    if paid {
        return Err(
            HistoryInvalid("a paid plan entitlement is held at the start of a free month").into(),
        );
    }
    // Read the held allowances first so a malformed stored amount is reported as invalid history
    // while the transaction is still usable; database failures stay ordinary errors.
    if let Err(error) = allowances::allowances_held_at(db, tenant, start).await {
        return Err(if error.downcast_ref::<sqlx::Error>().is_some() {
            error
        } else {
            HistoryInvalid("a held allowance is not a valid amount").into()
        });
    }
    let granted =
        allowances::grant_allowances_held_at(db, tenant, start, end, start, &free.source_key)
            .await?;
    Ok(Recovery::Granted(granted))
}
