// SPDX-License-Identifier: FSL-1.1-ALv2
//! Recovery of the payment link of an unpaid transaction during reconciliation.
use super::Fixture;
use super::reconcile_pagination::{
    Provider, assert_read_only_and_authenticated, case, credits, gets, job_state, list, payment,
    ready, unrelated,
};
use serde_json::{Value, json};

const SAFE: &str = "https://checkout.example.test/pay?_ptxn=txn_fixture";

/// A matching unpaid transaction as the provider returns it.
fn unpaid(c: &super::reconcile_pagination::Case, status: &str, checkout: Value) -> Value {
    let mut row = payment(&c.txn(), &c.tenant, &c.job, status);
    row["checkout"] = checkout;
    row
}
async fn stored_url(f: &Fixture, job: uuid::Uuid) -> String {
    sqlx::query_scalar("SELECT checkout_url FROM jobs WHERE id=$1")
        .bind(job)
        .fetch_one(&f.db)
        .await
        .unwrap()
}
async fn external_id(f: &Fixture, job: uuid::Uuid) -> String {
    sqlx::query_scalar("SELECT external_id FROM jobs WHERE id=$1")
        .bind(job)
        .fetch_one(&f.db)
        .await
        .unwrap()
}
/// The link a tenant billing reader sees for the job.
async fn history_url(f: &Fixture, tenant: uuid::Uuid, job: uuid::Uuid) -> String {
    let history = f
        .rpc(
            "owner",
            "ListBillingHistory",
            json!({"tenantId": tenant.to_string()}),
        )
        .await;
    let listed = history["jobs"].as_array().unwrap();
    let item = listed
        .iter()
        .find(|j| j["id"] == job.to_string())
        .expect("job is listed");
    item["checkoutUrl"].as_str().unwrap_or("").to_string()
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn unknown_id_recovery_saves_the_link_and_known_id_reads_keep_it() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    for status in ["draft", "ready"] {
        let provider = Provider::start().await;
        let worker = provider.worker(&f);
        let c = case(&f, "ctm_fixture", "").await;
        // Another job's transaction must not supply this job's link.
        let mut other = unrelated("txn_other", &c.tenant);
        other["status"] = json!("draft");
        other["checkout"] = json!({"url": "https://checkout.example.test/other"});
        let rows = vec![other, unpaid(&c, status, json!({"url": SAFE}))];
        provider.page("", list(rows, &c.filter, None));
        worker.step().await.unwrap();
        let (state, category, _, _) = job_state(&f, c.job).await;
        assert_eq!(
            (state.as_str(), category.as_str()),
            ("reconcile", "awaiting_payment"),
            "{status}"
        );
        assert_eq!(external_id(&f, c.job).await, c.txn());
        assert_eq!(stored_url(&f, c.job).await, SAFE, "{status}");
        assert_eq!(history_url(&f, c.tenant, c.job).await, SAFE, "{status}");
        assert_eq!(credits(&f, c.tenant).await, 0);

        // The known transaction is read directly from now on. Repeated reads are idempotent, and
        // an absent or empty link never clears the saved one.
        for body in [
            unpaid(&c, "ready", json!({"url": SAFE})),
            unpaid(&c, "ready", json!({"url": ""})),
            unpaid(&c, "ready", Value::Null),
            unpaid(&c, "ready", json!({})),
        ] {
            let again = Provider::start().await;
            again.page("", json!({"data": body}));
            ready(&f, c.job).await;
            again.worker(&f).step().await.unwrap();
            assert_eq!(
                again.requests().len(),
                1,
                "one read of {}",
                again.requests()[0]
            );
            let (state, category, _, _) = job_state(&f, c.job).await;
            assert_eq!(
                (state.as_str(), category.as_str()),
                ("reconcile", "awaiting_payment")
            );
            assert_eq!(external_id(&f, c.job).await, c.txn());
            assert_eq!(stored_url(&f, c.job).await, SAFE);
            assert_read_only_and_authenticated(&again);
        }
        assert_eq!(history_url(&f, c.tenant, c.job).await, SAFE);
        assert_eq!(credits(&f, c.tenant).await, 0);
        assert_read_only_and_authenticated(&provider);
        assert_eq!(gets(&provider), 1);
    }
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn known_id_recovery_restores_the_link_independently() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let provider = Provider::start().await;
    let c = case(&f, "ctm_fixture", "").await;
    sqlx::query("UPDATE jobs SET external_id=$2 WHERE id=$1")
        .bind(c.job)
        .bind(c.txn())
        .execute(&f.db)
        .await
        .unwrap();
    assert_eq!(stored_url(&f, c.job).await, "");
    provider.page(
        "",
        json!({"data": unpaid(&c, "ready", json!({"url": SAFE}))}),
    );
    provider.worker(&f).step().await.unwrap();
    assert_eq!(stored_url(&f, c.job).await, SAFE);
    assert_eq!(history_url(&f, c.tenant, c.job).await, SAFE);
    assert_eq!(external_id(&f, c.job).await, c.txn());
    assert_eq!(job_state(&f, c.job).await.0, "reconcile");
    assert_eq!(credits(&f, c.tenant).await, 0);
    assert_read_only_and_authenticated(&provider);
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn unsafe_links_are_never_saved_and_follow_the_ambiguity_policy() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let unsafe_urls = [
        "not a url",
        "http://checkout.example.test/pay",
        "https://user@checkout.example.test/pay",
        "https://user:secret@checkout.example.test/pay",
    ];
    for known in [false, true] {
        for url in unsafe_urls {
            let provider = Provider::start().await;
            let c = case(&f, "ctm_fixture", "").await;
            let row = unpaid(&c, "ready", json!({"url": url}));
            if known {
                sqlx::query("UPDATE jobs SET external_id=$2 WHERE id=$1")
                    .bind(c.job)
                    .bind(c.txn())
                    .execute(&f.db)
                    .await
                    .unwrap();
                provider.page("", json!({"data": row}));
            } else {
                provider.page("", list(vec![row], &c.filter, None));
            }
            provider.worker(&f).step().await.unwrap();
            let (state, category, attempts, _) = job_state(&f, c.job).await;
            assert_eq!(
                (state.as_str(), category.as_str(), attempts),
                ("reconcile", "payment_ambiguous", 1),
                "known={known} {url}"
            );
            assert_eq!(stored_url(&f, c.job).await, "", "known={known} {url}");
            assert_eq!(history_url(&f, c.tenant, c.job).await, "");
            assert_eq!(credits(&f, c.tenant).await, 0);
            assert_read_only_and_authenticated(&provider);

            // An unsafe link does not replace a link that was saved earlier either.
            sqlx::query(
                "UPDATE jobs SET checkout_url=$2,external_id='',available_at=now() WHERE id=$1",
            )
            .bind(c.job)
            .bind(SAFE)
            .execute(&f.db)
            .await
            .unwrap();
            if known {
                sqlx::query("UPDATE jobs SET external_id=$2 WHERE id=$1")
                    .bind(c.job)
                    .bind(c.txn())
                    .execute(&f.db)
                    .await
                    .unwrap();
            }
            provider.worker(&f).step().await.unwrap();
            assert_eq!(stored_url(&f, c.job).await, SAFE, "known={known} {url}");
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn completed_and_failed_transactions_keep_their_behavior() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    // A completed payment credits once and does not save the link.
    let provider = Provider::start().await;
    let c = case(&f, "ctm_fixture", "").await;
    let row = unpaid(&c, "completed", json!({"url": SAFE}));
    provider.page("", list(vec![row], &c.filter, None));
    provider.worker(&f).step().await.unwrap();
    assert_eq!(job_state(&f, c.job).await.0, "done");
    assert_eq!(credits(&f, c.tenant).await, 1);
    assert_eq!(stored_url(&f, c.job).await, "");
    // A canceled transaction still fails a known-ID job and does not save a link.
    for status in ["canceled", "past_due"] {
        let provider = Provider::start().await;
        let c = case(&f, "ctm_fixture", "").await;
        sqlx::query("UPDATE jobs SET external_id=$2 WHERE id=$1")
            .bind(c.job)
            .bind(c.txn())
            .execute(&f.db)
            .await
            .unwrap();
        provider.page(
            "",
            json!({"data": unpaid(&c, status, json!({"url": SAFE}))}),
        );
        provider.worker(&f).step().await.unwrap();
        let (state, category, _, _) = job_state(&f, c.job).await;
        assert_eq!(
            (state.as_str(), category.as_str()),
            ("failed", "payment_failed")
        );
        assert_eq!(stored_url(&f, c.job).await, "");
        assert_eq!(credits(&f, c.tenant).await, 0);
    }
}
