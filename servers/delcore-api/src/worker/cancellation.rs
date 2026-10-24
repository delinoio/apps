// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
use chrono::DateTime;
use sqlx::PgConnection;
impl Worker {
    pub(super) async fn activate_free(
        &self,
        tx: &mut PgConnection,
        tenant: &str,
        subscription: &str,
        at: DateTime<Utc>,
    ) -> Result<()> {
        // Confirmed cancellation ends the subscription-scoped renewal failure in the same
        // transaction; debt, disputes and operator blocks are separate state and stay untouched.
        sqlx::query("UPDATE tenants SET plan_id=(SELECT id FROM plans WHERE is_default AND free),external_subscription_id='',renewal_failed=false,auto_suspended=true WHERE id=$1").bind(uid(tenant)?).execute(&mut *tx).await?;
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
        // The free period is derived after the paid selection and entitlements ended, because a
        // retained paid period that starts at the calendar month start moves the free period
        // start to this cancellation. The tenant row, the allowance grant and every later
        // metering lookup use this one period so no allowance is keyed by the paid period.
        let period = postgres::period::free_period(tx, tenant, at).await?;
        sqlx::query("UPDATE tenants SET period_start=$2,period_end=$3 WHERE id=$1")
            .bind(uid(tenant)?)
            .bind(period.start)
            .bind(period.end)
            .execute(&mut *tx)
            .await?;
        // Confirmed cancellation completes failed intents too, allowing final deletion.
        let completed=sqlx::query("UPDATE jobs SET state='done',error_category='',updated_at=now() WHERE tenant_id=$1 AND kind='cancel' AND state IN ('pending','running','reconcile','failed')").bind(uid(tenant)?).execute(&mut *tx).await?.rows_affected();
        tracing::info!(
            tenant_id = tenant,
            completed_jobs = completed,
            "confirmed cancellation completed cancel jobs"
        );
        // The same transaction retires the scheduled paid downgrade or add-on removal of the
        // canceled subscription: a canceled subscription never renews, so the change can no longer
        // apply and must not hold later subscription changes or final deletion. Only attempted
        // `do_not_bill` jobs saved against this subscription are retired; the job keeps its saved
        // provider ID and history, other subscriptions' jobs and unresolved initial payments are
        // untouched, and the requested paid selections are never installed.
        let superseded=sqlx::query("UPDATE jobs SET state='done',error_category='superseded_by_cancellation',updated_at=now() WHERE tenant_id=$1 AND kind='subscription' AND payload->>'timing'='do_not_bill' AND state IN ('pending','running','reconcile','failed') AND external_id=$2").bind(uid(tenant)?).bind(subscription).execute(&mut *tx).await?.rows_affected();
        tracing::info!(
            tenant_id = tenant,
            subscription_id = subscription,
            superseded_jobs = superseded,
            "confirmed cancellation retired scheduled subscription changes"
        );
        postgres::grant_allowances(
            tx,
            tenant,
            period.start,
            period.end,
            at,
            &period.source_key,
            None,
        )
        .await?;
        Ok(())
    }
}
