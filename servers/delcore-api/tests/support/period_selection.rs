// SPDX-License-Identifier: FSL-1.1-ALv2
//! Billing-period selection must follow the subscription that applies at the requested instant:
//! a recorded Paddle period never overrides a free calendar selection after confirmed early
//! cancellation, and ingestion, feature authorization and aggregate queries share that choice.
use super::{Fixture, key};
use chrono::{DateTime, Datelike, Duration, Months, SubsecRound, TimeZone, Utc};
use delcore_api::postgres;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

const SUBSCRIPTION: &str = "sub_period_fixture";
const CUSTOMER: &str = "ctm_period_fixture";
const EXTERNAL_PRICE: &str = "pri_period_fixture";
const GROUP: &str = "eu";

type Period = (DateTime<Utc>, DateTime<Utc>);

fn calendar(at: DateTime<Utc>) -> Period {
    let start = Utc
        .with_ymd_and_hms(at.year(), at.month(), 1, 0, 0, 0)
        .single()
        .unwrap();
    (start, start + Months::new(1))
}

/// Provider occurrence times. The cancellation is early: it happens well before the recorded
/// paid period ends. Every event time stays inside the accepted lateness bound.
struct Timeline {
    now: DateTime<Utc>,
    paid_start: DateTime<Utc>,
    paid_end: DateTime<Utc>,
    canceled: DateTime<Utc>,
}
impl Timeline {
    fn now() -> Self {
        let now = Utc::now().trunc_subsecs(0);
        let canceled = now - Duration::hours(2);
        // The odd offset keeps the paid period unaligned with every UTC calendar month.
        let paid_start = canceled - Duration::days(20) - Duration::minutes(37);
        Self {
            now,
            paid_start,
            paid_end: paid_start + Duration::days(30),
            canceled,
        }
    }
    /// An occurrence before the cancellation that is still reported after it.
    fn before(&self, hours: i64) -> DateTime<Utc> {
        self.canceled - Duration::hours(hours)
    }
    fn after(&self, minutes: i64) -> DateTime<Utc> {
        self.canceled + Duration::minutes(minutes)
    }
    /// The free calendar month is ambiguous when it rolls over inside the fixture window.
    fn same_month(&self) -> bool {
        calendar(self.before(13)) == calendar(self.now)
    }
}

struct Scenario {
    f: Fixture,
    tenant: String,
    user: Value,
    service: String,
    free_price: String,
    job: String,
    time: Timeline,
}

