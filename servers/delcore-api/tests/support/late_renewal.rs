// SPDX-License-Identifier: FSL-1.1-ALv2
//! A recurring payment that is delivered after a newer subscription failure must not clear the
//! failure, whatever the delivery order, while a newer confirmed renewal still recovers it.
use super::{Fixture, key};
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

const SUBSCRIPTION: &str = "sub_order";
const CUSTOMER: &str = "ctm_order";
const EXTERNAL_PRICE: &str = "pri_order";

/// Provider occurrence times. Periods last 40 days, so the renewed period covers the request
/// clock for any current date and the authorization checks stay independent of the month length.
struct Timeline {
    month: DateTime<Utc>,
}
impl Timeline {
    fn now() -> Self {
        let today = Utc::now();
        Self {
            month: Utc
                .with_ymd_and_hms(today.year(), today.month(), 1, 0, 0, 0)
                .single()
                .unwrap(),
        }
    }
    fn days(&self, days: i64) -> DateTime<Utc> {
        self.month + Duration::days(days)
    }
    /// Seeded confirmed period `[-40, 0)` days.
    fn seeded_end(&self) -> DateTime<Utc> {
        self.days(0)
    }
    /// Renewal A: period `[0, 40)` days, occurrence one second after the period start.
    fn payment_a(&self) -> (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>) {
        (
            self.days(0),
            self.days(40),
            self.days(0) + Duration::seconds(1),
        )
    }
    /// Failure B occurs after A and before the next renewal.
    fn past_due(&self) -> DateTime<Utc> {
        self.days(20)
    }
    /// Informational active status between B and the next renewal.
    fn active(&self) -> DateTime<Utc> {
        self.days(25)
    }
    /// Renewal C: period `[40, 80)` days, occurrence after B and the active status.
    fn payment_c(&self) -> (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>) {
        (
            self.days(40),
            self.days(80),
            self.days(40) + Duration::seconds(1),
        )
    }
}

struct Scenario {
    f: Fixture,
    tenant: String,
    user: Value,
    time: Timeline,
}

async fn scenario() -> Scenario {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    // Authorization checks need a registered calling service.
    let service = Uuid::parse_str(&f.service().await).unwrap();
    let paid = f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"paid plan","plan":{"name":"pro","rank":10,"free":false}})).await;
    let plan = Uuid::parse_str(paid["plan"]["id"].as_str().unwrap()).unwrap();
    let user = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let tenant = user["tenants"][0]["id"].as_str().unwrap().to_owned();
    let tenant_id = Uuid::parse_str(&tenant).unwrap();
    let time = Timeline::now();
    // A recurring-only entitlement set keeps the renewal independent of usage allowances.
    let price = Uuid::now_v7();
    sqlx::query("INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,kind) VALUES($1,$2,$4,'base',5,0,'USD',$3,now(),2)")
        .bind(price).bind(plan).bind(EXTERNAL_PRICE).bind(service).execute(&f.db).await.unwrap();
    // A postpaid plan keeps the allowance-depletion check out of the authorization result.
    sqlx::query("UPDATE plans SET postpaid=true WHERE id=$1")
        .bind(plan)
        .execute(&f.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2,$3,$4)")
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind(price)
        .bind(time.days(-40))
        .execute(&f.db)
        .await
        .unwrap();
    sqlx::query("UPDATE tenants SET plan_id=$2,external_customer_id=$3,external_subscription_id=$4,period_start=$5,period_end=$6 WHERE id=$1")
        .bind(tenant_id).bind(plan).bind(CUSTOMER).bind(SUBSCRIPTION).bind(time.days(-40)).bind(time.seeded_end()).execute(&f.db).await.unwrap();
    sqlx::query("INSERT INTO provider_objects VALUES($1,$2,'active')")
        .bind(SUBSCRIPTION)
        .bind(time.days(-40))
        .execute(&f.db)
        .await
        .unwrap();
    Scenario {
        f,
        tenant,
        user,
        time,
    }
}

