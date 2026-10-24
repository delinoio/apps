// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
impl Worker {
    pub(super) async fn reconcile(&self, mut j: Job) -> Result<()> {
        if j.external_id.is_empty() && matches!(j.kind.as_str(), "cancel" | "subscription") {
            j.external_id =
                sqlx::query_scalar("SELECT external_subscription_id FROM tenants WHERE id=$1")
                    .bind(uid(&j.tenant)?)
                    .fetch_one(&self.pool)
                    .await?;
        }
        if j.external_id.is_empty() {
            let customer: String =
                sqlx::query_scalar("SELECT external_customer_id FROM tenants WHERE id=$1")
                    .bind(uid(&j.tenant)?)
                    .fetch_one(&self.pool)
                    .await?;
            let mut path = "/transactions?per_page=200&order_by=created_at[DESC]".to_string();
            if !customer.is_empty() {
                path.push_str(&format!(
                    "&customer_id={}",
                    provider::escape_path(&customer)
                ))
            }
            let raw = match self.provider.request(Method::GET, &path, Value::Null).await {
                Ok(v) => v,
                Err(e) => return self.retry(&j, e, true).await,
            };
            let transactions: Vec<provider::Transaction> = match serde_json::from_value(raw) {
                Ok(v) => v,
                Err(_) => return self.retry(&j, Error::Ambiguous, true).await,
            };
            for t in transactions {
                let t = t.attribute_charge(&self.config.paddle_credit_product_id);
                if t.custom.job_id != j.id || t.custom.tenant_id != j.tenant {
                    continue;
                }
                if t.status == "completed" {
                    return self.apply_transaction(t, Utc::now()).await;
                }
                return self
                    .finish(&j, "reconcile", "awaiting_payment", &t.id)
                    .await;
            }
            return self
                .finish(&j, "failed", "unknown_payment_outcome", "")
                .await;
        }
        if j.external_id.starts_with("sub_") {
            if j.kind == "cancel" {
                let raw = match self
                    .provider
                    .request(
                        Method::GET,
                        &format!("/subscriptions/{}", provider::escape_path(&j.external_id)),
                        Value::Null,
                    )
                    .await
                {
                    Ok(v) => v,
                    Err(e) => return self.retry(&j, e, true).await,
                };
                if !raw.is_object() {
                    return self.retry(&j, Error::Ambiguous, true).await;
                }
                if raw["status"] == "canceled" {
                    // The cancellation boundary is the provider's own `canceled_at`. The
                    // observation time is not a substitute: it would move the paid/free
                    // transition and the subscription watermark past the real cancellation and
                    // make the genuine notification stale. Paddle sets `canceled_at` only when
                    // the cancellation takes effect (a scheduled cancellation keeps the
                    // subscription active with `canceled_at` null), so a canceled subscription
                    // without a usable value is unexpected and is retried as ambiguous without
                    // any projection.
                    let Some(canceled_at) = provider::canceled_at(&raw, Utc::now()) else {
                        tracing::warn!(
                            kind = j.kind,
                            "canceled subscription has no valid cancellation time; retrying"
                        );
                        return self.retry(&j, Error::Ambiguous, true).await;
                    };
                    let event = provider::Event {
                        r#type: "subscription.canceled".into(),
                        occurred_at: canceled_at,
                        data: raw,
                        ..Default::default()
                    };
                    let mut tx = postgres::begin(&self.pool).await?;
                    self.subscription_event(&mut tx, &event).await?;
                    tx.commit().await?;
                    return self.finish(&j, "done", "", &j.external_id).await;
                }
                // The `request_started` marker is written before the provider request, so an
                // interruption in the short window between the two also lands here although
                // nothing was sent. An accepted cancellation always shows as a scheduled
                // cancellation on the still-active subscription, so without one the request
                // never took effect: return the job to pending to rerun the final check and send
                // it. The attempt stays counted so a provider that never reports the schedule
                // cannot make this loop look like a fresh job.
                if raw["scheduled_change"]["action"] != "cancel" {
                    tracing::warn!(
                        kind = j.kind,
                        "cancellation is not scheduled at the provider; sending it again"
                    );
                    return self
                        .finish(&j, "pending", "cancellation_not_scheduled", "")
                        .await;
                }
                return self
                    .finish(&j, "reconcile", "awaiting_renewal", &j.external_id)
                    .await;
            }
            if j.kind == "subscription" {
                let p: Payload = match serde_json::from_value(j.payload.clone()) {
                    Ok(v) => v,
                    Err(_) => {
                        return self
                            .finish(&j, "failed", "invalid_payload", &j.external_id)
                            .await;
                    }
                };
                if p.timing == "do_not_bill" {
                    let raw = match self
                        .provider
                        .request(
                            Method::GET,
                            &format!("/subscriptions/{}", provider::escape_path(&j.external_id)),
                            Value::Null,
                        )
                        .await
                    {
                        Ok(v) => v,
                        Err(e) => return self.retry(&j, e, true).await,
                    };
                    let state: provider::Transaction = match serde_json::from_value(raw.clone()) {
                        Ok(v) => v,
                        Err(_) => return self.retry(&j, Error::Ambiguous, true).await,
                    };
                    if state.id != j.external_id {
                        return self.retry(&j, Error::Ambiguous, true).await;
                    }
                    if state.status == "canceled" {
                        // A canceled subscription can never renew, so the scheduled change can no
                        // longer apply, whether or not the saved items still match it. Project the
                        // cancellation through the same ordered path as the notification, at the
                        // provider's own `canceled_at` (see the cancel branch above). The
                        // projection is keyed by the saved subscription ID: it only changes a
                        // tenant that still carries this subscription, and it is a no-op when the
                        // notification was applied first or the association was already cleared.
                        // The job is then retired as superseded without installing the requested
                        // paid selections. This also covers retrying a job that failed with
                        // `subscription_mismatch` after cancellation won.
                        let Some(canceled_at) = provider::canceled_at(&raw, Utc::now()) else {
                            tracing::warn!(
                                kind = j.kind,
                                "canceled subscription has no valid cancellation time; retrying"
                            );
                            return self.retry(&j, Error::Ambiguous, true).await;
                        };
                        let event = provider::Event {
                            r#type: "subscription.canceled".into(),
                            occurred_at: canceled_at,
                            data: raw,
                            ..Default::default()
                        };
                        let mut tx = postgres::begin(&self.pool).await?;
                        self.subscription_event(&mut tx, &event).await?;
                        tx.commit().await?;
                        tracing::info!(
                            job_id = j.id,
                            tenant_id = j.tenant,
                            subscription_id = j.external_id,
                            "scheduled subscription change superseded by confirmed cancellation"
                        );
                        return self
                            .finish(&j, "done", "superseded_by_cancellation", &j.external_id)
                            .await;
                    }
                    if !webhook::matches_prices(&state, &p.external_price_ids) {
                        return self
                            .finish(&j, "failed", "subscription_mismatch", &j.external_id)
                            .await;
                    }
                    return self
                        .finish(&j, "reconcile", "awaiting_renewal", &j.external_id)
                        .await;
                }
            }
            let path = format!(
                "/transactions?subscription_id={}&per_page=200&order_by=created_at[DESC]",
                provider::escape_path(&j.external_id)
            );
            let raw = match self.provider.request(Method::GET, &path, Value::Null).await {
                Ok(v) => v,
                Err(e) => return self.retry(&j, e, true).await,
            };
            let transactions: Vec<provider::Transaction> = match serde_json::from_value(raw) {
                Ok(v) => v,
                Err(_) => return self.retry(&j, Error::Ambiguous, true).await,
            };
            for t in transactions {
                let t = t.attribute_charge(&self.config.paddle_credit_product_id);
                let matches = t.custom.job_id == j.id
                    || t.items.iter().any(|item| {
                        item.price.custom.job_id == j.id
                            && item.price.product_id == self.config.paddle_credit_product_id
                    });
                if matches && t.status == "completed" {
                    return self.apply_transaction(t, Utc::now()).await;
                }
            }
            return self
                .finish(&j, "reconcile", "awaiting_payment", &j.external_id)
                .await;
        }
        let raw = match self
            .provider
            .request(
                Method::GET,
                &format!("/transactions/{}", provider::escape_path(&j.external_id)),
                Value::Null,
            )
            .await
        {
            Ok(v) => v,
            Err(e) => return self.retry(&j, e, true).await,
        };
        let t: provider::Transaction = match serde_json::from_value(raw) {
            Ok(v) => v,
            Err(_) => return self.retry(&j, Error::Ambiguous, true).await,
        };
        if t.status == "completed" {
            return self.apply_transaction(t, Utc::now()).await;
        }
        if matches!(t.status.as_str(), "canceled" | "past_due") {
            return self
                .finish(&j, "failed", "payment_failed", &j.external_id)
                .await;
        }
        self.finish(&j, "reconcile", "awaiting_payment", &j.external_id)
            .await
    }
}