async fn scenario() -> Scenario {
    let f = Fixture::setup().await;
    let service = f.service().await;
    f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"declare meter","meter":{"serviceId":service,"name":"usage","key":"usage","eventType":"request","aggregation":"METER_AGGREGATION_SUM","valueAttribute":"units","groupBy":["region"]}})).await;
    let feature = f.rpc("owner","PutFeature",json!({"requestKey":key(),"reason":"declare feature","feature":{"serviceId":service,"key":"limit","name":"Limit","kind":"FEATURE_KIND_METERED","meterKey":"usage"}})).await["feature"]["id"].clone();
    let free = f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"free plan","plan":{"key":"free","name":"free","free":true,"defaultFree":true}})).await["plan"]["id"].as_str().unwrap().to_owned();
    let free_price = f.rpc("owner","CreatePrice",json!({"requestKey":key(),"reason":"free allowance","price":{"key":"usage-free","planId":free,"serviceId":service,"meter":"usage","unitPrice":"1","included":"10","currency":"USD"}})).await["price"]["id"].as_str().unwrap().to_owned();
    f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"free limit","replaceFeatureValues":true,"plan":{"key":"free","name":"free","free":true,"defaultFree":true,"featureValues":[{"featureId":feature,"limit":"10"}]}})).await;
    let paid = f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"paid plan","plan":{"name":"pro","rank":10,"free":false}})).await["plan"]["id"].as_str().unwrap().to_owned();
    let price = f.rpc("owner","CreatePrice",json!({"requestKey":key(),"reason":"paid usage","price":{"key":"usage-pro","planId":paid,"serviceId":service,"meter":"usage","unitPrice":"1","included":"1000","currency":"USD"}})).await["price"]["id"].as_str().unwrap().to_owned();
    f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"paid limit","replaceFeatureValues":true,"plan":{"name":"pro","rank":10,"free":false,"featureValues":[{"featureId":feature,"limit":"1000"}]}})).await;
    let version: String = sqlx::query_scalar(
        "SELECT active_version_id::text FROM catalog_entities WHERE id=$1::text::uuid",
    )
    .bind(&paid)
    .fetch_one(&f.db)
    .await
    .unwrap();
    let user = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let tenant = user["tenants"][0]["id"].as_str().unwrap().to_owned();
    let job = key();
    let payload = json!({"plan_id":paid,"price_ids":[price],"version_ids":[version],"external_price_ids":[EXTERNAL_PRICE]});
    sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,state,attempts,available_at,created_at,updated_at) VALUES($1,$2,'subscription',$3,$4,'pending',0,now(),now(),now())")
        .bind(Uuid::parse_str(&job).unwrap()).bind(Uuid::parse_str(&tenant).unwrap()).bind(format!("fixture:{job}")).bind(payload).execute(&f.db).await.unwrap();
    Scenario {
        f,
        tenant,
        user,
        service,
        free_price,
        job,
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
    /// Confirms the paid subscription and records its Paddle period.
    async fn subscribe(&self) {
        let t = &self.time;
        let data = json!({"id":"txn_period_fixture","status":"completed","origin":"web","customer_id":CUSTOMER,"subscription_id":SUBSCRIPTION,"currency_code":"USD",
            "custom_data":{"delcore_job_id":self.job,"delcore_tenant_id":self.tenant},
            "items":[{"price":{"id":EXTERNAL_PRICE,"product_id":"pro_period_fixture"},"quantity":1}],
            "billing_period":{"starts_at":t.paid_start.to_rfc3339(),"ends_at":t.paid_end.to_rfc3339()},
            "billed_at":t.paid_start.to_rfc3339()});
        self.deliver("evt_payment", "transaction.completed", t.paid_start, data)
            .await;
        let recorded = self.recorded().await;
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert_eq!((recorded[0].0, recorded[0].1), (t.paid_start, t.paid_end));
    }
    /// A newer verified cancellation without a local cancel intent.
    async fn cancel(&self) {
        self.deliver(
            "evt_canceled",
            "subscription.updated",
            self.time.canceled,
            json!({"id":SUBSCRIPTION,"status":"canceled"}),
        )
        .await;
    }
    async fn recorded(&self) -> Vec<(DateTime<Utc>, DateTime<Utc>, bool)> {
        sqlx::query_as("SELECT starts_at,ends_at,postpaid FROM billing_periods WHERE tenant_id=$1::text::uuid ORDER BY starts_at")
            .bind(&self.tenant).fetch_all(&self.f.db).await.unwrap()
    }
    async fn ledger(&self) -> Vec<String> {
        sqlx::query("SELECT kind||':'||amount::text||':'||period_start::text||':'||source_key FROM ledger WHERE tenant_id=$1::text::uuid ORDER BY source_key,id")
            .bind(&self.tenant).fetch_all(&self.f.db).await.unwrap().iter().map(|r| r.get(0)).collect()
    }
    async fn resolve(&self, at: DateTime<Utc>) -> anyhow::Result<Period> {
        let mut conn = self.f.db.acquire().await?;
        postgres::billing_period(&mut conn, &self.tenant, at).await
    }
    async fn ingest(&self, event: &str, units: &str, at: DateTime<Utc>) {
        self.f.rpc("service","IngestEvent",json!({"requestKey":key(),"event":{"tenantId":self.tenant,"serviceId":self.service,"eventKey":event,"eventType":"request","occurredAt":at.to_rfc3339(),"attributes":{"region":{"text":GROUP},"units":{"number":units}}}})).await;
    }
    async fn ingest_error(&self, event: &str, at: DateTime<Utc>, code: &str) {
        self.f.error("service","IngestEvent",json!({"requestKey":key(),"event":{"tenantId":self.tenant,"serviceId":self.service,"eventKey":event,"eventType":"request","occurredAt":at.to_rfc3339(),"attributes":{"region":{"text":GROUP},"units":{"number":"1"}}}}),code).await;
    }
    async fn persisted(&self, event: &str) -> Period {
        sqlx::query_as("SELECT period_start,period_end FROM metering_events WHERE tenant_id=$1::text::uuid AND event_key=$2")
            .bind(&self.tenant).bind(event).fetch_one(&self.f.db).await.unwrap()
    }
    async fn authorize(&self) -> Value {
        self.f.rpc("service","Authorize",json!({"tenantId":self.tenant,"userId":self.user["userId"],"capability":"execute","featureKey":"limit","groupValues":{"region":{"text":GROUP}},"paid":false})).await
    }
    fn aggregates_request(&self, at: Option<DateTime<Utc>>) -> Value {
        let mut request = json!({"tenantId":self.tenant,"meterKey":"usage"});
        if let Some(at) = at {
            request["periodAt"] = json!(at.to_rfc3339());
        }
        request
    }
    async fn aggregates(&self, at: Option<DateTime<Utc>>) -> Vec<Value> {
        self.f
            .rpc(
                "service",
                "ListMeterAggregates",
                self.aggregates_request(at),
            )
            .await["aggregates"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
}

/// Inserts a recurring (kind 2) price owned by a new add-on, or by the free base plan.
async fn recurring_price(s: &Scenario, add_on: bool) -> Uuid {
    let price = Uuid::now_v7();
    let external = format!("pri_period_{price}");
    if add_on {
        let addon = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO addons(id,service_id,name) VALUES($1,$2::text::uuid,'period add-on')",
        )
        .bind(addon)
        .bind(&s.service)
        .execute(&s.f.db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO prices(id,addon_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,kind) VALUES($1,$2,$3::text::uuid,'usage',0,0,'USD',$4,now(),2)")
            .bind(price)
            .bind(addon)
            .bind(&s.service)
            .bind(external)
            .execute(&s.f.db)
            .await
            .unwrap();
    } else {
        sqlx::query("INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,kind) SELECT $1,plan_id,service_id,meter,0,0,currency,$3,now(),2 FROM prices WHERE id=$2::text::uuid")
            .bind(price)
            .bind(&s.free_price)
            .bind(external)
            .execute(&s.f.db)
            .await
            .unwrap();
    }
    price
}

fn unavailable(result: anyhow::Result<Period>) -> bool {
    result.is_err_and(|e| e.is::<postgres::PeriodUnavailable>())
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn early_cancellation_selects_calendar_month_without_touching_the_paid_period() {
    let s = scenario().await;
    let t = &s.time;
    s.subscribe().await;
    let paid = (t.paid_start, t.paid_end);
    // Before cancellation the recorded Paddle period applies, also exactly at the instant.
    for at in [t.before(24), t.canceled, t.after(30)] {
        assert_eq!(s.resolve(at).await.unwrap(), paid, "{at}");
    }
    let before = s.recorded().await;
    s.cancel().await;
    assert_eq!(
        s.recorded().await,
        before,
        "the historical paid period must stay unchanged"
    );
    // Historical instants keep the paid period; the cancellation instant and later are free.
    assert_eq!(s.resolve(t.before(24)).await.unwrap(), paid);
    assert_eq!(
        s.resolve(t.canceled - Duration::seconds(1)).await.unwrap(),
        paid
    );
    assert_eq!(s.resolve(t.canceled).await.unwrap(), calendar(t.canceled));
    assert_eq!(s.resolve(t.after(30)).await.unwrap(), calendar(t.after(30)));
    // Activation of the default free plan completed.
    let state: (String, bool) = sqlx::query_as("SELECT t.external_subscription_id,p.free FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1::text::uuid")
        .bind(&s.tenant).fetch_one(&s.f.db).await.unwrap();
    assert_eq!(state, (String::new(), true));
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn ingestion_authorization_and_queries_use_the_calendar_month_after_cancellation() {
    let s = scenario().await;
    let t = &s.time;
    if !t.same_month() {
        // The fixture window crosses a UTC month boundary, so the free grant month is ambiguous.
        return;
    }
    s.subscribe().await;
    // The recorded period is postpaid, so inheriting its policy would be observable.
    sqlx::query("UPDATE billing_periods SET postpaid=true WHERE tenant_id=$1::text::uuid")
        .bind(&s.tenant)
        .execute(&s.f.db)
        .await
        .unwrap();
    let paid = (t.paid_start, t.paid_end);
    s.ingest("paid-before", "20", t.before(12)).await;
    s.f.process_metering().await;
    s.cancel().await;
    let calendar_period = calendar(t.after(30));
    assert_eq!(s.persisted("paid-before").await, paid);
    let paid_ledger = s.ledger().await;
    let paid_rows = s.recorded().await;

    // A delayed event that occurred before the cancellation keeps its paid attribution.
    s.ingest("paid-late", "5", t.before(6)).await;
    assert_eq!(s.persisted("paid-late").await, paid);
    // New free usage persists the calendar bounds.
    s.ingest("free-1", "3", t.after(30)).await;
    assert_eq!(s.persisted("free-1").await, calendar_period);
    s.f.process_metering().await;

    // Metered authorization reads the calendar aggregate, not the 25 paid-period units.
    let result = s.authorize().await;
    assert_eq!(result["allowed"], true, "{result}");
    assert_eq!(result["feature"]["usage"], "3", "{result}");
    assert_eq!(result["feature"]["limit"], "10", "{result}");
    assert_eq!(
        result["feature"]["periodStart"],
        calendar_period.0.to_rfc3339().replace("+00:00", "Z"),
        "{result}"
    );
    assert_eq!(
        result["feature"]["periodEnd"],
        calendar_period.1.to_rfc3339().replace("+00:00", "Z"),
        "{result}"
    );

    // Aggregate queries select the matching period for each instant.
    let free = s.aggregates(Some(t.after(30))).await;
    assert_eq!(free.len(), 1, "{free:?}");
    assert_eq!(free[0]["value"], "3");
    let now_default = s.aggregates(None).await;
    assert_eq!(now_default.len(), 1, "{now_default:?}");
    assert_eq!(now_default[0]["value"], "3");
    let historical = s.aggregates(Some(t.before(6))).await;
    assert_eq!(historical.len(), 1, "{historical:?}");
    assert_eq!(historical[0]["value"], "25");

    // Paid rows and the postpaid policy snapshot stay intact; the calendar grant covers the
    // free units, so the old paid period's policy is never applied to them.
    assert_eq!(s.recorded().await, paid_rows);
    let mut conn = s.f.db.acquire().await.unwrap();
    assert!(
        !postgres::period_postpaid(&mut conn, &s.tenant, calendar_period.0)
            .await
            .unwrap()
    );
    assert!(
        postgres::period_postpaid(&mut conn, &s.tenant, t.paid_start)
            .await
            .unwrap()
    );
    let after = s.ledger().await;
    assert!(
        after.iter().all(|row| !row.contains("postpaid")),
        "{after:?}"
    );
    assert!(
        after
            .iter()
            .filter(|row| row.contains(&calendar_period.0.to_string()))
            .all(|row| row.starts_with("grant") || row.starts_with("credit")),
        "free units inside the calendar allowance must not be charged: {after:?}"
    );
    for row in paid_ledger {
        assert!(after.contains(&row), "paid ledger row changed: {row}");
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn free_base_with_active_recurring_add_on_keeps_recorded_paddle_coverage() {
    let s = scenario().await;
    let t = &s.time;
    // Prices are immutable, so the fixture inserts an add-on with a recurring price; it only
    // needs the pinned entitlement history, not the purchase flow.
    let recurring = recurring_price(&s, true).await;
    let tenant = Uuid::parse_str(&s.tenant).unwrap();
    let addon_start = t.paid_start - Duration::days(5);
    sqlx::query("DELETE FROM entitlements WHERE tenant_id=$1")
        .bind(tenant)
        .execute(&s.f.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO entitlements(id,tenant_id,price_id,starts_at,ends_at) VALUES($1,$2,$3,$4,$5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .bind(recurring)
    .bind(addon_start)
    .bind(t.canceled)
    .execute(&s.f.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO billing_periods(tenant_id,starts_at,ends_at,postpaid) VALUES($1,$2,$3,false)",
    )
    .bind(tenant)
    .bind(t.paid_start)
    .bind(t.paid_end)
    .execute(&s.f.db)
    .await
    .unwrap();
    let paid = (t.paid_start, t.paid_end);
    // While the add-on entitlement is active the recorded period applies, also if it started
    // before the period (the base plan is free throughout).
    assert_eq!(s.resolve(t.before(24)).await.unwrap(), paid);
    assert_eq!(
        s.resolve(t.canceled - Duration::seconds(1)).await.unwrap(),
        paid
    );
    // The entitlement end is exclusive: from that instant the calendar month applies.
    assert_eq!(s.resolve(t.canceled).await.unwrap(), calendar(t.canceled));
    assert_eq!(s.resolve(t.after(30)).await.unwrap(), calendar(t.after(30)));
    // An active add-on without recorded coverage must not fall back to a calendar month.
    assert!(unavailable(
        s.resolve(addon_start + Duration::days(1)).await
    ));
    // Without an add-on entitlement a free base is calendar-based even before the add-on.
    assert_eq!(
        s.resolve(addon_start - Duration::days(1)).await.unwrap(),
        calendar(addon_start - Duration::days(1))
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn recurring_price_of_the_free_base_plan_keeps_the_calendar_month() {
    let s = scenario().await;
    let t = &s.time;
    // Tenant creation and cancellation activation grant every active base-plan price as an
    // entitlement, so a recurring price owned by the free base plan is not an add-on and
    // requires no Paddle coverage.
    let recurring = recurring_price(&s, false).await;
    let tenant = Uuid::parse_str(&s.tenant).unwrap();
    let start = t.paid_start - Duration::days(5);
    sqlx::query("INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2,$3,$4)")
        .bind(Uuid::now_v7())
        .bind(tenant)
        .bind(recurring)
        .bind(start)
        .execute(&s.f.db)
        .await
        .unwrap();
    for at in [start + Duration::days(1), t.canceled, t.after(30)] {
        assert_eq!(s.resolve(at).await.unwrap(), calendar(at), "{at}");
    }
    s.ingest("free-base-recurring", "1", t.after(30)).await;
    assert_eq!(
        s.persisted("free-base-recurring").await,
        calendar(t.after(30))
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn recurring_entitlement_lookup_uses_the_tenant_window_index() {
    let s = scenario().await;
    let mut tx = s.f.db.begin().await.unwrap();
    // The planner may prefer a sequential scan on an almost empty table; forbid it so the plan
    // proves that the lookup can use an index on the tenant and validity window.
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan: Vec<String> = sqlx::query_scalar("EXPLAIN SELECT EXISTS(SELECT 1 FROM entitlements en JOIN prices pr ON pr.id=en.price_id WHERE en.tenant_id=$1 AND pr.addon_id IS NOT NULL AND pr.kind=2 AND en.starts_at<=$2 AND (en.ends_at IS NULL OR en.ends_at>$2))")
        .bind(Uuid::parse_str(&s.tenant).unwrap())
        .bind(s.time.now)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    // Release the transaction before asserting: fixture cleanup drops the schema and would
    // otherwise wait for it when the assertion fails.
    tx.rollback().await.unwrap();
    let plan = plan.join("\n");
    assert!(plan.contains("entitlements_tenant_window"), "{plan}");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn missing_paid_coverage_keeps_the_unavailable_period_failure_in_every_consumer() {
    let s = scenario().await;
    let t = &s.time;
    s.subscribe().await;
    assert!(unavailable(s.resolve(t.paid_end + Duration::days(1)).await));
    // The paid base remains selected and the recorded coverage disappears: no calendar fallback.
    sqlx::query("DELETE FROM billing_periods WHERE tenant_id=$1::text::uuid")
        .bind(&s.tenant)
        .execute(&s.f.db)
        .await
        .unwrap();
    assert!(unavailable(s.resolve(t.before(1)).await));
    s.ingest_error("no-coverage", t.before(1), "failed_precondition")
        .await;
    let denied = s.authorize().await;
    assert_ne!(denied["allowed"], true, "{denied}");
    assert!(
        denied["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("BLOCK_REASON_RENEWAL")),
        "{denied}"
    );
    s.f.error(
        "service",
        "ListMeterAggregates",
        s.aggregates_request(Some(t.before(1))),
        "invalid_argument",
    )
    .await;
}
