// SPDX-License-Identifier: FSL-1.1-ALv2

use crate::{events, postgres, rating};
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use std::collections::HashSet;

/// Record the free monthly baseline the applied event's period should already have, before the
/// period is rated, so rating never charges purchased credit for usage the free plan covers.
///
/// Maintenance only grants the month that contains its own run, so an event of a wholly skipped
/// earlier month would otherwise find no allowance. The grants share the event transaction: a
/// rollback leaves both the recovery and the rating for the retry. Contradictory retained history
/// fails the metering work permanently, which the existing failure reporting surfaces.
pub(super) async fn recover_allowances(
    tx: &mut PgConnection,
    applied: &events::Applied,
) -> Result<()> {
    if applied.changes.is_empty() {
        return Ok(());
    }
    match postgres::recovery::recover_free_period(
        tx,
        &applied.tenant_id,
        applied.period_start,
        applied.period_end,
    )
    .await
    {
        Ok(postgres::recovery::Recovery::Granted(grants)) if grants > 0 => {
            tracing::info!(
                category = "allowance_recovery",
                grants,
                "recovered missed free allowance baseline before rating"
            );
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(error)
            if error
                .downcast_ref::<postgres::recovery::HistoryInvalid>()
                .is_some() =>
        {
            tracing::error!(
                category = "allowance_recovery_invalid",
                reason = %error,
                "free allowance history is inconsistent; metering work failed"
            );
            Err(error.context(events::permanent("inconsistent free allowance history")))
        }
        Err(error) => Err(error),
    }
}

/// Append each changed meter period's charge difference in the event transaction.
/// The source identifies the event revision, so committed work cannot charge twice.
pub(super) async fn rate_metering(
    tx: &mut PgConnection,
    now: DateTime<Utc>,
    applied: &events::Applied,
) -> Result<usize> {
    let mut seen = HashSet::new();
    let mut entries = 0;
    for change in &applied.changes {
        if !seen.insert(&change.meter_id) {
            continue;
        }
        let period = postgres::MeterPeriod {
            tenant_id: applied.tenant_id.clone(),
            service_id: applied.service_id.clone(),
            meter_id: change.meter_id.clone(),
            meter_key: change.change.meter_key.clone(),
            start: applied.period_start,
            end: applied.period_end,
        };
        let kind = if applied.to_revision > 1 {
            postgres::LedgerKind::Correction
        } else {
            postgres::LedgerKind::Usage
        };
        let source = format!(
            "rating:{}:{}:{}",
            applied.event_id, applied.to_revision, change.meter_id
        );
        let delta = postgres::reconcile_meter_period(tx, &period, kind, &source, now)
            .await
            .map_err(|error| {
                if error.downcast_ref::<rating::Inconsistent>().is_some() {
                    error.context(events::permanent("inconsistent rating state"))
                } else {
                    error
                }
            })?;
        if !delta.is_zero() {
            entries += 1;
        }
    }
    Ok(entries)
}
