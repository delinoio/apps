// SPDX-License-Identifier: FSL-1.1-ALv2
use super::Worker;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use sqlx::PgConnection;

impl Worker {
    pub(super) async fn retain(&self, conn: &mut PgConnection, now: DateTime<Utc>) -> Result<()> {
        let transient_cutoff = now - Duration::seconds(crate::config::TRANSIENT_RETENTION_SECONDS);
        for table in ["audit", "requests"] {
            let removed = sqlx::query(&format!("DELETE FROM {table} WHERE created_at<=$1"))
                .bind(transient_cutoff)
                .execute(&mut *conn)
                .await?
                .rows_affected();
            if removed > 0 {
                tracing::info!(
                    category = "retention",
                    record_kind = table,
                    removed,
                    "expired transient records removed"
                );
            }
        }
        // Keep normalized quantities and attribution for the longer accounting
        // interval even after raw event attributes have expired.
        if self.config.accounting_retention_seconds <= 0
            || self.config.raw_usage_retention_seconds <= 0
        {
            return Ok(());
        }
        let cutoff = now
            - Duration::seconds(
                self.config
                    .raw_usage_retention_seconds
                    .max(self.config.accounting_retention_seconds),
            );
        sqlx::query("SELECT set_config('delcore.retention','on',true)")
            .execute(&mut *conn)
            .await?;
        for sql in [
            "INSERT INTO usage_replay SELECT service_id,event_key,id,fingerprint FROM usage_events WHERE tenant_id IN (SELECT id FROM tenants WHERE deleted_at<$1) ON CONFLICT DO NOTHING",
            "DELETE FROM ledger WHERE tenant_id IN (SELECT id FROM tenants WHERE deleted_at<$1)",
            "DELETE FROM usage_events WHERE tenant_id IN (SELECT id FROM tenants WHERE deleted_at<$1)",
            "UPDATE provider_transactions SET amount=0 WHERE tenant_id IN (SELECT id FROM tenants WHERE deleted_at<$1)",
            "DELETE FROM settlements WHERE tenant_id IN (SELECT id FROM tenants WHERE deleted_at<$1)",
        ] {
            sqlx::query(sql).bind(cutoff).execute(&mut *conn).await?;
        }
        let raw_cutoff = now - Duration::seconds(self.config.raw_usage_retention_seconds);
        let cleared = crate::events::remove_attributes(conn, raw_cutoff).await?;
        if cleared > 0 {
            tracing::info!(
                category = "retention",
                revisions = cleared,
                "raw metering attributes removed"
            );
        }
        // Replay identities survive; unfinished metering work holds history.
        let removed = crate::events::retain(conn, cutoff).await?;
        if removed > 0 {
            tracing::info!(
                category = "retention",
                tenants = removed,
                "metering history retained as replay identities"
            );
        }
        sqlx::query("SELECT set_config('delcore.retention','off',true)")
            .execute(&mut *conn)
            .await?;
        Ok(())
    }
}
