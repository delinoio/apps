// SPDX-License-Identifier: FSL-1.1-ALv2
use super::Worker;
use crate::{
    accounting::{Decimal, USD, USD_MINOR_UNITS},
    postgres,
};
use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Months, TimeZone, Utc};
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

impl Worker {
    pub async fn maintain(&self, now: DateTime<Utc>) -> Result<()> {
        let mut tx = postgres::begin(&self.pool).await?;
        self.free_plan_changes(&mut tx, now).await?;
        let start = Utc
            .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
            .single()
            .expect("valid month");
        let end = start + Months::new(1);
        // Free plans use calendar months, including tenants created mid-month.
        // Only a period that actually advances here is a renewal. Tenants that
        // already hold the current period were initialized by creation,
        // confirmed free activation, or an earlier renewal, so re-deriving their
        // grants from today's mutable entitlements would double-grant a meter
        // that an explicit refresh added after the period began.
        let renewed: Vec<(String, bool)> = sqlx::query_as(&format!("UPDATE tenants t SET period_start=$1,period_end=$2 FROM plans p WHERE t.plan_id=p.id AND {} AND t.external_subscription_id='' AND t.period_end<=$3 RETURNING t.id::text,(t.deleted_at IS NULL)", postgres::selected_plan_bool_sql("t", "p", "free"))).bind(start).bind(end).bind(now).fetch_all(&mut *tx).await?;
        // The grants share this transaction, so a failed renewal leaves both the
        // period and its grants uncommitted and the next pass retries them once.
        for (tenant, active) in renewed {
            if !active {
                continue;
            }
            // A retained paid period that starts at the month start moves the free period start
            // to the cancellation instant. The tenant row and the grant follow the same period
            // that billing-period resolution returns, so the grant is never keyed by the paid
            // period.
            let period = postgres::period::free_period(&mut tx, &tenant, now).await?;
            if period.start != start {
                sqlx::query("UPDATE tenants SET period_start=$2,period_end=$3 WHERE id=$1::text::uuid AND period_start=$4 AND period_end=$5")
                    .bind(&tenant)
                    .bind(period.start)
                    .bind(period.end)
                    .bind(start)
                    .bind(end)
                    .execute(&mut *tx)
                    .await?;
            }
            postgres::grant_allowances(
                &mut tx,
                &tenant,
                period.start,
                period.end,
                period.start,
                &period.source_key,
                None,
            )
            .await?;
        }
        // Selection and queueing share the single-instance transaction lock, so
        // cancellation, disabling, and job claims cannot invalidate a selection.
        let rows = sqlx::query(r#"SELECT t.id,t.auto_amount::text FROM tenants t WHERE t.auto_amount IS NOT NULL AND t.external_subscription_id<>'' AND t.period_end>now() AND NOT t.auto_suspended AND t.deleting_at IS NULL AND t.deleted_at IS NULL AND NOT t.operator_block AND NOT t.renewal_failed
AND NOT EXISTS(SELECT 1 FROM disputes WHERE tenant_id=t.id AND active)
AND COALESCE((SELECT sum(amount) FROM ledger WHERE tenant_id=t.id AND kind IN ('credit','usage','refund','correction')),0)<t.auto_threshold
AND NOT EXISTS(SELECT 1 FROM jobs WHERE tenant_id=t.id AND kind='auto_topup' AND state IN ('pending','running','reconcile'))
AND COALESCE((SELECT sum((payload->>'amount')::numeric) FROM jobs WHERE tenant_id=t.id AND kind='auto_topup' AND created_at >= $1 AND (state<>'failed' OR error_category NOT IN ('provider_rejected','payment_failed','payment_method_required')) AND NOT (state='done' AND error_category IN ('auto_topup_disabled','subscription_canceled'))),0)+t.auto_amount<=t.auto_cap"#).bind(start).fetch_all(&mut *tx).await?;
        for row in rows {
            let tenant: Uuid = row.try_get(0)?;
            let amount = Decimal::parse(row.try_get::<&str, _>(1)?)?;
            let units = amount.minor_units(USD_MINOR_UNITS)?;
            sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1,$2,'auto_topup',$1::uuid::text,$3,$4,$4,$4)").bind(Uuid::now_v7()).bind(tenant).bind(json!({"amount":amount.to_string(),"minor_units":units.to_string(),"currency":USD})).bind(now).execute(&mut *tx).await?;
        }
        self.round_usage(&mut tx, now).await?;
        self.settle(&mut tx, now).await?;
        // Every independent debt, dispute, refund, and unfinished-job gate holds deletion.
        let rows = sqlx::query(r#"SELECT t.id,t.owner_id,t.personal FROM tenants t WHERE t.deleting_at<$1 AND t.deleted_at IS NULL
AND NOT EXISTS(SELECT 1 FROM disputes WHERE tenant_id=t.id AND active)
AND COALESCE((SELECT sum(amount) FROM ledger WHERE tenant_id=t.id AND kind IN ('credit','usage','refund','correction')),0)=0
AND COALESCE((SELECT sum(amount) FROM ledger WHERE tenant_id=t.id AND kind IN ('postpaid','settlement')),0)=0
AND NOT EXISTS(SELECT 1 FROM jobs WHERE tenant_id=t.id AND kind<>'email' AND state<>'done' AND NOT (kind IN ('topup','auto_topup') AND state='failed' AND error_category IN ('provider_rejected','payment_failed','payment_method_required')))
AND t.external_subscription_id=''"#).bind(now-Duration::seconds(self.config.reporting_grace_seconds+self.config.deletion_grace_seconds)).fetch_all(&mut *tx).await?;
        for row in rows {
            let tenant: Uuid = row.try_get(0)?;
            let user: Option<Uuid> = row.try_get(1)?;
            let personal: bool = row.try_get(2)?;
            for sql in [
                "DELETE FROM ownership_transfers WHERE tenant_id=$1",
                "UPDATE jobs SET payload='{}'::jsonb,state='done',error_category='recipient_deleted' WHERE tenant_id=$1 AND kind='email'",
                "UPDATE groups SET name=id::text WHERE tenant_id=$1",
                "UPDATE audit SET actor_id='deleted',reason='' WHERE tenant_id=$1",
                "UPDATE requests SET response=''::bytea WHERE tenant_id=$1",
                "UPDATE jobs SET payload='{}'::jsonb,checkout_url='' WHERE tenant_id=$1 AND state='done'",
                "DELETE FROM group_members WHERE tenant_id=$1",
                // Keys (and their grants) cannot outlive their tenant.
                "DELETE FROM api_keys WHERE tenant_id=$1",
                "DELETE FROM members WHERE tenant_id=$1",
            ] {
                sqlx::query(sql).bind(tenant).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE invitations SET email='',token_hash=id::text,revoked_at=COALESCE(revoked_at,$2) WHERE tenant_id=$1").bind(tenant).bind(now).execute(&mut *tx).await?;
            sqlx::query("UPDATE tenants SET name='Deleted tenant',deleted_at=$2,external_customer_id='',owner_id=NULL WHERE id=$1").bind(tenant).bind(now).execute(&mut *tx).await?;
            if personal {
                sqlx::query("UPDATE users SET email='',subject_hash='deleted:'||id::text WHERE id=$1 AND withdrawing_at IS NOT NULL").bind(user).execute(&mut *tx).await?;
            }
        }
        // Keep event IDs as replay tombstones while stripping retained raw payloads.
        sqlx::query("UPDATE webhooks SET payload='{}'::jsonb WHERE processed_at IS NOT NULL AND received_at<$1 AND payload<>'{}'::jsonb").bind(now-Duration::seconds(self.config.webhook_retention_seconds)).execute(&mut *tx).await?;
        sqlx::query("UPDATE jobs SET payload='{}'::jsonb WHERE kind='email' AND state='done' AND updated_at<$1").bind(now-Duration::seconds(self.config.invitation_seconds)).execute(&mut *tx).await?;
        self.notices(&mut tx, now).await?;
        self.retain(&mut tx, now).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn free_plan_changes(&self, conn: &mut PgConnection, now: DateTime<Utc>) -> Result<()> {
        #[derive(Deserialize)]
        struct Change {
            plan_id: String,
            #[serde(default)]
            price_ids: Vec<String>,
            #[serde(default)]
            version_ids: Vec<String>,
        }
        let rows = sqlx::query("SELECT j.id,j.tenant_id::text,j.payload,j.available_at FROM jobs j JOIN tenants t ON t.id=j.tenant_id WHERE j.kind='subscription' AND j.payload->>'timing'='free_next_period' AND j.state IN ('pending','reconcile') AND j.available_at<=$1 AND t.external_subscription_id='' AND t.deleted_at IS NULL").bind(now).fetch_all(&mut *conn).await?;
        for row in rows {
            let job: Uuid = row.try_get(0)?;
            let tenant: String = row.try_get(1)?;
            let change: Change = serde_json::from_value(row.try_get(2)?)?;
            let at: DateTime<Utc> = row.try_get(3)?;
            let tenant_id = Uuid::parse_str(&tenant)?;
            sqlx::query("UPDATE tenants SET plan_id=$2 WHERE id=$1")
                .bind(tenant_id)
                .bind(Uuid::parse_str(&change.plan_id)?)
                .execute(&mut *conn)
                .await?;
            postgres::replace_tenant_selections(conn, &tenant, &change.version_ids, at).await?;
            sqlx::query("UPDATE entitlements SET ends_at=$2 WHERE tenant_id=$1 AND ends_at IS NULL AND starts_at<$2").bind(tenant_id).bind(at).execute(&mut *conn).await?;
            for price in change.price_ids {
                sqlx::query(
                    "INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2,$3,$4)",
                )
                .bind(Uuid::now_v7())
                .bind(tenant_id)
                .bind(Uuid::parse_str(&price)?)
                .bind(at)
                .execute(&mut *conn)
                .await?;
            }
            sqlx::query("UPDATE jobs SET state='done',updated_at=$2 WHERE id=$1")
                .bind(job)
                .bind(now)
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}

/// Queue one message per current billing recipient. Keys deduplicate transitions
/// without placing recipient addresses in telemetry.
pub(super) async fn notify(
    conn: &mut PgConnection,
    tenant: &str,
    key: &str,
    subject: &str,
    text: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    let tenant_id = Uuid::parse_str(tenant)?;
    let rows = sqlx::query("SELECT u.id::text,u.email FROM members m JOIN users u ON u.id=m.user_id WHERE tenant_id=$1 AND (1=ANY(roles) OR 3=ANY(roles)) AND u.email<>''").bind(tenant_id).fetch_all(&mut *conn).await?;
    for row in rows {
        let user: String = row.try_get(0)?;
        let email: String = row.try_get(1)?;
        sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1,$2,'email',$3,$4,$5,$5,$5) ON CONFLICT(dedup_key) DO NOTHING").bind(Uuid::now_v7()).bind(tenant_id).bind(format!("{key}:{user}")).bind(json!({"to":email,"subject":subject,"text":text})).bind(now).execute(&mut *conn).await?;
    }
    Ok(())
}
