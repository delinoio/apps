// SPDX-License-Identifier: FSL-1.1-ALv2
//! Paid same-plan rank downgrades with an unchanged recurring price set.
use super::{Fixture, key};
use chrono::{DateTime, Duration, Utc};
use delcore_api::provider::Transaction;
use serde_json::{Value, json};
use uuid::Uuid;

const EXTERNAL_PRICE: &str = "pri_same";
const SUBSCRIPTION: &str = "sub_same";
const CUSTOMER: &str = "ctm_same";

struct Scenario {
    tenant: String,
    plan: String,
    price: String,
    /// Version selected by the tenant before the operator edits the plan.
    version_a: String,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
}

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

fn transaction(
    id: &str,
    origin: &str,
    status: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    prices: &[&str],
    custom: Value,
) -> Transaction {
    serde_json::from_value(json!({
        "id": id,
        "origin": origin,
        "status": status,
        "customer_id": CUSTOMER,
        "subscription_id": SUBSCRIPTION,
        "currency_code": "USD",
        "custom_data": custom,
        "items": prices.iter().map(|p| json!({"quantity":1,"price":{"id":p}})).collect::<Vec<_>>(),
        "billing_period": {"starts_at": start, "ends_at": end},
    }))
    .unwrap()
}

async fn put_plan(f: &Fixture, plan: &str, rank: i32, free: bool) -> String {
    let response = f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"rank fixture","plan":{"id":plan,"name":"Paid","rank":rank,"free":free}})).await;
    response["plan"]["activeVersionId"].as_str().unwrap().into()
}

async fn selected(f: &Fixture, tenant: &str, plan: &str) -> (String, DateTime<Utc>) {
    sqlx::query_as("SELECT version_id::text,starts_at FROM tenant_catalog_selections WHERE tenant_id=$1 AND product_id=$2 AND ends_at IS NULL")
        .bind(uuid(tenant)).bind(uuid(plan)).fetch_one(&f.db).await.unwrap()
}

async fn job_row(f: &Fixture, job: &str) -> (String, String, Value) {
    sqlx::query_as("SELECT state,error_category,payload FROM jobs WHERE id=$1")
        .bind(uuid(job))
        .fetch_one(&f.db)
        .await
        .unwrap()
}

async fn effects(f: &Fixture, tenant: &str) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM entitlements WHERE tenant_id=$1),(SELECT count(*) FROM entitlements WHERE tenant_id=$1 AND ends_at IS NULL),(SELECT count(*) FROM allowance_grants WHERE tenant_id=$1)")
        .bind(uuid(tenant)).fetch_one(&f.db).await.unwrap()
}

/// Establish the paid fixture: plan A (rank 10) with one recurring price, and a tenant that
/// holds a confirmed subscription period on version A.
async fn setup(f: &Fixture) -> Scenario {
    f.bootstrap().await;
    let service = f.service().await;
    let plan = f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"paid plan","plan":{"name":"Paid","rank":10,"free":false}})).await["plan"]["id"].as_str().unwrap().to_owned();
    let price = f.rpc("owner","CreatePrice",json!({"requestKey":key(),"reason":"recurring price","price":{"key":"base-usd","planId":plan,"serviceId":service,"meter":"base","unitPrice":"10","included":"0","currency":"USD","kind":"PRICE_KIND_RECURRING","externalPriceId":EXTERNAL_PRICE}})).await["price"]["id"].as_str().unwrap().to_owned();
    let tenant = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Downgrade fixture"}),
        )
        .await["tenant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let initial = f.rpc("owner","ChangeSubscription",json!({"requestKey":key(),"tenantId":tenant,"planId":plan,"priceIds":[price],"returnUrl":f.config.return_urls[0]})).await["jobId"].as_str().unwrap().to_owned();
    let period_start = Utc::now() - Duration::days(10);
    let period_end = period_start + Duration::days(30);
    let payment = transaction(
        "txn_initial",
        "web",
        "completed",
        period_start,
        period_end,
        &[EXTERNAL_PRICE],
        json!({"delcore_job_id":initial,"delcore_tenant_id":tenant}),
    );
    f.worker
        .apply_transaction(payment, Utc::now())
        .await
        .unwrap();
    assert_eq!(job_row(f, &initial).await.0, "done");
    let (version_a, _) = selected(f, &tenant, &plan).await;
    Scenario {
        tenant,
        plan,
        price,
        version_a,
        period_start,
        period_end,
    }
}

/// Request the explicit downgrade and return its job ID plus the retained requested version.
async fn request_downgrade(f: &Fixture, s: &Scenario) -> (String, String) {
    let version_b = put_plan(f, &s.plan, 5, false).await;
    assert_ne!(version_b, s.version_a);
    let response = f.rpc("owner","ChangeSubscription",json!({"requestKey":key(),"tenantId":s.tenant,"planId":s.plan,"priceIds":[s.price],"returnUrl":f.config.return_urls[0]})).await;
    (response["jobId"].as_str().unwrap().to_owned(), version_b)
}

