// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
use serde::Serialize;

/// Key in `jobs.payload` that holds the progress of a paginated transaction scan.
///
/// The checkpoint is private to reconciliation: payment intent fields and `external_id` stay
/// untouched, and the defaulted payload decoders ignore the extra key.
const SCAN_KEY: &str = "reconcile_scan";
/// Category of a job that waits for the next page of an unfinished transaction scan. Like
/// `provider_billing_window`, it refunds the attempt, because reading a page is progress and
/// not a provider failure.
pub(super) const SCAN_PENDING: &str = "reconcile_scan_pending";

/// Durable progress of a transaction scan: the search context, the last cursor that was
/// examined completely and the cursors that earlier steps of the same scan requested.
#[derive(Serialize, Deserialize)]
struct Scan {
    context: String,
    after: String,
    /// Cursors requested before `after` in this scan. They let a later step detect a provider
    /// that cycles through several pages. Checkpoints written before this field existed decode
    /// with an empty list, which only delays the detection of a cycle that began before them.
    #[serde(default)]
    seen: Vec<String>,
}
/// How a job update treats the scan checkpoint in `jobs.payload`.
pub(super) enum ScanWrite {
    /// Leave the checkpoint as it is, so errors and timeouts keep completed pages.
    Keep,
    /// Remove the checkpoint because the scan ended, so the next scan starts fresh.
    Clear,
    Set(Value),
}
/// Returns the cursor to resume from. A checkpoint of another search context (for example a
/// tenant whose provider customer changed) is ignored and the scan starts at the first page.
fn resume_scan(j: &Job, context: &str) -> Option<Scan> {
    let scan: Scan = serde_json::from_value(j.payload.get(SCAN_KEY)?.clone()).ok()?;
    (scan.context == context && !scan.after.is_empty()).then_some(scan)
}
fn resume_cursor(j: &Job, context: &str) -> Option<String> {
    resume_scan(j, context).map(|scan| scan.after)
}
/// Returns the cursors a scan has requested once `next` is queued, or `None` when the provider
/// leads the scan back to a page that was requested already (for example `txn_a`, `txn_b`,
/// `txn_a`). The provider contract only rejects the cursor of the current request, so without this
/// history a cycle over several pages would requeue the job without end, because each scan step is
/// refunded.
fn advance_seen(resumed: Option<Scan>, next: &str) -> Option<Vec<String>> {
    let seen = resumed.map_or_else(Vec::new, |scan| {
        let mut seen = scan.seen;
        seen.push(scan.after);
        seen
    });
    if seen.iter().any(|cursor| cursor == next) {
        return None;
    }
    Some(seen)
}
impl Worker {
    /// Reads the next unexamined page of a transaction list.
    ///
    /// A worker step reads at most one page. Provider and decoding failures are handled here
    /// through the normal reconciliation retry policy and yield `None`.
    async fn scan_page(
        &self,
        j: &Job,
        path: &str,
        context: &str,
    ) -> Result<Option<(Vec<provider::Transaction>, Result<Option<String>, Error>)>> {
        let after = resume_cursor(j, context);
        let page = match self.provider.request_page(path, after.as_deref()).await {
            Ok(page) => page,
            Err(e) => {
                self.retry(j, e, true).await?;
                return Ok(None);
            }
        };
        match serde_json::from_value(page.data) {
            Ok(transactions) => Ok(Some((transactions, page.next))),
            Err(_) => {
                self.retry(j, Error::Ambiguous, true).await?;
                Ok(None)
            }
        }
    }
    /// Records the examined page and queues the next one immediately. The attempt is refunded
    /// by `finish`, so a long history never exhausts the retry budget.
    async fn defer_scan(&self, j: &Job, context: String, after: String) -> Result<()> {
        // A revisited cursor is an ambiguous provider response. Handling it as a reconciliation
        // error keeps the earlier progress and spends an attempt, so the retry limit ends the scan.
        let Some(seen) = advance_seen(resume_scan(j, &context), &after) else {
            tracing::warn!(kind = j.kind, "transaction scan revisits an earlier page");
            return self.retry(j, Error::Ambiguous, true).await;
        };
        tracing::info!(kind = j.kind, "transaction scan continues on the next page");
        let scan = serde_json::to_value(Scan {
            context,
            after,
            seen,
        })?;
        self.finish_scan(j, "reconcile", SCAN_PENDING, "", ScanWrite::Set(scan))
            .await
    }
}
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
            let context = format!("customer:{customer}");
            let Some((transactions, next)) = self.scan_page(&j, &path, &context).await? else {
                return Ok(());
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
                    .finish_scan(&j, "reconcile", "awaiting_payment", &t.id, ScanWrite::Clear)
                    .await;
            }
            match next {
                Ok(Some(after)) => return self.defer_scan(&j, context, after).await,
                Ok(None) => {}
                Err(e) => return self.retry(&j, e, true).await,
            }
            return self
                .finish_scan(
                    &j,
                    "failed",
                    "unknown_payment_outcome",
                    "",
                    ScanWrite::Clear,
                )
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
                    let event = provider::Event {
                        r#type: "subscription.canceled".into(),
                        occurred_at: Utc::now(),
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
                    let state: provider::Transaction = match serde_json::from_value(raw) {
                        Ok(v) => v,
                        Err(_) => return self.retry(&j, Error::Ambiguous, true).await,
                    };
                    if state.id != j.external_id {
                        return self.retry(&j, Error::Ambiguous, true).await;
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
            let context = format!("subscription:{}", j.external_id);
            let Some((transactions, next)) = self.scan_page(&j, &path, &context).await? else {
                return Ok(());
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
            match next {
                Ok(Some(after)) => return self.defer_scan(&j, context, after).await,
                Ok(None) => {}
                Err(e) => return self.retry(&j, e, true).await,
            }
            return self
                .finish_scan(
                    &j,
                    "reconcile",
                    "awaiting_payment",
                    &j.external_id,
                    ScanWrite::Clear,
                )
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

#[cfg(test)]
mod scan_history_tests {
    use super::*;

    fn scan(after: &str, seen: &[&str]) -> Scan {
        Scan {
            context: "customer:ctm_fixture".into(),
            after: after.into(),
            seen: seen.iter().map(|cursor| cursor.to_string()).collect(),
        }
    }
    #[test]
    fn a_cycle_over_several_pages_is_detected() {
        // First step: nothing was requested yet, so the first cursor is always new.
        let seen = advance_seen(None, "txn_a").unwrap();
        assert!(seen.is_empty());
        // Second step requested txn_a and moves on to txn_b.
        let seen = advance_seen(Some(scan("txn_a", &[])), "txn_b").unwrap();
        assert_eq!(seen, ["txn_a"]);
        // Third step requested txn_b and the provider points back to txn_a.
        assert_eq!(advance_seen(Some(scan("txn_b", &["txn_a"])), "txn_a"), None);
        // A longer cycle is detected as well, and the repeat of the current cursor.
        assert_eq!(
            advance_seen(Some(scan("txn_c", &["txn_a", "txn_b"])), "txn_a"),
            None
        );
        assert_eq!(advance_seen(Some(scan("txn_b", &["txn_a"])), "txn_b"), None);
    }
    #[test]
    fn distinct_cursors_and_older_checkpoints_continue() {
        let seen = advance_seen(Some(scan("txn_c", &["txn_a", "txn_b"])), "txn_d").unwrap();
        assert_eq!(seen, ["txn_a", "txn_b", "txn_c"]);
        // A checkpoint written before the history existed decodes with an empty list.
        let old: Scan =
            serde_json::from_value(json!({"context": "customer:ctm_fixture", "after": "txn_a1"}))
                .unwrap();
        assert!(old.seen.is_empty());
        assert_eq!(advance_seen(Some(old), "txn_a2").unwrap(), ["txn_a1"]);
    }
}