impl Scenario {
    async fn deliver(&self, id: &str, kind: &str, occurred: DateTime<Utc>, data: Value) {
        self.try_deliver(id, kind, occurred, data).await.unwrap();
    }
    async fn try_deliver(
        &self,
        id: &str,
        kind: &str,
        occurred: DateTime<Utc>,
        data: Value,
    ) -> anyhow::Result<()> {
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
        self.f.worker.process_webhook(id).await
    }
    fn completion(
        &self,
        txn: &str,
        period: (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>),
    ) -> Value {
        json!({"id":txn,"status":"completed","origin":"subscription_recurring","customer_id":CUSTOMER,"subscription_id":SUBSCRIPTION,"currency_code":"USD",
            "items":[{"price":{"id":EXTERNAL_PRICE,"product_id":"pro_order"},"quantity":1}],
            "billing_period":{"starts_at":period.0.to_rfc3339(),"ends_at":period.1.to_rfc3339()}})
    }
    async fn pay(
        &self,
        event: &str,
        txn: &str,
        period: (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>),
    ) {
        let data = self.completion(txn, period);
        self.deliver(event, "transaction.completed", period.2, data)
            .await;
    }
    async fn pay_a(&self, event: &str) {
        self.pay(event, "txn_a", self.time.payment_a()).await;
    }
    async fn pay_c(&self, event: &str) {
        self.pay(event, "txn_c", self.time.payment_c()).await;
    }
    async fn status(&self, event: &str, kind: &str, status: &str, occurred: DateTime<Utc>) {
        self.deliver(
            event,
            kind,
            occurred,
            json!({"id":SUBSCRIPTION,"status":status}),
        )
        .await;
    }
    async fn past_due(&self, event: &str) {
        self.status(
            event,
            "subscription.past_due",
            "past_due",
            self.time.past_due(),
        )
        .await;
    }
    async fn active(&self, event: &str) {
        self.status(
            event,
            "subscription.activated",
            "active",
            self.time.active(),
        )
        .await;
    }
    async fn flag(&self) -> bool {
        sqlx::query_scalar("SELECT renewal_failed FROM tenants WHERE id=$1::text::uuid")
            .bind(&self.tenant)
            .fetch_one(&self.f.db)
            .await
            .unwrap()
    }
    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE tenant_id=$1::text::uuid"
        ))
        .bind(&self.tenant)
        .fetch_one(&self.f.db)
        .await
        .unwrap()
    }
    async fn object(&self, id: &str) -> Value {
        let row: Option<(DateTime<Utc>, String)> =
            sqlx::query_as("SELECT occurred_at,state FROM provider_objects WHERE external_id=$1")
                .bind(id)
                .fetch_optional(&self.f.db)
                .await
                .unwrap();
        row.map_or(
            Value::Null,
            |(at, state)| json!({"at":at.to_rfc3339(),"state":state}),
        )
    }
    /// Returns the failure, period, watermark and payment-accounting state of the tenant.
    async fn state(&self) -> Value {
        let row: (bool, DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
            "SELECT renewal_failed,period_start,period_end FROM tenants WHERE id=$1::text::uuid",
        )
        .bind(&self.tenant)
        .fetch_one(&self.f.db)
        .await
        .unwrap();
        let transactions: Vec<String> = sqlx::query_scalar("SELECT external_id FROM provider_transactions WHERE tenant_id=$1::text::uuid ORDER BY external_id")
            .bind(&self.tenant).fetch_all(&self.f.db).await.unwrap();
        let periods: Vec<String> = sqlx::query_scalar("SELECT starts_at::text FROM billing_periods WHERE tenant_id=$1::text::uuid ORDER BY starts_at")
            .bind(&self.tenant).fetch_all(&self.f.db).await.unwrap();
        json!({"renewalFailed":row.0,"start":row.1.to_rfc3339(),"end":row.2.to_rfc3339(),
            "subscription":self.object(SUBSCRIPTION).await,
            "failure":self.object(&format!("failure:{SUBSCRIPTION}")).await,
            "transactions":transactions,"billingPeriods":periods,
            "grants":self.count("allowance_grants").await,
            "openEntitlements":sqlx::query_scalar::<_, i64>("SELECT count(*) FROM entitlements WHERE tenant_id=$1::text::uuid AND ends_at IS NULL").bind(&self.tenant).fetch_one(&self.f.db).await.unwrap()})
    }
    async fn authorize(&self) -> Value {
        self.f.rpc("service","Authorize",json!({"tenantId":self.tenant,"userId":self.user["userId"],"capability":"execute","paid":true})).await
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn older_renewal_keeps_newer_failure_in_either_delivery_order() {
    let mut finals = Vec::new();
    for failure_first in [true, false] {
        let s = scenario().await;
        if failure_first {
            s.past_due("evt_b").await;
            assert!(s.flag().await);
            assert_eq!(s.state().await["end"], s.time.seeded_end().to_rfc3339());
            s.pay_a("evt_a").await;
        } else {
            s.pay_a("evt_a").await;
            assert!(!s.flag().await);
            s.past_due("evt_b").await;
        }
        let state = s.state().await;
        let label = format!("failureFirst={failure_first}: {state}");
        assert_eq!(state["renewalFailed"], true, "{label}");
        assert_eq!(state["start"], s.time.payment_a().0.to_rfc3339(), "{label}");
        assert_eq!(state["end"], s.time.payment_a().1.to_rfc3339(), "{label}");
        assert_eq!(
            state["subscription"],
            json!({"at":s.time.past_due().to_rfc3339(),"state":"past_due"}),
            "{label}"
        );
        assert_eq!(state["transactions"], json!(["txn_a"]), "{label}");
        assert_eq!(
            state["billingPeriods"].as_array().unwrap().len(),
            1,
            "{label}"
        );
        assert_eq!(
            s.authorize().await["reasons"],
            json!(["BLOCK_REASON_RENEWAL"])
        );
        finals.push(state);
    }
    assert_eq!(
        finals[0], finals[1],
        "final state must not depend on delivery order"
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn active_status_does_not_let_an_older_renewal_clear_the_failure() {
    let s = scenario().await;
    s.past_due("evt_b").await;
    s.active("evt_active").await;
    assert_eq!(
        s.state().await["subscription"],
        json!({"at":s.time.active().to_rfc3339(),"state":"active"})
    );
    assert!(s.flag().await, "active status alone must not clear failure");
    s.pay_a("evt_a").await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], true, "{state}");
    assert_eq!(state["end"], s.time.payment_a().1.to_rfc3339(), "{state}");
    assert_eq!(state["transactions"], json!(["txn_a"]), "{state}");
    assert_eq!(
        state["failure"],
        json!({"at":s.time.past_due().to_rfc3339(),"state":"past_due"}),
        "{state}"
    );
    // The watermark keeps the newer informational status.
    assert_eq!(state["subscription"]["state"], "active", "{state}");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn newer_renewal_recovers_the_failure_and_advances_the_period() {
    for (label, with_a, with_active) in [
        ("failure then recovery", false, false),
        ("renewal, failure, active, recovery", true, true),
    ] {
        let s = scenario().await;
        if with_a {
            s.pay_a("evt_a").await;
        }
        s.past_due("evt_b").await;
        if with_active {
            s.active("evt_active").await;
            assert!(s.flag().await, "{label}: active alone keeps the failure");
        }
        assert!(s.flag().await, "{label}");
        s.pay_c("evt_c").await;
        let state = s.state().await;
        assert_eq!(state["renewalFailed"], false, "{label}: {state}");
        assert_eq!(
            state["start"],
            s.time.payment_c().0.to_rfc3339(),
            "{label}: {state}"
        );
        assert_eq!(
            state["end"],
            s.time.payment_c().1.to_rfc3339(),
            "{label}: {state}"
        );
        assert_eq!(state["failure"], Value::Null, "{label}: {state}");
        assert_eq!(
            state["subscription"],
            json!({"at":s.time.payment_c().2.to_rfc3339(),"state":"active"}),
            "{label}: {state}"
        );
        let expected: Vec<&str> = if with_a {
            vec!["txn_a", "txn_c"]
        } else {
            vec!["txn_c"]
        };
        assert_eq!(state["transactions"], json!(expected), "{label}: {state}");
        assert_eq!(
            state["billingPeriods"].as_array().unwrap().len(),
            expected.len()
        );
        assert_eq!(s.authorize().await["allowed"], true, "{label}");
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn duplicate_and_stale_events_keep_failure_and_accounting() {
    let s = scenario().await;
    s.past_due("evt_b").await;
    s.pay_a("evt_a").await;
    let after = s.state().await;
    assert_eq!(after["renewalFailed"], true);
    // Reprocessing the original event IDs is a no-op.
    s.f.worker.process_webhook("evt_a").await.unwrap();
    s.f.worker.process_webhook("evt_b").await.unwrap();
    // The same occurrences under fresh event IDs are stale or already applied.
    s.past_due("evt_b_again").await;
    s.pay_a("evt_a_again").await;
    s.status(
        "evt_older_past_due",
        "subscription.past_due",
        "past_due",
        s.time.past_due() - Duration::days(1),
    )
    .await;
    assert_eq!(s.state().await, after);
    // An older active status cannot erase the failure either.
    s.status(
        "evt_older_active",
        "subscription.activated",
        "active",
        s.time.past_due() - Duration::days(2),
    )
    .await;
    assert_eq!(s.state().await, after);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn invalid_renewals_do_not_clear_the_failure() {
    let s = scenario().await;
    s.past_due("evt_b").await;
    let before = s.state().await;
    let c = s.time.payment_c();
    let mut mismatches = Vec::new();
    let mut customer = s.completion("txn_customer", c);
    customer["customer_id"] = json!("ctm_other");
    mismatches.push(customer);
    let mut currency = s.completion("txn_currency", c);
    currency["currency_code"] = json!("EUR");
    mismatches.push(currency);
    let mut items = s.completion("txn_items", c);
    items["items"][0]["price"]["id"] = json!("pri_other");
    mismatches.push(items);
    let mut quantity = s.completion("txn_quantity", c);
    quantity["items"][0]["quantity"] = json!(2);
    mismatches.push(quantity);
    // A period that starts before the confirmed period end does not advance the subscription.
    let non_advancing = s.completion(
        "txn_period",
        (
            s.time.days(-10),
            s.time.days(30),
            s.time.days(40) + Duration::seconds(2),
        ),
    );
    mismatches.push(non_advancing);
    for (index, data) in mismatches.into_iter().enumerate() {
        s.deliver(
            &format!("evt_invalid_{index}"),
            "transaction.completed",
            c.2 + Duration::seconds(index as i64),
            data,
        )
        .await;
        assert_eq!(s.state().await["renewalFailed"], true, "mismatch {index}");
    }
    assert_eq!(s.state().await, before);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn failed_renewal_rolls_back_failure_recovery() {
    let s = scenario().await;
    s.past_due("evt_b").await;
    let before = s.state().await;
    sqlx::query("CREATE OR REPLACE FUNCTION fixture_fail_period() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'fixture period failure'; END $$ LANGUAGE plpgsql")
        .execute(&s.f.db).await.unwrap();
    sqlx::query("CREATE TRIGGER fixture_fail_period BEFORE INSERT ON billing_periods FOR EACH ROW EXECUTE FUNCTION fixture_fail_period()")
        .execute(&s.f.db).await.unwrap();
    let c = s.time.payment_c();
    let data = s.completion("txn_c", c);
    assert!(
        s.try_deliver("evt_c", "transaction.completed", c.2, data)
            .await
            .is_err()
    );
    assert_eq!(
        s.state().await,
        before,
        "failed renewal must leave no partial state"
    );
    let processed: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT processed_at FROM webhooks WHERE event_id='evt_c'")
            .fetch_one(&s.f.db)
            .await
            .unwrap();
    assert!(processed.is_none());
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn independent_restrictions_remain_alongside_the_preserved_failure() {
    for (restriction, reason) in [
        ("debt", "BLOCK_REASON_BALANCE"),
        ("dispute", "BLOCK_REASON_DISPUTE"),
        ("operator", "BLOCK_REASON_OPERATOR"),
        ("deletion", "BLOCK_REASON_DELETING"),
    ] {
        let s = scenario().await;
        let tenant = Uuid::parse_str(&s.tenant).unwrap();
        s.pay_a("evt_a").await;
        let clean = s.authorize().await;
        assert_eq!(clean["allowed"], true, "{restriction}: {clean}");
        s.past_due("evt_b").await;
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
        let result = s.authorize().await;
        assert!(
            !result["allowed"].as_bool().unwrap_or(false),
            "{restriction}: {result}"
        );
        let reasons = result["reasons"].as_array().unwrap();
        assert!(reasons.contains(&json!(reason)), "{restriction}: {result}");
        assert!(
            reasons.contains(&json!("BLOCK_REASON_RENEWAL")),
            "{restriction}: {result}"
        );
        // A late older renewal still leaves each restriction and the failure in place.
        s.pay_c("evt_c").await;
        let recovered = s.authorize().await;
        let reasons = recovered["reasons"].as_array().cloned().unwrap_or_default();
        assert!(
            reasons.contains(&json!(reason)),
            "{restriction}: {recovered}"
        );
        assert!(
            !reasons.contains(&json!("BLOCK_REASON_RENEWAL")),
            "{restriction}: {recovered}"
        );
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn reconciled_renewal_is_ordered_by_provider_billing_time() {
    let s = scenario().await;
    // Reconciliation applies the payment at the current time, long after it was billed. A failure
    // that occurred after billing but before reconciliation must still be newer than the payment.
    let (starts, ends, billed) = s.time.payment_a();
    s.status(
        "evt_b",
        "subscription.past_due",
        "past_due",
        billed + Duration::minutes(1),
    )
    .await;
    assert!(s.flag().await);
    let mut data = s.completion("txn_a", (starts, ends, billed));
    data["billed_at"] = json!(billed.to_rfc3339());
    let t: delcore_api::provider::Transaction = serde_json::from_value(data).unwrap();
    s.f.worker.apply_transaction(t, Utc::now()).await.unwrap();
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], true, "{state}");
    assert_eq!(state["end"], ends.to_rfc3339(), "{state}");
    assert_eq!(state["transactions"], json!(["txn_a"]), "{state}");
    assert_eq!(
        state["failure"],
        json!({"at":(billed + Duration::minutes(1)).to_rfc3339(),"state":"past_due"}),
        "{state}"
    );
    assert_eq!(
        s.authorize().await["reasons"],
        json!(["BLOCK_REASON_RENEWAL"])
    );
    // A renewal billed after the failure still recovers it.
    let (starts, ends, _) = s.time.payment_c();
    let mut data = s.completion("txn_c", (starts, ends, starts));
    data["billed_at"] = json!((billed + Duration::minutes(2)).to_rfc3339());
    let t: delcore_api::provider::Transaction = serde_json::from_value(data).unwrap();
    s.f.worker.apply_transaction(t, Utc::now()).await.unwrap();
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], false, "{state}");
    assert_eq!(state["failure"], Value::Null, "{state}");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn stale_past_due_after_a_later_active_status_still_fails_the_tenant() {
    let s = scenario().await;
    // The informational status is newer than the failure, so the failure is stale by the
    // watermark, but no payment resolved it.
    s.active("evt_active").await;
    assert!(!s.flag().await);
    s.past_due("evt_b").await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], true, "{state}");
    assert_eq!(
        state["failure"],
        json!({"at":s.time.past_due().to_rfc3339(),"state":"past_due"}),
        "{state}"
    );
    assert_eq!(
        state["subscription"],
        json!({"at":s.time.active().to_rfc3339(),"state":"active"}),
        "{state}"
    );
    assert_eq!(
        s.authorize().await["reasons"],
        json!(["BLOCK_REASON_RENEWAL"])
    );
    // An older renewal leaves the failure, a newer one recovers it.
    s.pay_a("evt_a").await;
    assert!(s.flag().await);
    s.pay_c("evt_c").await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], false, "{state}");
    assert_eq!(state["failure"], Value::Null, "{state}");
    assert_eq!(s.authorize().await["allowed"], true);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn stale_past_due_after_a_newer_payment_does_not_fail_the_tenant() {
    let s = scenario().await;
    s.pay_c("evt_c").await;
    assert!(!s.flag().await);
    // The failure occurred before the confirmed renewal, which already resolved it.
    s.past_due("evt_b").await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], false, "{state}");
    assert_eq!(state["failure"], Value::Null, "{state}");
    assert_eq!(s.authorize().await["allowed"], true);
}
