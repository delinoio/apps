// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Fixture, key};
use chrono::{DateTime, Utc};
use delcore_api::worker::Worker;
use serde_json::json;

struct Setup {
    tenant: String,
    plan: String,
    service: String,
}

async fn meter(f: &Fixture, service: &str, name: &str) {
    f.rpc("owner", "PutMeter", json!({"requestKey":key(),"reason":"declare meter","meter":{"serviceId":service,"key":name,"name":name,"eventType":name,"aggregation":"METER_AGGREGATION_COUNT"}})).await;
}

async fn price(f: &Fixture, s: &Setup, meter: &str, unit: &str, included: &str) -> String {
    let response = f.rpc("owner", "CreatePrice", json!({"requestKey":key(),"reason":"publish price","price":{"key":format!("{meter}-usd"),"planId":s.plan,"serviceId":s.service,"meter":meter,"unitPrice":unit,"included":included,"currency":"USD"}})).await;
    response["price"]["id"].as_str().unwrap().to_owned()
}

/// Creates a default free plan with usage price `a` (included 100).
async fn setup(f: &Fixture) -> (Setup, String) {
    f.bootstrap().await;
    let service = f.service().await;
    let catalog = f.rpc("owner", "GetCatalog", json!({})).await;
    let plan = catalog["plans"][0]["id"].as_str().unwrap().to_owned();
    meter(f, &service, "a").await;
    meter(f, &service, "b").await;
    let mut s = Setup {
        tenant: String::new(),
        plan,
        service,
    };
    let a = price(f, &s, "a", "1", "100").await;
    create_tenant(f, &mut s, "First").await;
    (s, a)
}

/// Creates a tenant that selects the price versions active right now.
async fn create_tenant(f: &Fixture, s: &mut Setup, name: &str) {
    let created = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":name}),
        )
        .await;
    s.tenant = created["tenant"]["id"].as_str().unwrap().to_owned();
}

async fn refresh(f: &Fixture, s: &Setup, prices: &[&str]) {
    f.rpc(
        "owner",
        "ChangeSubscription",
        json!({"requestKey":key(),"tenantId":s.tenant,"planId":s.plan,"priceIds":prices,"returnUrl":"https://consumer.example.invalid/delcore/return"}),
    )
    .await;
}

/// Returns the number of grants and whether their total equals `expected` for
/// one meter whose source key starts with `prefix`.
async fn grants(f: &Fixture, s: &Setup, meter: &str, prefix: &str, expected: &str) -> (i64, bool) {
    sqlx::query_as("SELECT count(*),COALESCE(sum(amount),0)=$5::numeric FROM allowance_grants WHERE tenant_id=$1::text::uuid AND service_id=$2::text::uuid AND meter=$3 AND source_key LIKE $4")
        .bind(&s.tenant).bind(&s.service).bind(meter).bind(format!("{prefix}%")).bind(expected).fetch_one(&f.db).await.unwrap()
}

async fn grant_total(f: &Fixture, s: &Setup, meter: &str) -> String {
    sqlx::query_scalar("SELECT COALESCE(sum(amount),0)::text FROM allowance_grants WHERE tenant_id=$1::text::uuid AND service_id=$2::text::uuid AND meter=$3")
        .bind(&s.tenant).bind(&s.service).bind(meter).fetch_one(&f.db).await.unwrap()
}

async fn ledger(f: &Fixture, s: &Setup) -> (i64, String) {
    sqlx::query_as(
        "SELECT count(*),COALESCE(sum(amount),0)::text FROM ledger WHERE tenant_id=$1::text::uuid",
    )
    .bind(&s.tenant)
    .fetch_one(&f.db)
    .await
    .unwrap()
}

async fn period(f: &Fixture, s: &Setup) -> (DateTime<Utc>, DateTime<Utc>) {
    sqlx::query_as("SELECT period_start,period_end FROM tenants WHERE id=$1::text::uuid")
        .bind(&s.tenant)
        .fetch_one(&f.db)
        .await
        .unwrap()
}

