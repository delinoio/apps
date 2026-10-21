// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{
    Server,
    operations::{audit, scrub},
};
use server_common::{Result, authority::TenantState, now};
impl Server {
    pub async fn maintain(&self) -> Result<()> {
        for (query, cutoff) in [
            (
                "DELETE FROM replays WHERE ctid IN (SELECT ctid FROM replays WHERE expires_at<=$1 LIMIT 10000)",
                now(),
            ),
            (
                "DELETE FROM audit_events WHERE ctid IN (SELECT ctid FROM audit_events WHERE occurred_at<$1 LIMIT 10000)",
                now() - chrono::Duration::days(90),
            ),
        ] {
            loop {
                if sqlx::query(query)
                    .bind(cutoff)
                    .execute(&self.db)
                    .await?
                    .rows_affected()
                    < 10000
                {
                    break;
                }
            }
        }
        let mut after = uuid::Uuid::nil();
        loop {
            let tenants:Vec<uuid::Uuid>=sqlx::query_scalar("SELECT DISTINCT tenant_id FROM projects WHERE tenant_id>$1 ORDER BY tenant_id LIMIT 100").bind(after).fetch_all(&self.db).await?;
            if tenants.is_empty() {
                break;
            }
            after = *tenants.last().unwrap();
            let ids: Vec<_> = tenants.iter().map(ToString::to_string).collect();
            let states = match self.authority.states(&ids).await {
                Ok(s) => s,
                Err(_) => {
                    tracing::warn!(category = "state_check", "tenant cleanup failed");
                    continue;
                }
            };
            for (tenant, state) in tenants.into_iter().zip(states) {
                if state != TenantState::Deleted {
                    continue;
                }
                let mut tx = self.db.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                    .bind(format!("projects:{tenant}"))
                    .execute(&mut *tx)
                    .await?;
                let projects: Vec<uuid::Uuid> =
                    sqlx::query_scalar("SELECT id FROM projects WHERE tenant_id=$1 FOR UPDATE")
                        .bind(tenant)
                        .fetch_all(&mut *tx)
                        .await?;
                let trace = server_common::id();
                for id in &projects {
                    audit(&mut tx, "system:cleanup", "tenant.cleanup", *id, &trace).await?
                }
                sqlx::query("DELETE FROM projects WHERE tenant_id=$1")
                    .bind(tenant)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("INSERT INTO deleted_tenants(tenant_id,deleted_at) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(tenant).bind(now()).execute(&mut *tx).await?;
                // Cascade first: an in-flight child write must finish before its replay is scrubbed.
                for id in projects {
                    scrub(&mut tx, "project_id", id).await?
                }
                tx.commit().await?;
            }
        }
        tracing::info!(category = "maintenance", "maintenance cycle completed");
        Ok(())
    }
}
