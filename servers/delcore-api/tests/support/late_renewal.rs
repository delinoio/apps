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
    /// Delivers a failed recurring transaction for the period `[starts, ends)` that occurred at
    /// `failed_at`.
    async fn fail(
        &self,
        event: &str,
        txn: &str,
        period: (DateTime<Utc>, DateTime<Utc>),
        failed_at: DateTime<Utc>,
    ) {
        let mut data = self.completion(txn, (period.0, period.1, failed_at));
        data["status"] = json!("past_due");
        self.deliver(event, "transaction.payment_failed", failed_at, data)
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

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn transaction_payment_failure_keeps_its_occurrence_against_an_older_renewal() {
    let s = scenario().await;
    // Renewal B for the period after A fails before A's completion is delivered.
    let (starts, ends, _) = s.time.payment_c();
    let failed_at = s.time.days(41);
    let mut data = s.completion("txn_b", (starts, ends, failed_at));
    data["status"] = json!("past_due");
    s.deliver("evt_b", "transaction.payment_failed", failed_at, data)
        .await;
    assert!(s.flag().await);
    assert_eq!(
        s.state().await["failure"],
        json!({"at":failed_at.to_rfc3339(),"state":"past_due"})
    );
    s.pay_a("evt_a").await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], true, "{state}");
    assert_eq!(state["end"], s.time.payment_a().1.to_rfc3339(), "{state}");
    assert_eq!(state["transactions"], json!(["txn_a"]), "{state}");
    assert_eq!(
        s.authorize().await["reasons"],
        json!(["BLOCK_REASON_RENEWAL"])
    );
    // A renewal confirmed after the failure recovers it.
    s.pay(
        "evt_d",
        "txn_d",
        (starts, ends, s.time.days(41) + Duration::seconds(1)),
    )
    .await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], false, "{state}");
    assert_eq!(state["failure"], Value::Null, "{state}");
    assert_eq!(s.authorize().await["allowed"], true);
}