fn restarted(f: &Fixture) -> Worker {
    Worker::new(f.db.clone(), f.config.clone()).unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn maintenance_keeps_only_the_refresh_difference_for_a_meter_added_midmonth() {
    let f = Fixture::setup().await;
    let (s, a) = setup(&f).await;
    assert_eq!(grants(&f, &s, "a", "free:", "100").await, (1, true));
    assert_eq!(grants(&f, &s, "b", "", "0").await, (0, true));
    let b = price(&f, &s, "b", "1", "300").await;
    // Catalog publication alone never changes the tenant's selected prices.
    f.worker.maintain(Utc::now()).await.unwrap();
    assert_eq!(grants(&f, &s, "b", "", "0").await, (0, true));

    refresh(&f, &s, &[&a, &b]).await;
    let refreshed = grant_total(&f, &s, "b").await;
    assert_eq!(
        grants(&f, &s, "b", "free_refresh:", &refreshed).await,
        (1, true)
    );
    assert!(refreshed.parse::<f64>().unwrap() > 0.0 && refreshed.parse::<f64>().unwrap() < 300.0);
    let before = ledger(&f, &s).await;

    // Repeated passes and a fresh worker must not mint the monthly key for the
    // meter that the refresh added after the period was initialized.
    f.worker.maintain(Utc::now()).await.unwrap();
    f.worker.maintain(Utc::now()).await.unwrap();
    restarted(&f).maintain(Utc::now()).await.unwrap();
    assert_eq!(grants(&f, &s, "b", "free:", "0").await, (0, true));
    assert_eq!(grant_total(&f, &s, "b").await, refreshed);
    assert_eq!(grants(&f, &s, "a", "free:", "100").await, (1, true));
    assert_eq!(ledger(&f, &s).await, before);
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn existing_meter_refresh_keeps_its_difference_and_next_month_grants_once() {
    let f = Fixture::setup().await;
    let (s, a) = setup(&f).await;
    price(&f, &s, "b", "1", "100").await;
    let mut s = s;
    create_tenant(&f, &mut s, "Second").await;
    assert_eq!(grants(&f, &s, "b", "free:", "100").await, (1, true));
    let raised = price(&f, &s, "b", "1", "300").await;
    refresh(&f, &s, &[&a, &raised]).await;
    let total = grant_total(&f, &s, "b").await;
    assert_eq!(grants(&f, &s, "b", "free_refresh:", "0").await.0, 1);
    f.worker.maintain(Utc::now()).await.unwrap();
    restarted(&f).maintain(Utc::now()).await.unwrap();
    assert_eq!(grant_total(&f, &s, "b").await, total);
    assert_eq!(grants(&f, &s, "b", "free:", "100").await, (1, true));

    // The next UTC calendar month grants each selected meter's full quantity once.
    let (start, end) = period(&f, &s).await;
    f.worker.maintain(end).await.unwrap();
    restarted(&f).maintain(end).await.unwrap();
    let (next_start, next_end) = period(&f, &s).await;
    assert_eq!(next_start, end);
    assert!(next_end > next_start);
    let month = end.format("%Y-%m").to_string();
    assert_eq!(
        grants(&f, &s, "a", &format!("free:{}:{month}", s.tenant), "100").await,
        (1, true)
    );
    assert_eq!(
        grants(&f, &s, "b", &format!("free:{}:{month}", s.tenant), "300").await,
        (1, true)
    );
    // The earlier period keeps its original grants.
    let earlier: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM allowance_grants WHERE tenant_id=$1::text::uuid AND period_start=$2",
    )
    .bind(&s.tenant)
    .bind(start)
    .fetch_one(&f.db)
    .await
    .unwrap();
    assert_eq!(earlier, 3);
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn failed_renewal_rolls_back_period_and_grants_then_retries_once() {
    let f = Fixture::setup().await;
    let (s, _) = setup(&f).await;
    let (start, end) = period(&f, &s).await;
    let month = end.format("%Y-%m").to_string();
    // Reject every grant of the next period so the period update must roll back with them.
    sqlx::query(&format!("ALTER TABLE allowance_grants ADD CONSTRAINT reject_renewal CHECK (period_start<>'{}'::timestamptz) NOT VALID", end.to_rfc3339()))
        .execute(&f.db)
        .await
        .unwrap();
    assert!(f.worker.maintain(end).await.is_err());
    assert_eq!(period(&f, &s).await, (start, end));
    assert_eq!(
        grants(&f, &s, "a", &format!("free:{}:{month}", s.tenant), "100")
            .await
            .0,
        0
    );
    sqlx::query("ALTER TABLE allowance_grants DROP CONSTRAINT reject_renewal")
        .execute(&f.db)
        .await
        .unwrap();
    f.worker.maintain(end).await.unwrap();
    f.worker.maintain(end).await.unwrap();
    assert_eq!(period(&f, &s).await.0, end);
    assert_eq!(
        grants(&f, &s, "a", &format!("free:{}:{month}", s.tenant), "100").await,
        (1, true)
    );
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn midmonth_creation_keeps_its_initial_grant_through_maintenance() {
    let f = Fixture::setup().await;
    let (s, _) = setup(&f).await;
    f.worker.maintain(Utc::now()).await.unwrap();
    restarted(&f).maintain(Utc::now()).await.unwrap();
    assert_eq!(grants(&f, &s, "a", "free:", "100").await, (1, true));
}