fn renewal(s: &Scenario, id: &str, status: &str, prices: &[&str]) -> Transaction {
    transaction(
        id,
        "subscription_recurring",
        status,
        s.period_end,
        s.period_end + Duration::days(30),
        prices,
        json!({}),
    )
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn paid_rank_downgrade_with_unchanged_prices_waits_for_confirmed_renewal() {
    let f = Fixture::setup().await;
    let s = setup(&f).await;
    let (job, version_b) = request_downgrade(&f, &s).await;

    // The request is a paid scheduled change that keeps the provider price IDs.
    let (state, _, payload) = job_row(&f, &job).await;
    assert_eq!(state, "pending");
    assert_eq!(payload["timing"], "do_not_bill");
    assert_eq!(payload["external_price_ids"], json!([EXTERNAL_PRICE]));
    assert_eq!(payload["version_ids"], json!([version_b]));
    assert_eq!(payload["price_ids"], json!([s.price]));
    let available: DateTime<Utc> = sqlx::query_scalar("SELECT available_at FROM jobs WHERE id=$1")
        .bind(uuid(&job))
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert!(available < s.period_end, "paid change is dispatchable now");
    assert_eq!(selected(&f, &s.tenant, &s.plan).await.0, s.version_a);

    // The provider accepted the scheduled change; version A stays until payment.
    sqlx::query("UPDATE jobs SET state='reconcile' WHERE id=$1")
        .bind(uuid(&job))
        .execute(&f.db)
        .await
        .unwrap();
    f.worker
        .maintain(s.period_end + Duration::hours(1))
        .await
        .unwrap();
    assert_eq!(selected(&f, &s.tenant, &s.plan).await.0, s.version_a);

    // A later catalog version does not change the accepted request.
    let version_c = put_plan(&f, &s.plan, 3, false).await;
    assert_ne!(version_c, version_b);

    let before = effects(&f, &s.tenant).await;
    let confirmed = renewal(&s, "txn_renewal", "completed", &[EXTERNAL_PRICE]);
    f.worker
        .apply_transaction(confirmed.clone(), Utc::now())
        .await
        .unwrap();
    let (version, starts) = selected(&f, &s.tenant, &s.plan).await;
    assert_eq!(version, version_b);
    assert_eq!(starts, s.period_end);
    assert_eq!(job_row(&f, &job).await.0, "done");
    let (total, open, grants) = effects(&f, &s.tenant).await;
    assert_eq!((total, open), (before.0 + 1, 1));
    let period_start: DateTime<Utc> =
        sqlx::query_scalar("SELECT period_start FROM tenants WHERE id=$1")
            .bind(uuid(&s.tenant))
            .fetch_one(&f.db)
            .await
            .unwrap();
    assert_eq!(period_start, s.period_end);

    // Replaying the same notification creates no second selection, entitlement or allowance.
    f.worker
        .apply_transaction(confirmed, Utc::now())
        .await
        .unwrap();
    assert_eq!(effects(&f, &s.tenant).await, (total, open, grants));
    let selections: i64 = sqlx::query_scalar("SELECT count(*) FROM tenant_catalog_selections WHERE tenant_id=$1 AND product_id=$2 AND version_id=$3")
        .bind(uuid(&s.tenant)).bind(uuid(&s.plan)).bind(uuid(&version_b)).fetch_one(&f.db).await.unwrap();
    assert_eq!(selections, 1);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn rank_downgrade_is_not_activated_without_matching_confirmed_payment() {
    let f = Fixture::setup().await;
    let s = setup(&f).await;
    let (job, _) = request_downgrade(&f, &s).await;
    sqlx::query("UPDATE jobs SET state='reconcile' WHERE id=$1")
        .bind(uuid(&job))
        .execute(&f.db)
        .await
        .unwrap();

    // An unconfirmed transaction status and a missing payment leave the request unchanged.
    for status in ["ready", "billed", "past_due"] {
        let pending = renewal(&s, "txn_unconfirmed", status, &[EXTERNAL_PRICE]);
        f.worker
            .apply_transaction(pending, Utc::now())
            .await
            .unwrap();
        assert_eq!(selected(&f, &s.tenant, &s.plan).await.0, s.version_a);
        assert_eq!(job_row(&f, &job).await.0, "reconcile");
    }

    // A renewal for another recurring item fails the request and keeps version A.
    let other = renewal(&s, "txn_other", "completed", &["pri_other"]);
    f.worker.apply_transaction(other, Utc::now()).await.unwrap();
    assert_eq!(selected(&f, &s.tenant, &s.plan).await.0, s.version_a);
    let (state, category, _) = job_row(&f, &job).await;
    assert_eq!(
        (state.as_str(), category.as_str()),
        ("failed", "payment_mismatch")
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn rejected_rank_downgrade_does_not_install_the_requested_version() {
    let f = Fixture::setup().await;
    let s = setup(&f).await;
    let (job, _) = request_downgrade(&f, &s).await;
    // A definitive provider rejection fails the job.
    sqlx::query("UPDATE jobs SET state='failed',error_category='provider_rejected' WHERE id=$1")
        .bind(uuid(&job))
        .execute(&f.db)
        .await
        .unwrap();
    let confirmed = renewal(&s, "txn_renewal", "completed", &[EXTERNAL_PRICE]);
    f.worker
        .apply_transaction(confirmed, Utc::now())
        .await
        .unwrap();
    assert_eq!(selected(&f, &s.tenant, &s.plan).await.0, s.version_a);
    let (state, category, _) = job_row(&f, &job).await;
    assert_eq!(
        (state.as_str(), category.as_str()),
        ("failed", "provider_rejected")
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn ordinary_renewal_keeps_the_pinned_version_after_a_catalog_change() {
    let f = Fixture::setup().await;
    let s = setup(&f).await;
    put_plan(&f, &s.plan, 5, false).await;
    let confirmed = renewal(&s, "txn_renewal", "completed", &[EXTERNAL_PRICE]);
    f.worker
        .apply_transaction(confirmed, Utc::now())
        .await
        .unwrap();
    assert_eq!(selected(&f, &s.tenant, &s.plan).await.0, s.version_a);
    let period_start: DateTime<Utc> =
        sqlx::query_scalar("SELECT period_start FROM tenants WHERE id=$1")
            .bind(uuid(&s.tenant))
            .fetch_one(&f.db)
            .await
            .unwrap();
    assert_eq!(period_start, s.period_end);
    assert!(s.period_start < s.period_end);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn same_rank_and_upgrade_refreshes_remain_immediate_local_changes() {
    let f = Fixture::setup().await;
    let s = setup(&f).await;
    let change = |price: &str| json!({"requestKey":key(),"tenantId":s.tenant,"planId":s.plan,"priceIds":[price],"returnUrl":f.config.return_urls[0]});

    // Same rank: nothing changes in the catalog, so the refresh applies at once.
    let same = f.rpc("owner", "ChangeSubscription", change(&s.price)).await;
    let (state, _, payload) = job_row(&f, same["jobId"].as_str().unwrap()).await;
    assert_eq!(state, "done");
    assert!(payload.get("timing").is_none());
    assert_eq!(selected(&f, &s.tenant, &s.plan).await.0, s.version_a);

    // Upgrade: the higher-rank version is selected immediately without a provider job.
    let version_up = put_plan(&f, &s.plan, 20, false).await;
    let up = f.rpc("owner", "ChangeSubscription", change(&s.price)).await;
    let (state, _, payload) = job_row(&f, up["jobId"].as_str().unwrap()).await;
    assert_eq!(state, "done");
    assert!(payload.get("external_price_ids").is_none());
    assert_eq!(selected(&f, &s.tenant, &s.plan).await.0, version_up);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn unassociated_free_rank_downgrade_stays_a_free_deferred_change() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let service = f.service().await;
    f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"declare meter","meter":{"serviceId":service,"name":"requests","key":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT"}})).await;
    let plans = f.rpc("owner", "GetCatalog", json!({})).await;
    let plan = plans["plans"][0]["id"].as_str().unwrap().to_owned();
    f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"rank free plan","plan":{"id":plan,"name":"free","rank":5,"free":true,"defaultFree":true}})).await;
    let price = f.rpc("owner","CreatePrice",json!({"requestKey":key(),"reason":"usage price","price":{"key":"requests-usd","planId":plan,"serviceId":service,"meter":"requests","unitPrice":"1","included":"2","currency":"USD"}})).await["price"]["id"].as_str().unwrap().to_owned();
    let tenant = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Free downgrade"}),
        )
        .await["tenant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (version_a, _) = selected(&f, &tenant, &plan).await;
    let version_b = f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"lower free rank","plan":{"id":plan,"name":"free","rank":1,"free":true,"defaultFree":true}})).await["plan"]["activeVersionId"].as_str().unwrap().to_owned();
    assert_ne!(version_a, version_b);
    let job = f.rpc("owner","ChangeSubscription",json!({"requestKey":key(),"tenantId":tenant,"planId":plan,"priceIds":[price],"returnUrl":f.config.return_urls[0]})).await["jobId"].as_str().unwrap().to_owned();
    let (state, _, payload) = job_row(&f, &job).await;
    assert_eq!(state, "pending");
    assert_eq!(payload["timing"], "free_next_period");
    assert_eq!(payload["version_ids"], json!([version_b]));
    let end: DateTime<Utc> = sqlx::query_scalar("SELECT available_at FROM jobs WHERE id=$1")
        .bind(uuid(&job))
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(selected(&f, &tenant, &plan).await.0, version_a);
    f.worker.maintain(end - Duration::seconds(1)).await.unwrap();
    assert_eq!(selected(&f, &tenant, &plan).await.0, version_a);
    f.worker.maintain(end + Duration::seconds(1)).await.unwrap();
    assert_eq!(selected(&f, &tenant, &plan).await.0, version_b);
    assert_eq!(job_row(&f, &job).await.0, "done");
}
