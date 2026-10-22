// SPDX-License-Identifier: FSL-1.1-ALv2
//! Reusable fixtures for provider webhook and subscription lifecycle tests.
use super::{Fixture, key};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

/// A tenant on the default free plan, plus a paid plan and a provider subscription ID.
///
/// The free plan carries one metered price with included usage, so a tenant that returns to
/// the free plan has an active usage entitlement and remaining included usage.
pub struct Subscriber {
    pub tenant: String,
    pub user: String,
    pub paid_plan: String,
    pub subscription: String,
    /// Provider time of the initial payment; later occurrences are offset from it.
    pub paid_at: DateTime<Utc>,
}

impl Subscriber {
    /// Return a provider occurrence time `millis` after the initial payment.
    ///
    /// Offsets stay in the past of processing time because the fixture waits after capturing
    /// `paid_at`; keep them below that wait.
    pub fn after(&self, millis: i64) -> DateTime<Utc> {
        self.paid_at + Duration::milliseconds(millis)
    }
}

pub async fn subscriber(f: &Fixture) -> Subscriber {
    f.bootstrap().await;
    let service = f.service().await;
    let catalog = f.rpc("owner", "GetCatalog", json!({})).await;
    let free = catalog["plans"][0]["id"].as_str().unwrap().to_owned();
    f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"declare meter","meter":{"serviceId":service,"name":"requests","key":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT"}})).await;
    f.rpc("owner","CreatePrice",json!({"requestKey":key(),"reason":"free usage","price":{"key":"requests-usd","planId":free,"serviceId":service,"meter":"requests","unitPrice":"1.00","included":"5","currency":"USD"}})).await;
    let paid = f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"paid plan","plan":{"name":"paid","rank":2,"free":false}})).await;
    let tenant = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Subscriber"}),
        )
        .await["tenant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let user = f.rpc("owner", "GetCurrentUser", json!({})).await["userId"]
        .as_str()
        .unwrap()
        .to_owned();
    let paid_at = Utc::now();
    // Occurrences are offset from `paid_at` and must precede the processing time.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    Subscriber {
        tenant,
        user,
        paid_plan: paid["plan"]["id"].as_str().unwrap().to_owned(),
        subscription: format!("sub_{}", key()),
        paid_at,
    }
}

/// Store a provider notification as the webhook endpoint would, then process it.
pub async fn deliver(f: &Fixture, event_type: &str, at: DateTime<Utc>, data: Value) {
    let id = format!("evt_{}", key());
    let payload = json!({"event_id":id,"event_type":event_type,"occurred_at":at,"data":data});
    sqlx::query(
        "INSERT INTO webhooks(event_id,occurred_at,payload,received_at) VALUES($1,$2,$3,now())",
    )
    .bind(&id)
    .bind(at)
    .bind(&payload)
    .execute(&f.db)
    .await
    .unwrap();
    f.worker.process_webhook(&id).await.unwrap();
}

pub async fn subscription_status(f: &Fixture, s: &Subscriber, status: &str, at: DateTime<Utc>) {
    let kind = if status == "past_due" {
        "subscription.past_due"
    } else {
        "subscription.updated"
    };
    deliver(f, kind, at, json!({"id":s.subscription,"status":status})).await;
}

/// Confirm the initial subscription payment for the tenant at `s.paid_at`.
pub async fn initial_payment(f: &Fixture, s: &Subscriber) {
    let job = Uuid::now_v7();
    let payload = json!({"plan_id":s.paid_plan,"price_ids":[],"version_ids":[],"external_price_ids":["pri_paid"]});
    sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,state,available_at,created_at,updated_at) VALUES($1,$2::text::uuid,'subscription',$1::text,$3,'running',now(),now(),now())")
        .bind(job).bind(&s.tenant).bind(payload).execute(&f.db).await.unwrap();
    let transaction = json!({"id":format!("txn_{}",key()),"status":"completed","origin":"web",
        "customer_id":"ctm_fixture","subscription_id":s.subscription,"currency_code":"USD",
        "custom_data":{"delcore_job_id":job.to_string(),"delcore_tenant_id":s.tenant},
        "items":[{"price":{"id":"pri_paid"},"quantity":1}],
        "billing_period":{"starts_at":s.paid_at,"ends_at":s.paid_at+Duration::days(30)},
        "billed_at":s.paid_at});
    deliver(f, "transaction.completed", s.paid_at, transaction).await;
}

/// Read plan, provider subscription association and renewal failure flag.
pub async fn tenant_state(f: &Fixture, s: &Subscriber) -> (String, String, bool) {
    sqlx::query_as("SELECT plan_id::text,external_subscription_id,renewal_failed FROM tenants WHERE id=$1::text::uuid")
        .bind(&s.tenant).fetch_one(&f.db).await.unwrap()
}

pub async fn default_free_plan(f: &Fixture) -> String {
    sqlx::query_scalar("SELECT id::text FROM plans WHERE is_default AND free")
        .fetch_one(&f.db)
        .await
        .unwrap()
}

pub async fn authorize_paid(f: &Fixture, s: &Subscriber) -> Value {
    f.rpc(
        "service",
        "Authorize",
        json!({"tenantId":s.tenant,"userId":s.user,"capability":"execute","paid":true}),
    )
    .await
}

pub async fn tenant_blocked(f: &Fixture, s: &Subscriber) -> bool {
    f.rpc("owner", "GetTenant", json!({"tenantId":s.tenant}))
        .await["tenant"]["blocked"]
        == true
}
