// SPDX-License-Identifier: FSL-1.1-ALv2
use super::Worker;
use crate::{postgres, provider::Transaction};
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use std::collections::HashMap;

pub(super) fn matches_prices(t: &Transaction, expected: &[String]) -> bool {
    if expected.is_empty() || t.items.len() != expected.len() {
        return false;
    }
    let mut counts = HashMap::new();
    for price in expected {
        *counts.entry(price.as_str()).or_insert(0usize) += 1;
    }
    for item in &t.items {
        let Some(count) = counts.get_mut(item.price.id.as_str()) else {
            return false;
        };
        if item.quantity != 1 || *count == 0 {
            return false;
        }
        *count -= 1;
    }
    true
}

impl Worker {
    pub(super) async fn renew(
        &self,
        conn: &mut PgConnection,
        t: &Transaction,
        at: DateTime<Utc>,
    ) -> Result<()> {
        let Some(period) = &t.billing_period else {
            return Ok(());
        };
        if t.subscription_id.is_empty() || period.ends_at <= period.starts_at || t.currency != "USD"
        {
            return Ok(());
        }
        let Some(row) = sqlx::query("SELECT id::text,external_customer_id,period_end FROM tenants WHERE external_subscription_id=$1 AND deleted_at IS NULL")
            .bind(&t.subscription_id).fetch_optional(&mut *conn).await? else { return Ok(()); };
        let tenant: String = row.try_get(0)?;
        let customer: String = row.try_get(1)?;
        let previous_end: DateTime<Utc> = row.try_get(2)?;
        if customer != t.customer_id || period.starts_at < previous_end {
            return Ok(());
        }
        // A scheduled change takes effect only at a renewal whose billing period starts at or after
        // the request (`created_at`). A delayed notification for a period that began earlier
        // already contained the old subscription, so it must not consume the change. It renews
        // the current entitlements below and leaves the change for the next renewal.
        let change = sqlx::query("SELECT id::text,payload FROM jobs WHERE tenant_id=$1::text::uuid AND kind='subscription' AND state IN ('reconcile','running') AND payload->>'timing'='do_not_bill' AND created_at<=$2 ORDER BY created_at LIMIT 1")
            .bind(&tenant).bind(period.starts_at).fetch_optional(&mut *conn).await?;
        if let Some(change) = change {
            let change_id: String = change.try_get(0)?;
            let p: super::webhook::PaymentPayload = serde_json::from_value(change.try_get(1)?)?;
            if !matches_prices(t, &p.external_price_ids) {
                sqlx::query("UPDATE jobs SET state='failed',error_category='payment_mismatch' WHERE id=$1::text::uuid").bind(&change_id).execute(&mut *conn).await?;
                return Ok(());
            }
            sqlx::query("UPDATE entitlements SET ends_at=$2 WHERE tenant_id=$1::text::uuid AND ends_at IS NULL AND starts_at<$2").bind(&tenant).bind(period.starts_at).execute(&mut *conn).await?;
            for price in &p.price_ids {
                sqlx::query("INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2::text::uuid,$3::text::uuid,$4)").bind(uuid::Uuid::now_v7()).bind(&tenant).bind(price).bind(period.starts_at).execute(&mut *conn).await?;
            }
            sqlx::query("UPDATE tenants SET plan_id=$2::text::uuid WHERE id=$1::text::uuid")
                .bind(&tenant)
                .bind(&p.plan_id)
                .execute(&mut *conn)
                .await?;
            postgres::catalog::replace_tenant_selections(
                conn,
                &tenant,
                &p.version_ids,
                period.starts_at,
            )
            .await?;
            sqlx::query("UPDATE jobs SET state='done',external_id=$2,error_category='',updated_at=now() WHERE id=$1::text::uuid").bind(&change_id).bind(&t.id).execute(&mut *conn).await?;
        } else {
            let expected: Vec<String> = sqlx::query_scalar("SELECT p.external_price_id FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=$1::text::uuid AND en.ends_at IS NULL AND p.kind=2 ORDER BY p.id").bind(&tenant).fetch_all(&mut *conn).await?;
            if !matches_prices(t, &expected) {
                return Ok(());
            }
        }
        sqlx::query("INSERT INTO provider_transactions VALUES($1,$2::text::uuid,'subscription',0,$3,$4) ON CONFLICT DO NOTHING").bind(&t.id).bind(&tenant).bind(&t.currency).bind(at).execute(&mut *conn).await?;
        sqlx::query("INSERT INTO provider_objects VALUES($1,$2,'active') ON CONFLICT(external_id) DO UPDATE SET occurred_at=EXCLUDED.occurred_at,state='active' WHERE provider_objects.occurred_at<EXCLUDED.occurred_at").bind(&t.subscription_id).bind(at).execute(&mut *conn).await?;
        super::webhook::record_subscription_payment(conn, &t.subscription_id, at).await?;
        // The provider watermark above ignores a payment older than the latest status, so the
        // tenant failure flag needs the same ordering. A failure that occurred after this payment
        // stays until a newer confirmed renewal recovers it; an older failure is recovered here.
        // The failure is kept in its own row because an informational status can replace the
        // subscription row's state after the failure.
        let failure_key = format!("failure:{}", t.subscription_id);
        let failed_at: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT occurred_at FROM provider_objects WHERE external_id=$1")
                .bind(&failure_key)
                .fetch_optional(&mut *conn)
                .await?;
        let failure_is_newer = failed_at.is_some_and(|failed| failed > at);
        sqlx::query("UPDATE tenants SET period_start=$2,period_end=$3,renewal_failed=renewal_failed AND $4 WHERE id=$1::text::uuid").bind(&tenant).bind(period.starts_at).bind(period.ends_at).bind(failure_is_newer).execute(&mut *conn).await?;
        if failure_is_newer {
            tracing::warn!(
                tenant_id = %tenant,
                subscription_id = %t.subscription_id,
                transaction_id = %t.id,
                "kept newer subscription payment failure after an older renewal payment"
            );
        } else {
            sqlx::query("DELETE FROM provider_objects WHERE external_id=$1")
                .bind(&failure_key)
                .execute(&mut *conn)
                .await?;
        }
        postgres::allowances::grant_allowances(
            conn,
            &tenant,
            period.starts_at,
            period.ends_at,
            period.starts_at,
            &format!("renewal:{}", t.id),
            None,
        )
        .await?;
        // A free grant that already sits at the renewed period start must not add its
        // allowance to the paid period; see `separate_free_grants`.
        postgres::period::separate_free_grants(conn, &tenant, period.starts_at).await?;
        sqlx::query(&format!("INSERT INTO billing_periods SELECT t.id,$2,$3,{} FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1::text::uuid ON CONFLICT DO NOTHING",postgres::period::selected_plan_bool_sql("t","p","postpaid"))).bind(&tenant).bind(period.starts_at).bind(period.ends_at).execute(&mut *conn).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_price_multisets_and_rejects_extra_quantities() {
        let transaction: Transaction = serde_json::from_value(serde_json::json!({
            "items": [
                {"quantity":1,"price":{"id":"pri_a"}},
                {"quantity":1,"price":{"id":"pri_a"}},
                {"quantity":1,"price":{"id":"pri_b"}}
            ]
        }))
        .unwrap();
        assert!(matches_prices(
            &transaction,
            &["pri_b".into(), "pri_a".into(), "pri_a".into()]
        ));
        assert!(!matches_prices(
            &transaction,
            &["pri_b".into(), "pri_b".into(), "pri_a".into()]
        ));
        assert!(!matches_prices(&transaction, &[]));
        let mut bundled = transaction.clone();
        bundled.items[0].quantity = 2;
        assert!(!matches_prices(
            &bundled,
            &["pri_b".into(), "pri_a".into(), "pri_a".into()]
        ));
    }
}
