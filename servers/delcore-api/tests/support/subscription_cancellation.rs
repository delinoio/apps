// SPDX-License-Identifier: FSL-1.1-ALv2
//! Confirmed subscription cancellation must end the subscription-scoped renewal failure,
//! whatever the delivery order of the initial payment and the status notifications.
use super::{Fixture, key};
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

const SUBSCRIPTION: &str = "sub_cancel_fixture";
const CUSTOMER: &str = "ctm_cancel_fixture";
const EXTERNAL_PRICE: &str = "pri_cancel_fixture";

/// Delivery order of the initial payment relative to the status notifications.
#[derive(Clone, Copy, Debug)]
enum Order {
    PaymentFirst,
    PaymentLast,
}

/// Provider occurrence times, anchored to the current UTC month so the free period is current.
struct Timeline {
    paid: DateTime<Utc>,
    period_end: DateTime<Utc>,
    month: DateTime<Utc>,
}
impl Timeline {
    fn now() -> Self {
        let today = Utc::now();
        let month = Utc
            .with_ymd_and_hms(today.year(), today.month(), 1, 0, 0, 0)
            .single()
            .unwrap();
        Self {
            paid: month - Duration::days(30),
            period_end: month,
            month,
        }
    }
    fn past_due(&self) -> DateTime<Utc> {
        self.month - Duration::minutes(2)
    }
    fn canceled(&self) -> DateTime<Utc> {
        self.month
    }
}

struct Scenario {
    f: Fixture,
    tenant: String,
    user: Value,
    job: String,
    plan: String,
    version: String,
    price: String,
    time: Timeline,
}

async fn scenario() -> Scenario {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let service = f.service().await;
    f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"declare meter","meter":{"serviceId":service,"name":"requests","key":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT"}})).await;
    let catalog = f.rpc("owner", "GetCatalog", json!({})).await;
    let free = catalog["plans"][0]["id"].as_str().unwrap().to_owned();
    f.rpc("owner","CreatePrice",json!({"requestKey":key(),"reason":"free usage allowance","price":{"key":"requests-free","planId":free,"serviceId":service,"meter":"requests","unitPrice":"1","included":"100","currency":"USD"}})).await;
    let paid = f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"paid plan","plan":{"key":"pro","name":"pro","rank":10,"free":false}})).await;
    let plan = paid["plan"]["id"].as_str().unwrap().to_owned();
    let price = f.rpc("owner","CreatePrice",json!({"requestKey":key(),"reason":"paid usage","price":{"key":"requests-pro","planId":plan,"serviceId":service,"meter":"requests","unitPrice":"1","included":"1000","currency":"USD"}})).await["price"]["id"].as_str().unwrap().to_owned();
    let version: String = sqlx::query_scalar(
        "SELECT active_version_id::text FROM catalog_entities WHERE id=$1::text::uuid",
    )
    .bind(&plan)
    .fetch_one(&f.db)
    .await
    .unwrap();
    let user = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let tenant = user["tenants"][0]["id"].as_str().unwrap().to_owned();
    let job = key();
    let payload = json!({"plan_id":plan,"price_ids":[price],"version_ids":[version],"external_price_ids":[EXTERNAL_PRICE]});
    sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,state,attempts,available_at,created_at,updated_at) VALUES($1,$2,'subscription',$3,$4,'pending',0,now(),now(),now())")
        .bind(Uuid::parse_str(&job).unwrap()).bind(Uuid::parse_str(&tenant).unwrap()).bind(format!("fixture:{job}")).bind(payload).execute(&f.db).await.unwrap();
    Scenario {
        f,
        tenant,
        user,
        job,
        plan,
        version,
        price,
        time: Timeline::now(),
    }
}