/// Turns the seeded scenario into one whose subscription is not associated with the tenant yet
/// and returns the pending initial-subscription job.
async fn unassociate(s: &Scenario) -> String {
    let tenant = Uuid::parse_str(&s.tenant).unwrap();
    let plan: Uuid = sqlx::query_scalar("SELECT plan_id FROM tenants WHERE id=$1")
        .bind(tenant)
        .fetch_one(&s.f.db)
        .await
        .unwrap();
    let price: Uuid = sqlx::query_scalar("SELECT id FROM prices WHERE external_price_id=$1")
        .bind(EXTERNAL_PRICE)
        .fetch_one(&s.f.db)
        .await
        .unwrap();
    let version: Uuid =
        sqlx::query_scalar("SELECT active_version_id FROM catalog_entities WHERE id=$1")
            .bind(plan)
            .fetch_one(&s.f.db)
            .await
            .unwrap();
    sqlx::query("DELETE FROM entitlements WHERE tenant_id=$1")
        .bind(tenant)
        .execute(&s.f.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM provider_objects WHERE external_id=$1")
        .bind(SUBSCRIPTION)
        .execute(&s.f.db)
        .await
        .unwrap();
    sqlx::query("UPDATE tenants SET plan_id=(SELECT id FROM plans WHERE is_default AND free),external_customer_id='',external_subscription_id='' WHERE id=$1")
        .bind(tenant)
        .execute(&s.f.db)
        .await
        .unwrap();
    let job = key();
    let payload = json!({"plan_id":plan,"price_ids":[price],"version_ids":[version],"external_price_ids":[EXTERNAL_PRICE]});
    sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,state,attempts,available_at,created_at,updated_at) VALUES($1,$2,'subscription',$3,$4,'pending',0,now(),now(),now())")
        .bind(Uuid::parse_str(&job).unwrap()).bind(tenant).bind(format!("fixture:{job}")).bind(payload).execute(&s.f.db).await.unwrap();
    job
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn retained_failure_survives_initial_association_against_a_recurring_payment() {
    let s = scenario().await;
    let job = unassociate(&s).await;
    // The initial payment occurred first, then renewal A, then the failure B. Notifications for
    // the failure arrive before the tenant is associated, the recurring completion arrives last.
    let initial_start = s.time.days(-40);
    let initial_at = initial_start + Duration::seconds(1);
    s.past_due("evt_b").await;
    assert!(!s.flag().await);
    let data = json!({"id":"txn_initial","status":"completed","origin":"web","customer_id":CUSTOMER,"subscription_id":SUBSCRIPTION,"currency_code":"USD",
        "custom_data":{"delcore_job_id":job,"delcore_tenant_id":s.tenant},
        "items":[{"price":{"id":EXTERNAL_PRICE,"product_id":"pro_order"},"quantity":1}],
        "billing_period":{"starts_at":initial_start.to_rfc3339(),"ends_at":s.time.days(0).to_rfc3339()},
        "billed_at":initial_at.to_rfc3339()});
    s.deliver("evt_initial", "transaction.completed", initial_at, data)
        .await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], true, "{state}");
    assert_eq!(
        state["failure"],
        json!({"at":s.time.past_due().to_rfc3339(),"state":"past_due"}),
        "{state}"
    );
    s.pay_a("evt_a").await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], true, "{state}");
    assert_eq!(state["end"], s.time.payment_a().1.to_rfc3339(), "{state}");
    assert_eq!(
        state["transactions"],
        json!(["txn_a", "txn_initial"]),
        "{state}"
    );
    // The initial association grants no usage allowance in this fixture, so the balance reason
    // may accompany the renewal failure; the failure itself must still block paid use.
    assert!(
        s.authorize().await["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("BLOCK_REASON_RENEWAL"))
    );
    // A renewal confirmed after the failure recovers it.
    s.pay_c("evt_c").await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], false, "{state}");
    assert_eq!(state["failure"], Value::Null, "{state}");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn recurring_payment_failure_before_association_is_retained_for_the_initial_payment() {
    let s = scenario().await;
    let job = unassociate(&s).await;
    let (starts, ends, _) = s.time.payment_c();
    let failed_at = s.time.days(41);
    // The failed renewal of the period after A arrives while the initial payment is still
    // unreconciled, so no tenant carries the subscription yet.
    s.fail("evt_b", "txn_b", (starts, ends), failed_at).await;
    assert!(!s.flag().await);
    let state = s.state().await;
    assert_eq!(
        state["failure"],
        json!({"at":failed_at.to_rfc3339(),"state":"past_due"}),
        "{state}"
    );
    // The older initial payment then establishes the association and must see the failure.
    let initial_start = s.time.days(-40);
    let initial_at = initial_start + Duration::seconds(1);
    let data = json!({"id":"txn_initial","status":"completed","origin":"web","customer_id":CUSTOMER,"subscription_id":SUBSCRIPTION,"currency_code":"USD",
        "custom_data":{"delcore_job_id":job,"delcore_tenant_id":s.tenant},
        "items":[{"price":{"id":EXTERNAL_PRICE,"product_id":"pro_order"},"quantity":1}],
        "billing_period":{"starts_at":initial_start.to_rfc3339(),"ends_at":s.time.days(0).to_rfc3339()},
        "billed_at":initial_at.to_rfc3339()});
    s.deliver("evt_initial", "transaction.completed", initial_at, data)
        .await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], true, "{state}");
    assert_eq!(
        state["failure"],
        json!({"at":failed_at.to_rfc3339(),"state":"past_due"}),
        "{state}"
    );
    assert!(
        s.authorize().await["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("BLOCK_REASON_RENEWAL"))
    );
    // An older renewal leaves the failure; a renewal confirmed after it recovers it.
    s.pay_a("evt_a").await;
    assert!(s.flag().await);
    s.pay(
        "evt_d",
        "txn_d",
        (starts, ends, failed_at + Duration::seconds(1)),
    )
    .await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], false, "{state}");
    assert_eq!(state["failure"], Value::Null, "{state}");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn payment_failure_before_association_older_than_the_initial_payment_is_ignored() {
    let s = scenario().await;
    let job = unassociate(&s).await;
    // A retained failure that occurred before the initial payment was already resolved by it.
    let initial_start = s.time.days(-40);
    let initial_at = initial_start + Duration::seconds(1);
    s.fail(
        "evt_b",
        "txn_b",
        (initial_start, s.time.days(0)),
        initial_start - Duration::hours(1),
    )
    .await;
    let data = json!({"id":"txn_initial","status":"completed","origin":"web","customer_id":CUSTOMER,"subscription_id":SUBSCRIPTION,"currency_code":"USD",
        "custom_data":{"delcore_job_id":job,"delcore_tenant_id":s.tenant},
        "items":[{"price":{"id":EXTERNAL_PRICE,"product_id":"pro_order"},"quantity":1}],
        "billing_period":{"starts_at":initial_start.to_rfc3339(),"ends_at":s.time.days(0).to_rfc3339()},
        "billed_at":initial_at.to_rfc3339()});
    s.deliver("evt_initial", "transaction.completed", initial_at, data)
        .await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], false, "{state}");
    assert_eq!(state["failure"], Value::Null, "{state}");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn payment_failure_for_an_associated_tenant_is_retained_only_when_it_applies() {
    let s = scenario().await;
    // The seeded period ends at day 0. A failure for a period that starts earlier is already
    // covered, and one for another customer is not this tenant's: neither is retained.
    s.fail(
        "evt_covered",
        "txn_covered",
        (s.time.days(-10), s.time.days(30)),
        s.time.days(-9),
    )
    .await;
    let mut data = s.completion(
        "txn_other",
        (
            s.time.days(0),
            s.time.days(40),
            s.time.days(0) + Duration::seconds(5),
        ),
    );
    data["status"] = json!("past_due");
    data["customer_id"] = json!("ctm_other");
    s.deliver(
        "evt_other",
        "transaction.payment_failed",
        s.time.days(0) + Duration::seconds(5),
        data,
    )
    .await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], false, "{state}");
    assert_eq!(state["failure"], Value::Null, "{state}");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn payment_of_a_replaced_subscription_does_not_resolve_a_stale_past_due() {
    let s = scenario().await;
    // The tenant replaced an earlier subscription. Its payment is recorded for the tenant, at
    // an instant after the current subscription's failure, but belongs to another subscription.
    let tenant = Uuid::parse_str(&s.tenant).unwrap();
    sqlx::query(
        "INSERT INTO provider_transactions VALUES('txn_replaced',$1,'subscription',0,'USD',$2)",
    )
    .bind(tenant)
    .bind(s.time.days(30))
    .execute(&s.f.db)
    .await
    .unwrap();
    // The informational status advances the watermark past the failure, which is delivered late.
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
        s.authorize().await["reasons"],
        json!(["BLOCK_REASON_RENEWAL"])
    );
    // A payment of the current subscription after the failure still recovers it.
    s.pay_c("evt_c").await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], false, "{state}");
    assert_eq!(state["failure"], Value::Null, "{state}");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn initial_payment_resolves_a_stale_past_due_of_its_subscription() {
    let s = scenario().await;
    let job = unassociate(&s).await;
    let initial_start = s.time.days(-40);
    let initial_at = initial_start + Duration::seconds(1);
    let data = json!({"id":"txn_initial","status":"completed","origin":"web","customer_id":CUSTOMER,"subscription_id":SUBSCRIPTION,"currency_code":"USD",
        "custom_data":{"delcore_job_id":job,"delcore_tenant_id":s.tenant},
        "items":[{"price":{"id":EXTERNAL_PRICE,"product_id":"pro_order"},"quantity":1}],
        "billing_period":{"starts_at":initial_start.to_rfc3339(),"ends_at":s.time.days(0).to_rfc3339()},
        "billed_at":initial_at.to_rfc3339()});
    s.deliver("evt_initial", "transaction.completed", initial_at, data)
        .await;
    // The failure occurred before the confirmed initial payment, which already resolved it.
    s.status(
        "evt_b",
        "subscription.past_due",
        "past_due",
        initial_at - Duration::seconds(1),
    )
    .await;
    let state = s.state().await;
    assert_eq!(state["renewalFailed"], false, "{state}");
    assert_eq!(state["failure"], Value::Null, "{state}");
}

