// SPDX-License-Identifier: FSL-1.1-ALv2
//! 30-day retention. Every record expires 30 days after its own immutable `created_at`, in every
//! processing state. Retries and state changes never extend it, and there is no hold for
//! unresolved executions or undelivered usage: the requester accepts possible duplicate execution
//! after deduplication expiry and permanent loss of pending usage.
use crate::error::Result;
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

pub const RETENTION_DAYS: i64 = 30;
/// Rows deleted per statement, so a long backlog does not hold one huge transaction.
const BATCH: i64 = 1000;

/// Tables with the 30-day creation-time retention, in deletion order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedTable {
    IdempotencyRecords,
    Attempts,
    Requests,
    AuditRecords,
    OperationalEvents,
}
impl RetainedTable {
    pub const ALL: [Self; 5] = [
        Self::IdempotencyRecords,
        Self::Attempts,
        Self::Requests,
        Self::AuditRecords,
        Self::OperationalEvents,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IdempotencyRecords => "idempotency_records",
            Self::Attempts => "attempts",
            Self::Requests => "requests",
            Self::AuditRecords => "audit_records",
            Self::OperationalEvents => "operational_events",
        }
    }
    /// Batched delete by creation time. Table and key column names are fixed identifiers, never
    /// caller input. `ctid` selects the batch because idempotency records have a composite key.
    fn delete_sql(self) -> String {
        let table = self.as_str();
        format!(
            "DELETE FROM {table} WHERE ctid IN (SELECT ctid FROM {table} WHERE created_at <= $1 LIMIT $2)"
        )
    }
}

/// Newest `created_at` that is already expired at `now`. A record created exactly 30 days ago is
/// expired.
pub fn cutoff(now: DateTime<Utc>) -> DateTime<Utc> {
    now - Duration::days(RETENTION_DAYS)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub deleted: Vec<(RetainedTable, u64)>,
}
impl Report {
    pub fn total(&self) -> u64 {
        self.deleted.iter().map(|(_, n)| n).sum()
    }
}

/// Delete expired rows from every retained table.
pub async fn sweep(pool: &PgPool, now: DateTime<Utc>) -> Result<Report> {
    let cutoff = cutoff(now);
    let mut report = Report::default();
    for table in RetainedTable::ALL {
        let sql = table.delete_sql();
        let mut total = 0;
        loop {
            let deleted = sqlx::query(&sql)
                .bind(cutoff)
                .bind(BATCH)
                .execute(pool)
                .await?
                .rows_affected();
            total += deleted;
            if (deleted as i64) < BATCH {
                break;
            }
        }
        report.deleted.push((table, total));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cutoff_is_exactly_thirty_days_and_inclusive() {
        let now = Utc::now();
        assert_eq!(now - cutoff(now), Duration::days(30));
        let sql = RetainedTable::Attempts.delete_sql();
        assert!(sql.contains("created_at <= $1"), "{sql}");
        // Retention never reads processing state, delivery state or update time.
        for table in RetainedTable::ALL {
            let sql = table.delete_sql();
            for forbidden in ["state", "updated_at", "delivered_at", "retry_at"] {
                assert!(!sql.contains(forbidden), "{sql}");
            }
        }
    }

    #[test]
    fn every_retained_table_is_covered_once() {
        let names: std::collections::HashSet<_> =
            RetainedTable::ALL.iter().map(|t| t.as_str()).collect();
        assert_eq!(names.len(), 5);
    }
}
