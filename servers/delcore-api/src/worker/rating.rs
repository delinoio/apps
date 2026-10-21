// SPDX-License-Identifier: FSL-1.1-ALv2

use crate::{events, postgres, rating};
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use std::collections::HashSet;

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