// A confirmed renewal can reach Delcore before the initial payment that associates its
// subscription with the tenant. The renewal stays unprocessed until the association exists.

const USAGE_ALLOWANCE: &str = "100";

impl Scenario {
    /// Adds a plan usage price with an included allowance and selects it with the initial
    /// subscription job, so the initial and renewed periods each grant that allowance.
    async fn with_usage_allowance(&self, job: &str) {
        let (plan, service): (Uuid, Uuid) =
            sqlx::query_as("SELECT plan_id,service_id FROM prices WHERE external_price_id=$1")
                .bind(EXTERNAL_PRICE)
                .fetch_one(&self.f.db)
                .await
                .unwrap();
        let usage = Uuid::now_v7();
        sqlx::query("INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,kind) VALUES($1,$2,$3,'calls',1,$4::text::numeric,'USD','',now(),1)")
            .bind(usage).bind(plan).bind(service).bind(USAGE_ALLOWANCE).execute(&self.f.db).await.unwrap();
        sqlx::query("UPDATE jobs SET payload=jsonb_set(payload,'{price_ids}',payload->'price_ids'||to_jsonb($2::text)) WHERE id=$1")
            .bind(Uuid::parse_str(job).unwrap()).bind(usage.to_string()).execute(&self.f.db).await.unwrap();
    }
    /// Keeps the initial job unfinished and away from the worker, as when its payment is still
    /// being reconciled.
    async fn park_initial(&self, job: &str) {
        sqlx::query("UPDATE jobs SET state='reconcile',external_id='txn_initial',available_at=now()+interval '1 day' WHERE id=$1")
            .bind(Uuid::parse_str(job).unwrap()).execute(&self.f.db).await.unwrap();
    }
    fn initial_data(&self, job: &str) -> Value {
        let start = self.time.days(-40);
        json!({"id":"txn_initial","status":"completed","origin":"web","customer_id":CUSTOMER,"subscription_id":SUBSCRIPTION,"currency_code":"USD",
            "custom_data":{"delcore_job_id":job,"delcore_tenant_id":self.tenant},
            "items":[{"price":{"id":EXTERNAL_PRICE,"product_id":"pro_order"},"quantity":1}],
            "billing_period":{"starts_at":start.to_rfc3339(),"ends_at":self.time.days(0).to_rfc3339()},
            "billed_at":(start + Duration::seconds(1)).to_rfc3339()})
    }
    async fn pay_initial(&self, event: &str, job: &str) {
        let at = self.time.days(-40) + Duration::seconds(1);
        self.deliver(event, "transaction.completed", at, self.initial_data(job))
            .await;
    }
    /// The recurring payment for the period after the initial one, carrying the custom data the
    /// provider copies from the subscription.
    fn renewal_data(&self, txn: &str, job: &str) -> Value {
        let mut data = self.completion(txn, self.time.payment_a());
        data["custom_data"] = json!({"delcore_job_id":job,"delcore_tenant_id":self.tenant});
        data
    }
    /// Stores a renewal notification without processing it.
    async fn store_renewal(&self, event: &str, data: Value) {
        let at = self.time.payment_a().2;
        let payload = json!({"event_id":event,"event_type":"transaction.completed","occurred_at":at.to_rfc3339(),"data":data});
        sqlx::query(
            "INSERT INTO webhooks(event_id,occurred_at,payload,received_at) VALUES($1,$2,$3,now())",
        )
        .bind(event)
        .bind(at)
        .bind(payload)
        .execute(&self.f.db)
        .await
        .unwrap();
    }
    async fn processed(&self, event: &str) -> bool {
        sqlx::query_scalar("SELECT processed_at IS NOT NULL FROM webhooks WHERE event_id=$1")
            .bind(event)
            .fetch_one(&self.f.db)
            .await
            .unwrap()
    }
    /// The accounting, period and allowance state that two delivery orders must agree on.
    async fn converged(&self) -> Value {
        let mut state = self.state().await;
        let grants: Vec<String> = sqlx::query_scalar("SELECT period_start::text||'/'||period_end::text||'/'||meter||'='||amount::text FROM allowance_grants WHERE tenant_id=$1::text::uuid ORDER BY period_start,meter")
            .bind(&self.tenant).fetch_all(&self.f.db).await.unwrap();
        state["grantList"] = json!(grants);
        let entitlements: i64 =
            sqlx::query_scalar("SELECT count(*) FROM entitlements WHERE tenant_id=$1::text::uuid")
                .bind(&self.tenant)
                .fetch_one(&self.f.db)
                .await
                .unwrap();
        state["entitlementRows"] = json!(entitlements);
        state
    }
    async fn job_row(&self, id: &str) -> (String, String, i32) {
        sqlx::query_as("SELECT state,error_category,attempts FROM jobs WHERE id=$1")
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(&self.f.db)
            .await
            .unwrap()
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn renewal_before_initial_payment_converges_with_the_reverse_order() {
    let mut finals = Vec::new();
    for renewal_first in [true, false] {
        let s = scenario().await;
        let job = unassociate(&s).await;
        s.with_usage_allowance(&job).await;
        s.park_initial(&job).await;
        let label = format!("renewalFirst={renewal_first}");
        s.store_renewal("evt_renewal", s.renewal_data("txn_renewal", &job))
            .await;
        if renewal_first {
            // The renewal is retained: nothing is recorded, the tenant is not paid and the
            // notification stays unprocessed.
            assert!(s.f.worker.process_webhook("evt_renewal").await.is_err());
            assert!(!s.processed("evt_renewal").await, "{label}");
            assert_eq!(s.count("provider_transactions").await, 0, "{label}");
            assert_eq!(s.count("billing_periods").await, 0, "{label}");
            assert_eq!(s.count("allowance_grants").await, 0, "{label}");
            assert_eq!(s.count("entitlements").await, 0, "{label}");
            assert_ne!(s.authorize().await["allowed"], true, "{label}");
            s.pay_initial("evt_initial", &job).await;
            // The initial payment installs only its own period until the renewal is retried.
            let state = s.state().await;
            assert_eq!(
                state["end"],
                s.time.days(0).to_rfc3339(),
                "{label}: {state}"
            );
            assert!(!s.processed("evt_renewal").await, "{label}");
            s.f.worker.process_webhook("evt_renewal").await.unwrap();
            assert!(s.processed("evt_renewal").await, "{label}");
        } else {
            s.pay_initial("evt_initial", &job).await;
            s.f.worker.process_webhook("evt_renewal").await.unwrap();
            assert!(s.processed("evt_renewal").await, "{label}");
        }
        let state = s.converged().await;
        assert_eq!(
            state["transactions"],
            json!(["txn_initial", "txn_renewal"]),
            "{label}: {state}"
        );
        assert_eq!(
            state["billingPeriods"].as_array().unwrap().len(),
            2,
            "{label}: {state}"
        );
        assert_eq!(
            state["start"],
            s.time.payment_a().0.to_rfc3339(),
            "{label}: {state}"
        );
        assert_eq!(
            state["end"],
            s.time.payment_a().1.to_rfc3339(),
            "{label}: {state}"
        );
        // One allowance per paid period, each with the usage price's included amount.
        assert_eq!(
            state["grantList"].as_array().unwrap().len(),
            2,
            "{label}: {state}"
        );
        assert!(
            state["grantList"]
                .as_array()
                .unwrap()
                .iter()
                .all(|g| g.as_str().unwrap().ends_with("calls=100")),
            "{label}: {state}"
        );
        assert_eq!(s.authorize().await["allowed"], true, "{label}");

        // A replay under the same or a fresh event id for the same transaction adds nothing.
        s.f.worker.process_webhook("evt_renewal").await.unwrap();
        s.store_renewal("evt_renewal_again", s.renewal_data("txn_renewal", &job))
            .await;
        s.f.worker
            .process_webhook("evt_renewal_again")
            .await
            .unwrap();
        assert_eq!(s.converged().await, state, "{label}: replay changed state");
        finals.push(state);
    }
    assert_eq!(
        finals[0], finals[1],
        "final state must not depend on delivery order"
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn deferred_renewal_survives_restart_and_retry_exhaustion_until_operator_retry() {
    let s = scenario().await;
    let job = unassociate(&s).await;
    s.park_initial(&job).await;
    s.store_renewal("evt_renewal", s.renewal_data("txn_renewal", &job))
        .await;
    let webhook_job = Uuid::now_v7();
    sqlx::query("INSERT INTO jobs(id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1,'webhook','webhook:evt_renewal','{\"event_id\":\"evt_renewal\"}',now(),now(),now())")
        .bind(webhook_job).execute(&s.f.db).await.unwrap();
    let webhook_job = webhook_job.to_string();
    let limit = s.f.config.retry_limit as i32;
    for attempt in 1..=limit {
        sqlx::query("UPDATE jobs SET available_at=now() WHERE id=$1")
            .bind(Uuid::parse_str(&webhook_job).unwrap())
            .execute(&s.f.db)
            .await
            .unwrap();
        s.f.worker.step().await.unwrap();
        let (state, category, attempts) = s.job_row(&webhook_job).await;
        assert_eq!(attempts, attempt);
        if attempt < limit {
            assert_eq!(
                (state.as_str(), category.as_str()),
                ("pending", "provider_transient")
            );
        }
        // The notification is never acknowledged while the association is missing.
        assert!(!s.processed("evt_renewal").await);
        if attempt == 1 {
            // A process that stopped while running the job returns it to pending on restart.
            sqlx::query("UPDATE jobs SET state='running' WHERE id=$1")
                .bind(Uuid::parse_str(&webhook_job).unwrap())
                .execute(&s.f.db)
                .await
                .unwrap();
            s.f.worker.recover().await.unwrap();
            assert_eq!(s.job_row(&webhook_job).await.0, "pending");
        }
    }
    let (state, category, _) = s.job_row(&webhook_job).await;
    assert_eq!(
        (state.as_str(), category.as_str()),
        ("failed", "retry_exhausted")
    );
    assert!(!s.processed("evt_renewal").await);
    assert_eq!(s.count("provider_transactions").await, 0);

    // The initial payment arrives, then an operator returns the failed work to processing.
    s.pay_initial("evt_initial", &job).await;
    s.f.rpc(
        "owner",
        "RetryJob",
        json!({"requestKey":key(),"jobId":webhook_job,"reason":"initial payment associated"}),
    )
    .await;
    assert_eq!(
        s.job_row(&webhook_job).await,
        ("pending".into(), "retry_exhausted".into(), 0)
    );
    sqlx::query("UPDATE jobs SET available_at=now() WHERE id=$1")
        .bind(Uuid::parse_str(&webhook_job).unwrap())
        .execute(&s.f.db)
        .await
        .unwrap();
    s.f.worker.step().await.unwrap();
    assert_eq!(s.job_row(&webhook_job).await.0, "done");
    assert!(s.processed("evt_renewal").await);
    let state = s.state().await;
    assert_eq!(
        state["transactions"],
        json!(["txn_initial", "txn_renewal"]),
        "{state}"
    );
    assert_eq!(state["end"], s.time.payment_a().1.to_rfc3339(), "{state}");
    // This fixture has no usage allowance, so only the renewal reason is relevant.
    let auth = s.authorize().await;
    assert!(
        !auth["reasons"]
            .as_array()
            .is_some_and(|r| r.contains(&json!("BLOCK_REASON_RENEWAL"))),
        "{auth}"
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn renewal_with_unrelated_or_inconsistent_metadata_is_not_retained() {
    let s = scenario().await;
    let job = unassociate(&s).await;
    s.park_initial(&job).await;
    let foreign_job = Uuid::now_v7().to_string();
    let other_tenant = Uuid::now_v7().to_string();
    let mut cases: Vec<(&str, Value)> = Vec::new();
    let mut case = |label, edit: &dyn Fn(&mut Value)| {
        let mut data = s.renewal_data("txn_renewal", &job);
        edit(&mut data);
        cases.push((label, data));
    };
    case("no custom data", &|d| {
        d.as_object_mut().unwrap().remove("custom_data");
    });
    case("unknown job", &|d| {
        d["custom_data"]["delcore_job_id"] = json!(foreign_job)
    });
    case("job id is not a UUID", &|d| {
        d["custom_data"]["delcore_job_id"] = json!("job_1")
    });
    case("job id is not UUIDv7", &|d| {
        d["custom_data"]["delcore_job_id"] = json!("7b1f3c52-9d0e-4a6b-8c3d-1e2f3a4b5c6d")
    });
    case("other tenant", &|d| {
        d["custom_data"]["delcore_tenant_id"] = json!(other_tenant)
    });
    case("empty customer", &|d| d["customer_id"] = json!(""));
    case("other currency", &|d| d["currency_code"] = json!("EUR"));
    case("other price", &|d| {
        d["items"][0]["price"]["id"] = json!("pri_other")
    });
    case("quantity", &|d| d["items"][0]["quantity"] = json!(2));
    case("extra item", &|d| {
        let item = d["items"][0].clone();
        d["items"].as_array_mut().unwrap().push(item);
    });
    case("no billing period", &|d| {
        d.as_object_mut().unwrap().remove("billing_period");
    });
    for (n, (label, data)) in cases.into_iter().enumerate() {
        let event = format!("evt_case_{n}");
        s.store_renewal(&event, data).await;
        // Terminal no-op: acknowledged without any benefit.
        s.f.worker.process_webhook(&event).await.unwrap();
        assert!(s.processed(&event).await, "{label}");
        assert_eq!(s.count("provider_transactions").await, 0, "{label}");
        assert_eq!(s.count("billing_periods").await, 0, "{label}");
        assert_eq!(s.count("allowance_grants").await, 0, "{label}");
    }
    // A job that is finished or terminally failed is no longer waiting for an association, and a
    // job that saved the renewal's own transaction is not waiting for an initial payment.
    for (label, state, external) in [
        ("done job", "done", "txn_initial"),
        ("failed job", "failed", "txn_initial"),
        ("renewal transaction", "reconcile", "txn_renewal"),
    ] {
        sqlx::query("UPDATE jobs SET state=$2,external_id=$3 WHERE id=$1")
            .bind(Uuid::parse_str(&job).unwrap())
            .bind(state)
            .bind(external)
            .execute(&s.f.db)
            .await
            .unwrap();
        let event = format!("evt_job_{state}_{external}");
        s.store_renewal(&event, s.renewal_data("txn_renewal", &job))
            .await;
        s.f.worker.process_webhook(&event).await.unwrap();
        assert!(s.processed(&event).await, "{label}");
        assert_eq!(s.count("provider_transactions").await, 0, "{label}");
    }
    let tenant: (String, String) = sqlx::query_as(
        "SELECT external_subscription_id,external_customer_id FROM tenants WHERE id=$1::text::uuid",
    )
    .bind(&s.tenant)
    .fetch_one(&s.f.db)
    .await
    .unwrap();
    assert_eq!(tenant, (String::new(), String::new()));
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn deferred_renewal_for_another_customer_grants_nothing_after_association() {
    let s = scenario().await;
    let job = unassociate(&s).await;
    s.park_initial(&job).await;
    let mut data = s.renewal_data("txn_renewal", &job);
    data["customer_id"] = json!("ctm_other");
    s.store_renewal("evt_renewal", data).await;
    // The metadata is consistent with the pending job, so the renewal waits for the association.
    assert!(s.f.worker.process_webhook("evt_renewal").await.is_err());
    s.pay_initial("evt_initial", &job).await;
    // Ordinary renewal validation then rejects the wrong customer as a terminal no-op.
    s.f.worker.process_webhook("evt_renewal").await.unwrap();
    assert!(s.processed("evt_renewal").await);
    let state = s.state().await;
    assert_eq!(state["transactions"], json!(["txn_initial"]), "{state}");
    assert_eq!(state["end"], s.time.days(0).to_rfc3339(), "{state}");
    assert_eq!(
        state["billingPeriods"].as_array().unwrap().len(),
        1,
        "{state}"
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn deferred_renewal_does_not_resurrect_a_canceled_subscription() {
    let s = scenario().await;
    let job = unassociate(&s).await;
    s.park_initial(&job).await;
    s.store_renewal("evt_renewal", s.renewal_data("txn_renewal", &job))
        .await;
    assert!(s.f.worker.process_webhook("evt_renewal").await.is_err());
    // The cancellation occurred after the renewal and is retained before the association.
    s.status(
        "evt_canceled",
        "subscription.canceled",
        "canceled",
        s.time.days(5),
    )
    .await;
    s.pay_initial("evt_initial", &job).await;
    let free_plan: bool = sqlx::query_scalar(
        "SELECT p.free FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1::text::uuid",
    )
    .bind(&s.tenant)
    .fetch_one(&s.f.db)
    .await
    .unwrap();
    assert!(
        free_plan,
        "retained cancellation applies with the initial payment"
    );
    // The ended subscription has no tenant and its job is finished: the renewal is dropped.
    s.f.worker.process_webhook("evt_renewal").await.unwrap();
    assert!(s.processed("evt_renewal").await);
    let state = s.state().await;
    assert_eq!(state["transactions"], json!(["txn_initial"]), "{state}");
    assert_ne!(state["end"], s.time.payment_a().1.to_rfc3339(), "{state}");
    let paid: bool = sqlx::query_scalar(
        "SELECT p.free FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1::text::uuid",
    )
    .bind(&s.tenant)
    .fetch_one(&s.f.db)
    .await
    .unwrap();
    assert!(paid, "the tenant stays on the free plan");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn deferred_renewal_keeps_a_newer_failure_after_association() {
    let s = scenario().await;
    let job = unassociate(&s).await;
    s.park_initial(&job).await;
    s.store_renewal("evt_renewal", s.renewal_data("txn_renewal", &job))
        .await;
    assert!(s.f.worker.process_webhook("evt_renewal").await.is_err());
    s.past_due("evt_b").await;
    s.pay_initial("evt_initial", &job).await;
    assert!(s.flag().await);
    s.f.worker.process_webhook("evt_renewal").await.unwrap();
    let state = s.state().await;
    assert_eq!(
        state["transactions"],
        json!(["txn_initial", "txn_renewal"]),
        "{state}"
    );
    assert_eq!(state["end"], s.time.payment_a().1.to_rfc3339(), "{state}");
    assert_eq!(state["renewalFailed"], true, "{state}");
}
