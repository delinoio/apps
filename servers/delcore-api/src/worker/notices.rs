// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Worker, maintenance::notify};
use crate::postgres;
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

impl Worker {
    pub(super) async fn notices(&self, conn: &mut PgConnection, now: DateTime<Utc>) -> Result<()> {
        // Period expiry blocks paid use, but is not a confirmed payment failure.
        let rows = sqlx::query(&format!(r#"SELECT t.id::text,t.operator_block,t.renewal_failed,
NOT t.renewal_failed AND {},
EXISTS(SELECT 1 FROM disputes d WHERE d.tenant_id=t.id AND active),
COALESCE((SELECT sum(amount) FROM ledger WHERE tenant_id=t.id AND kind IN ('credit','usage','refund','correction')),0)<0,
(NOT {} AND COALESCE((SELECT sum(amount) FROM ledger WHERE tenant_id=t.id AND kind IN ('credit','usage','refund','correction')),0)=0),t.period_start,t.period_end
FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.deleted_at IS NULL"#,postgres::period_expired_sql("t","p","$1"), postgres::selected_plan_bool_sql("t","p","postpaid"))).bind(now).fetch_all(&mut *conn).await?;
        for row in rows {
            let tenant: String = row.try_get(0)?;
            let tenant_id = Uuid::parse_str(&tenant)?;
            let operator: bool = row.try_get(1)?;
            let renewal: bool = row.try_get(2)?;
            let expired: bool = row.try_get(3)?;
            let dispute: bool = row.try_get(4)?;
            let mut balance: bool = row.try_get(5)?;
            let depleted: bool = row.try_get(6)?;
            let start: DateTime<Utc> = row.try_get(7)?;
            let end: DateTime<Utc> = row.try_get(8)?;
            if depleted {
                let services: Vec<String> = sqlx::query_scalar("SELECT DISTINCT p.service_id::text FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=$1 AND en.starts_at<=$2 AND (en.ends_at IS NULL OR en.ends_at>$2)").bind(tenant_id).bind(now).fetch_all(&mut *conn).await?;
                for service in services {
                    if !postgres::has_allowance(conn, &tenant, &service, start).await? {
                        balance = true;
                        break;
                    }
                }
            }
            // Key expiry by the confirmed period, including repeated expirations
            // when maintenance never observed the intervening renewal.
            let expired_reason = format!("period_expired:{}", end.format("%Y%m%dT%H%M%S%.6fZ"));
            for (family, reason, text, active) in [
                (
                    "operator",
                    "operator",
                    "An operator has blocked new paid use.",
                    operator,
                ),
                (
                    "renewal",
                    "renewal",
                    "A renewal payment failed. Complete payment recovery before starting new paid use.",
                    renewal,
                ),
                (
                    "period_expired",
                    expired_reason.as_str(),
                    "Your paid period ended and no renewal has been confirmed yet. New paid use resumes after the renewal payment is confirmed.",
                    expired,
                ),
                (
                    "dispute",
                    "dispute",
                    "A payment dispute is active. New paid use is blocked.",
                    dispute,
                ),
                (
                    "balance",
                    "balance",
                    "Your credit balance is insufficient. Add funds to recover new paid use.",
                    balance,
                ),
            ] {
                self.supersede_notices(
                    conn,
                    &tenant,
                    family,
                    if active { reason } else { "" },
                    now,
                )
                .await?;
                if !active {
                    continue;
                }
                let generation: Option<i32> = sqlx::query_scalar("INSERT INTO billing_notices VALUES($1,$2,true,1) ON CONFLICT(tenant_id,reason) DO UPDATE SET active=true,generation=billing_notices.generation+1 WHERE NOT billing_notices.active RETURNING generation").bind(tenant_id).bind(reason).fetch_optional(&mut *conn).await?;
                if let Some(generation) = generation {
                    notify(
                        conn,
                        &tenant,
                        &format!("notice:{tenant}:{reason}:{generation}"),
                        "Paid use blocked",
                        text,
                        now,
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    /// Deactivate obsolete notices and their pending emails. Running and sent
    /// messages are left alone because delivery may already have started.
    async fn supersede_notices(
        &self,
        conn: &mut PgConnection,
        tenant: &str,
        family: &str,
        keep: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let tenant_id = Uuid::parse_str(tenant)?;
        let reasons: Vec<String> = sqlx::query_scalar("UPDATE billing_notices SET active=false WHERE tenant_id=$1 AND active AND (reason=$2 OR starts_with(reason,$3)) AND reason<>$4 RETURNING reason").bind(tenant_id).bind(family).bind(format!("{family}:")).bind(keep).fetch_all(&mut *conn).await?;
        for reason in reasons {
            let result = sqlx::query("UPDATE jobs SET state='done',error_category='notice_superseded',updated_at=$3 WHERE tenant_id=$1 AND kind='email' AND state='pending' AND starts_with(dedup_key,$2)").bind(tenant_id).bind(format!("notice:{tenant}:{reason}:")).bind(now).execute(&mut *conn).await?;
            tracing::info!(
                tenant_id = tenant,
                family,
                canceled_emails = result.rows_affected(),
                "billing notice superseded"
            );
        }
        Ok(())
    }
}
