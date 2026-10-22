// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
use chrono::{DateTime, Datelike, Months, TimeZone};
use sqlx::PgConnection;
impl Worker {
    pub(super) async fn activate_free(
        &self,
        tx: &mut PgConnection,
        tenant: &str,
        at: DateTime<Utc>,
    ) -> Result<()> {
        // Confirmed cancellation ends the subscription-scoped renewal failure in the same
        // transaction; debt, disputes and operator blocks are separate state and stay untouched.
        let start = Utc
            .with_ymd_and_hms(at.year(), at.month(), 1, 0, 0, 0)
            .single()
            .expect("valid UTC month");
        let end = start + Months::new(1);
        sqlx::query("UPDATE tenants SET plan_id=(SELECT id FROM plans WHERE is_default AND free),external_subscription_id='',renewal_failed=false,auto_suspended=true,period_start=$2,period_end=$3 WHERE id=$1").bind(uid(tenant)?).bind(start).bind(end).execute(&mut *tx).await?;
        let plan: String = sqlx::query_scalar("SELECT plan_id::text FROM tenants WHERE id=$1")
            .bind(uid(tenant)?)
            .fetch_one(&mut *tx)
            .await?;
        let version = postgres::catalog::active_catalog_version(tx, "plan", &plan).await?;
        postgres::catalog::replace_tenant_selections(tx, tenant, &[version], at).await?;
        sqlx::query("UPDATE entitlements SET ends_at=$2 WHERE tenant_id=$1 AND ends_at IS NULL AND starts_at<$2").bind(uid(tenant)?).bind(at).execute(&mut *tx).await?;
        for price in postgres::catalog::active_product_prices(tx, "plan", &plan).await? {
            sqlx::query(
                "INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2,$3,$4)",
            )
            .bind(uuid::Uuid::now_v7())
            .bind(uid(tenant)?)
            .bind(uid(&price)?)
            .bind(at)
            .execute(&mut *tx)
            .await?;
        }
        // Confirmed cancellation completes failed intents too, allowing final deletion.
        let completed=sqlx::query("UPDATE jobs SET state='done',error_category='',updated_at=now() WHERE tenant_id=$1 AND kind='cancel' AND state IN ('pending','running','reconcile','failed')").bind(uid(tenant)?).execute(&mut *tx).await?.rows_affected();
        tracing::info!(
            tenant_id = tenant,
            completed_jobs = completed,
            "confirmed cancellation completed cancel jobs"
        );
        postgres::grant_allowances(
            tx,
            tenant,
            start,
            end,
            at,
            &format!("free:{tenant}:{}", start.format("%Y-%m")),
            None,
        )
        .await?;
        Ok(())
    }
}