impl Scenario {
    async fn deliver(&self, id: &str, kind: &str, occurred: DateTime<Utc>, data: Value) {
        let payload = json!({"event_id":id,"event_type":kind,"occurred_at":occurred.to_rfc3339(),"data":data});
        sqlx::query(
            "INSERT INTO webhooks(event_id,occurred_at,payload,received_at) VALUES($1,$2,$3,now())",
        )
        .bind(id)
        .bind(occurred)
        .bind(payload)
        .execute(&self.f.db)
        .await
        .unwrap();
        self.f.worker.process_webhook(id).await.unwrap();
    }
    async fn payment(&self, id: &str) {
        let data = json!({"id":"txn_cancel_fixture","status":"completed","origin":"web","customer_id":CUSTOMER,"subscription_id":SUBSCRIPTION,"currency_code":"USD",
            "custom_data":{"delcore_job_id":self.job,"delcore_tenant_id":self.tenant},
            "items":[{"price":{"id":EXTERNAL_PRICE,"product_id":"pro_cancel_fixture"},"quantity":1}],
            "billing_period":{"starts_at":self.time.paid.to_rfc3339(),"ends_at":self.time.period_end.to_rfc3339()},
            "billed_at":self.time.paid.to_rfc3339()});
        self.deliver(id, "transaction.completed", self.time.paid, data)
            .await;
    }
    async fn status(&self, id: &str, status: &str, occurred: DateTime<Utc>) {
        self.deliver(
            id,
            "subscription.updated",
            occurred,
            json!({"id":SUBSCRIPTION,"status":status}),
        )
        .await;
    }
    /// Delivers the same provider occurrences in the requested order.
    async fn run(&self, order: Order) {
        if matches!(order, Order::PaymentFirst) {
            self.payment("evt_payment").await;
        }
        self.status("evt_past_due", "past_due", self.time.past_due())
            .await;
        self.status("evt_canceled", "canceled", self.time.canceled())
            .await;
        if matches!(order, Order::PaymentLast) {
            self.payment("evt_payment").await;
        }
    }
    async fn flag(&self) -> bool {
        sqlx::query_scalar("SELECT renewal_failed FROM tenants WHERE id=$1::text::uuid")
            .bind(&self.tenant)
            .fetch_one(&self.f.db)
            .await
            .unwrap()
    }
    /// Returns the order-independent cancellation state of the tenant.
    async fn state(&self) -> Value {
        let row: (String, String, bool, bool, DateTime<Utc>, DateTime<Utc>, bool) = sqlx::query_as("SELECT t.plan_id::text,t.external_subscription_id,t.renewal_failed,t.auto_suspended,t.period_start,t.period_end,p.free FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1::text::uuid")
            .bind(&self.tenant).fetch_one(&self.f.db).await.unwrap();
        let selected: String = sqlx::query_scalar("SELECT version_id::text FROM tenant_catalog_selections WHERE tenant_id=$1::text::uuid AND product_id=$2::text::uuid AND ends_at IS NULL")
            .bind(&self.tenant).bind(&row.0).fetch_one(&self.f.db).await.unwrap();
        let entitlements: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM entitlements WHERE tenant_id=$1::text::uuid AND ends_at IS NULL",
        )
        .bind(&self.tenant)
        .fetch_one(&self.f.db)
        .await
        .unwrap();
        let paid_entitlements: i64 = sqlx::query_scalar("SELECT count(*) FROM entitlements WHERE tenant_id=$1::text::uuid AND ends_at IS NULL AND price_id=$2::text::uuid")
            .bind(&self.tenant).bind(&self.price).fetch_one(&self.f.db).await.unwrap();
        let grants: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM allowance_grants WHERE tenant_id=$1::text::uuid",
        )
        .bind(&self.tenant)
        .fetch_one(&self.f.db)
        .await
        .unwrap();
        json!({"free":row.6,"subscription":row.1,"renewalFailed":row.2,"autoSuspended":row.3,
            "start":row.4.to_rfc3339(),"end":row.5.to_rfc3339(),"selectedVersionIsPaid":selected==self.version,
            "entitlements":entitlements,"paidEntitlements":paid_entitlements,"grants":grants,
            "defaultFreePlan":row.0!=self.plan})
    }
    async fn authorize(&self, paid: bool) -> Value {
        self.f.rpc("service","Authorize",json!({"tenantId":self.tenant,"userId":self.user["userId"],"capability":"execute","paid":paid})).await
    }
    async fn blocked(&self) -> bool {
        self.f
            .rpc("owner", "GetTenant", json!({"tenantId":self.tenant}))
            .await["tenant"]["blocked"]
            .as_bool()
            .unwrap_or(false)
    }
    async fn tenant_state(&self) -> String {
        let r = self
            .f
            .rpc(
                "service",
                "GetTenantStates",
                json!({"tenantIds":[self.tenant]}),
            )
            .await;
        r["states"][0]["state"].as_str().unwrap().to_owned()
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn confirmed_cancellation_clears_renewal_failure_in_either_delivery_order() {
    let mut finals = Vec::new();
    for order in [Order::PaymentFirst, Order::PaymentLast] {
        let s = scenario().await;
        s.run(order).await;
        let state = s.state().await;
        assert_eq!(state["defaultFreePlan"], true, "{order:?}: {state}");
        assert_eq!(state["free"], true, "{order:?}: {state}");
        assert_eq!(state["subscription"], "", "{order:?}: {state}");
        assert_eq!(state["renewalFailed"], false, "{order:?}: {state}");
        assert_eq!(state["selectedVersionIsPaid"], false, "{order:?}: {state}");
        assert_eq!(state["paidEntitlements"], 0, "{order:?}: {state}");
        assert_eq!(state["entitlements"], 1, "{order:?}: {state}");
        assert_eq!(
            state["start"],
            s.time.month.to_rfc3339(),
            "{order:?}: {state}"
        );
        let paid = s.authorize(true).await;
        assert_eq!(paid["allowed"], true, "{order:?}: {paid}");
        assert!(
            paid["reasons"].as_array().is_none_or(Vec::is_empty),
            "{order:?}: {paid}"
        );
        assert_eq!(s.authorize(false).await["allowed"], true);
        assert!(!s.blocked().await, "{order:?}");
        assert_eq!(s.tenant_state().await, "TENANT_STATE_ACTIVE");
        finals.push(state);
    }
    assert_eq!(
        finals[0], finals[1],
        "final state must not depend on delivery order"
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn independent_restrictions_survive_confirmed_cancellation() {
    for (restriction, reason) in [
        ("debt", "BLOCK_REASON_BALANCE"),
        ("dispute", "BLOCK_REASON_DISPUTE"),
        ("operator", "BLOCK_REASON_OPERATOR"),
        ("deletion", "BLOCK_REASON_DELETING"),
    ] {
        let s = scenario().await;
        // Seed the restriction after the failure is recorded and before the cancellation is
        // delivered, so the cancellation transaction runs against a tenant that already has it.
        s.payment("evt_payment").await;
        s.status("evt_past_due", "past_due", s.time.past_due())
            .await;
        assert!(
            s.flag().await,
            "{restriction}: failure precedes cancellation"
        );
        let tenant = Uuid::parse_str(&s.tenant).unwrap();
        match restriction {
            "debt" => {
                sqlx::query("INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2,'correction',-5,$3,now())")
                    .bind(Uuid::now_v7()).bind(tenant).bind(format!("fixture:{}", key())).execute(&s.f.db).await.unwrap();
            }
            "dispute" => {
                sqlx::query(
                    "INSERT INTO disputes(external_id,tenant_id,active) VALUES($1,$2,true)",
                )
                .bind(key())
                .bind(tenant)
                .execute(&s.f.db)
                .await
                .unwrap();
            }
            "operator" => {
                sqlx::query("UPDATE tenants SET operator_block=true WHERE id=$1")
                    .bind(tenant)
                    .execute(&s.f.db)
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::query("UPDATE tenants SET deleting_at=now() WHERE id=$1")
                    .bind(tenant)
                    .execute(&s.f.db)
                    .await
                    .unwrap();
            }
        }
        s.status("evt_canceled", "canceled", s.time.canceled())
            .await;
        let result = s.authorize(true).await;
        // The protocol JSON omits `allowed` when it is false.
        assert!(
            !result["allowed"].as_bool().unwrap_or(false),
            "{restriction}: {result}"
        );
        let reasons = result["reasons"].as_array().unwrap();
        assert_eq!(reasons, &vec![json!(reason)], "{restriction}: {result}");
        assert!(!s.flag().await, "{restriction}");
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn duplicate_stale_and_active_notifications_do_not_change_failure_semantics() {
    let s = scenario().await;
    // Before cancellation, a newer active status must not clear a genuine failure.
    s.payment("evt_payment").await;
    s.status("evt_past_due", "past_due", s.time.past_due())
        .await;
    s.status(
        "evt_active",
        "active",
        s.time.past_due() + Duration::seconds(30),
    )
    .await;
    assert!(s.flag().await, "active status alone must not clear failure");
    assert_eq!(
        s.authorize(true).await["reasons"],
        json!(["BLOCK_REASON_RENEWAL"])
    );
    assert!(s.blocked().await);
    s.status("evt_canceled", "canceled", s.time.canceled())
        .await;
    let after = s.state().await;
    assert_eq!(after["renewalFailed"], false);
    // Replayed cancellation and an older past-due notification are stale and harmless.
    s.status("evt_canceled_replay", "canceled", s.time.canceled())
        .await;
    s.status(
        "evt_stale_past_due",
        "past_due",
        s.time.past_due() - Duration::minutes(1),
    )
    .await;
    assert_eq!(s.state().await, after);
    assert_eq!(s.authorize(true).await["allowed"], true);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn failed_free_activation_rolls_back_every_change() {
    let s = scenario().await;
    s.payment("evt_payment").await;
    s.status("evt_past_due", "past_due", s.time.past_due())
        .await;
    let before = s.state().await;
    assert_eq!(before["renewalFailed"], true);
    // A conflicting allowance grant key makes the activation fail after earlier steps ran.
    sqlx::query("CREATE OR REPLACE FUNCTION fixture_fail_grant() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'fixture grant failure'; END $$ LANGUAGE plpgsql")
        .execute(&s.f.db).await.unwrap();
    sqlx::query("CREATE TRIGGER fixture_fail_grant BEFORE INSERT ON allowance_grants FOR EACH ROW EXECUTE FUNCTION fixture_fail_grant()")
        .execute(&s.f.db).await.unwrap();
    let canceled = json!({"event_id":"evt_canceled","event_type":"subscription.canceled","occurred_at":s.time.canceled().to_rfc3339(),"data":{"id":SUBSCRIPTION,"status":"canceled"}});
    sqlx::query("INSERT INTO webhooks(event_id,occurred_at,payload,received_at) VALUES('evt_canceled',$1,$2,now())")
        .bind(s.time.canceled()).bind(canceled).execute(&s.f.db).await.unwrap();
    assert!(s.f.worker.process_webhook("evt_canceled").await.is_err());
    assert_eq!(
        s.state().await,
        before,
        "failed activation must leave no partial state"
    );
    let processed: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT processed_at FROM webhooks WHERE event_id='evt_canceled'")
            .fetch_one(&s.f.db)
            .await
            .unwrap();
    assert!(processed.is_none());
}

/// A confirmed cancellation supersedes a scheduled paid downgrade or add-on removal, because a
/// canceled subscription never renews.
mod superseded_change {
    use super::*;
    use delcore_api::{config::Config, worker::Worker};
    use std::sync::{Arc, Mutex};

    const OTHER_SUBSCRIPTION: &str = "sub_other_fixture";
    const REQUESTED_PRICE: &str = "pri_requested_fixture";

    /// A worker whose provider answers every GET with the body currently stored in `response`.
    async fn provider(
        s: &Scenario,
        response: Arc<Mutex<Value>>,
    ) -> (Worker, tokio::task::JoinHandle<()>) {
        let app = axum::Router::new().fallback(move || {
            let body = response.lock().unwrap().clone();
            async move { axum::Json(json!({ "data": body })) }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let worker = Worker::new(
            s.f.db.clone(),
            Config {
                paddle_url: url.clone(),
                email_url: url,
                ..s.f.config.clone()
            },
        )
        .unwrap();
        (worker, task)
    }

    fn body(status: &str, canceled_at: Value, prices: &[&str]) -> Value {
        let items: Vec<Value> = prices
            .iter()
            .map(|price| json!({"quantity":1,"price":{"id":price}}))
            .collect();
        json!({"id":SUBSCRIPTION,"status":status,"canceled_at":canceled_at,"items":items,
            "scheduled_change":null})
    }

    /// Establishes the paid subscription and queues a sent `do_not_bill` change that waits for
    /// the renewal of the saved subscription. Returns the change job.
    async fn scheduled_change(s: &Scenario, subscription: &str) -> Uuid {
        let id = Uuid::now_v7();
        let payload = json!({"plan_id":s.plan,"price_ids":[s.price],"version_ids":[s.version],
            "external_price_ids":[REQUESTED_PRICE],"timing":"do_not_bill"});
        sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,state,attempts,external_id,error_category,available_at,created_at,updated_at) VALUES($1,$2::text::uuid,'subscription',$1::text,$3,'reconcile',1,$4,'awaiting_renewal',now()-interval '1 hour',now(),now())")
            .bind(id).bind(&s.tenant).bind(payload).bind(subscription).execute(&s.f.db).await.unwrap();
        id
    }

    async fn job(s: &Scenario, id: Uuid) -> (String, String, String) {
        sqlx::query_as("SELECT state,error_category,external_id FROM jobs WHERE id=$1")
            .bind(id)
            .fetch_one(&s.f.db)
            .await
            .unwrap()
    }

    async fn step(s: &Scenario, worker: &Worker, id: Uuid) -> (String, String) {
        sqlx::query("UPDATE jobs SET available_at=now()-interval '1 hour' WHERE id=$1")
            .bind(id)
            .execute(&s.f.db)
            .await
            .unwrap();
        worker.step().await.unwrap();
        let (state, category, _) = job(s, id).await;
        (state, category)
    }

    async fn canceled_webhook(s: &Scenario, id: &str) {
        s.deliver(
            id,
            "subscription.canceled",
            s.time.canceled(),
            json!({"id":SUBSCRIPTION,"status":"canceled"}),
        )
        .await;
    }

    /// Establishes the paid subscription; the fixture's initial job completes with the payment.
    async fn subscribed() -> Scenario {
        let s = scenario().await;
        s.payment("evt_payment").await;
        assert_eq!(s.state().await["subscription"], SUBSCRIPTION);
        s
    }

    #[tokio::test]
    #[ignore = "requires a disposable TEST_DATABASE_URL"]
    async fn accepted_cancellation_retires_the_scheduled_change_and_unblocks_the_tenant() {
        let s = subscribed().await;
        let change = scheduled_change(&s, SUBSCRIPTION).await;
        // Jobs of another subscription and unresolved initial payments are not superseded.
        let other = scheduled_change(&s, OTHER_SUBSCRIPTION).await;
        let initial = Uuid::now_v7();
        sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,state,attempts,error_category,available_at,created_at,updated_at) VALUES($1,$2::text::uuid,'subscription',$1::text,'{}','reconcile',1,'unknown_payment_outcome',now(),now(),now())")
            .bind(initial).bind(&s.tenant).execute(&s.f.db).await.unwrap();

        // While active, the scheduled change holds subscription changes and deletion.
        s.f.error(
            "owner",
            "DeleteTenant",
            json!({"requestKey":key(),"tenantId":s.tenant}),
            "failed_precondition",
        )
        .await;

        canceled_webhook(&s, "evt_canceled").await;
        let state = s.state().await;
        assert_eq!(state["free"], true, "{state}");
        assert_eq!(state["selectedVersionIsPaid"], false, "{state}");
        assert_eq!(state["paidEntitlements"], 0, "{state}");
        assert_eq!(
            job(&s, change).await,
            (
                "done".into(),
                "superseded_by_cancellation".into(),
                SUBSCRIPTION.into()
            )
        );
        assert_eq!(job(&s, other).await.0, "reconcile");
        assert_eq!(job(&s, other).await.1, "awaiting_renewal");
        assert_eq!(job(&s, initial).await.1, "unknown_payment_outcome");

        // Only the unresolved initial payment still holds the tenant.
        s.f.error(
            "owner",
            "DeleteTenant",
            json!({"requestKey":key(),"tenantId":s.tenant}),
            "failed_precondition",
        )
        .await;
        sqlx::query("UPDATE jobs SET state='done' WHERE id=ANY($1)")
            .bind(vec![other, initial])
            .execute(&s.f.db)
            .await
            .unwrap();
        s.f.rpc(
            "owner",
            "DeleteTenant",
            json!({"requestKey":key(),"tenantId":s.tenant}),
        )
        .await;
    }

    #[tokio::test]
    #[ignore = "requires a disposable TEST_DATABASE_URL"]
    async fn canceled_subscription_converges_before_and_after_the_webhook() {
        for webhook_first in [false, true] {
            for prices in [&[REQUESTED_PRICE][..], &["pri_unrelated_fixture"][..]] {
                let label = format!("webhook_first={webhook_first} prices={prices:?}");
                let s = subscribed().await;
                let change = scheduled_change(&s, SUBSCRIPTION).await;
                let response = Arc::new(Mutex::new(body(
                    "canceled",
                    json!(s.time.canceled().to_rfc3339()),
                    prices,
                )));
                let (worker, mock) = provider(&s, response).await;
                if webhook_first {
                    canceled_webhook(&s, "evt_canceled").await;
                    // Reproduce a job that stranded before cancellation retired it.
                    sqlx::query("UPDATE jobs SET state='reconcile',error_category='awaiting_renewal' WHERE id=$1")
                        .bind(change).execute(&s.f.db).await.unwrap();
                }
                assert_eq!(
                    step(&s, &worker, change).await,
                    ("done".into(), "superseded_by_cancellation".into()),
                    "{label}"
                );
                let converged = s.state().await;
                assert_eq!(converged["free"], true, "{label}: {converged}");
                assert_eq!(converged["subscription"], "", "{label}: {converged}");
                assert_eq!(converged["selectedVersionIsPaid"], false, "{label}");
                assert_eq!(converged["paidEntitlements"], 0, "{label}");
                // The real notification and its replay add no transition.
                canceled_webhook(&s, "evt_canceled_late").await;
                assert_eq!(s.state().await, converged, "{label}");
                mock.abort();
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires a disposable TEST_DATABASE_URL"]
    async fn failed_mismatch_job_recovers_through_retry_after_cancellation() {
        let s = subscribed().await;
        let change = scheduled_change(&s, SUBSCRIPTION).await;
        canceled_webhook(&s, "evt_canceled").await;
        let converged = s.state().await;
        // The stranded state of a job that cancellation could not retire.
        sqlx::query(
            "UPDATE jobs SET state='failed',error_category='subscription_mismatch' WHERE id=$1",
        )
        .bind(change)
        .execute(&s.f.db)
        .await
        .unwrap();
        s.f.error(
            "owner",
            "DeleteTenant",
            json!({"requestKey":key(),"tenantId":s.tenant}),
            "failed_precondition",
        )
        .await;
        s.f.rpc("owner","RetryJob",json!({"requestKey":key(),"jobId":change.to_string(),"reason":"recover canceled change"})).await;
        let response = Arc::new(Mutex::new(body(
            "canceled",
            json!(s.time.canceled().to_rfc3339()),
            &["pri_unrelated_fixture"],
        )));
        let (worker, mock) = provider(&s, response).await;
        assert_eq!(
            step(&s, &worker, change).await,
            ("done".into(), "superseded_by_cancellation".into())
        );
        assert_eq!(s.state().await, converged, "no paid product is revived");
        s.f.rpc(
            "owner",
            "DeleteTenant",
            json!({"requestKey":key(),"tenantId":s.tenant}),
        )
        .await;
        mock.abort();
    }

    #[tokio::test]
    #[ignore = "requires a disposable TEST_DATABASE_URL"]
    async fn reconciliation_does_not_cancel_another_subscription() {
        let s = subscribed().await;
        // The tenant now carries a newer subscription; the saved one is already canceled.
        let change = scheduled_change(&s, OTHER_SUBSCRIPTION).await;
        let mut canceled = body(
            "canceled",
            json!(s.time.canceled().to_rfc3339()),
            &[REQUESTED_PRICE],
        );
        canceled["id"] = json!(OTHER_SUBSCRIPTION);
        let (worker, mock) = provider(&s, Arc::new(Mutex::new(canceled))).await;
        let before = s.state().await;
        assert_eq!(
            step(&s, &worker, change).await,
            ("done".into(), "superseded_by_cancellation".into())
        );
        assert_eq!(s.state().await, before);
        assert_eq!(before["subscription"], SUBSCRIPTION);
        mock.abort();
    }

    #[tokio::test]
    #[ignore = "requires a disposable TEST_DATABASE_URL"]
    async fn active_subscription_keeps_waiting_and_invalid_cancellation_time_retries() {
        let s = subscribed().await;
        let change = scheduled_change(&s, SUBSCRIPTION).await;
        let response = Arc::new(Mutex::new(body("active", Value::Null, &[REQUESTED_PRICE])));
        let (worker, mock) = provider(&s, response.clone()).await;
        let before = s.state().await;
        assert_eq!(
            step(&s, &worker, change).await,
            ("reconcile".into(), "awaiting_renewal".into())
        );
        *response.lock().unwrap() = body("active", Value::Null, &["pri_unrelated_fixture"]);
        assert_eq!(
            step(&s, &worker, change).await,
            ("failed".into(), "subscription_mismatch".into())
        );
        // A canceled subscription without a usable cancellation time changes nothing.
        sqlx::query("UPDATE jobs SET state='reconcile' WHERE id=$1")
            .bind(change)
            .execute(&s.f.db)
            .await
            .unwrap();
        *response.lock().unwrap() = body("canceled", Value::Null, &[REQUESTED_PRICE]);
        assert_eq!(
            step(&s, &worker, change).await,
            ("reconcile".into(), "payment_ambiguous".into())
        );
        assert_eq!(s.state().await, before);
        mock.abort();
    }
}
