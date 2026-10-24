// SPDX-License-Identifier: FSL-1.1-ALv2
//! Missed free-month baseline recovery. Clocks are explicit: the target tenant holds its free
//! price since three months before the current month, and no maintenance pass granted the
//! previous month.
use super::{Fixture, key};
use chrono::{DateTime, Datelike, Duration, Months, TimeZone, Utc};
use delcore_api::{
    events,
    metering::{Limits, Value},
    postgres::{self, recovery},
    worker::Worker,
};
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

struct World {
    f: Fixture,
    tenant: String,
    service: String,
    plan: String,
    /// Start of the current UTC month.
    cur: DateTime<Utc>,
    /// Start of the previous UTC month, the wholly skipped period.
    prev: DateTime<Utc>,
}

fn month_start(at: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(at.year(), at.month(), 1, 0, 0, 0)
        .unwrap()
}

impl World {
    async fn new() -> Self {
        let f = Fixture::setup().await;
        f.bootstrap().await;
        let service = f.service().await;
        let catalog = f.rpc("owner", "GetCatalog", json!({})).await;
        let plan = catalog["plans"][0]["id"].as_str().unwrap().to_owned();
        f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"declare meter","meter":{"serviceId":service,"key":"requests","name":"requests","eventType":"request","aggregation":"METER_AGGREGATION_SUM","valueAttribute":"quantity"}})).await;
        let cur = month_start(Utc::now());
        let mut w = Self {
            f,
            tenant: String::new(),
            service,
            plan,
            cur,
            prev: cur - Months::new(1),
        };
        w.price("100").await;
        w.tenant = w.tenant("Target", true).await;
        w
    }
    async fn price(&self, included: &str) -> String {
        let response = self.f.rpc("owner", "CreatePrice", json!({"requestKey":key(),"reason":"publish price","price":{"key":"requests-usd","planId":self.plan,"serviceId":self.service,"meter":"requests","unitPrice":"1","included":included,"currency":"USD"}})).await;
        sqlx::query("INSERT INTO catalog_price_activations(price_id,version_id,effective_at) SELECT entity_id,id,$2 FROM catalog_versions WHERE id=$1::text::uuid ON CONFLICT DO NOTHING")
            .bind(response["price"]["id"].as_str().unwrap()).bind(self.prev - Months::new(3)).execute(&self.f.db).await.unwrap();
        response["price"]["id"].as_str().unwrap().to_owned()
    }
    /// Creates a tenant holding the active price. A held tenant is backdated to three months before
    /// the current month and credited with 1000 purchased units; its current-month grant stays.
    async fn tenant(&self, name: &str, held: bool) -> String {
        let created = self
            .f
            .rpc(
                "owner",
                "CreateTenant",
                json!({"requestKey":key(),"name":name}),
            )
            .await;
        let tenant = created["tenant"]["id"].as_str().unwrap().to_owned();
        if held {
            let back = self.cur - Months::new(3);
            for table in ["tenant_catalog_selections", "entitlements"] {
                sqlx::query(&format!(
                    "UPDATE {table} SET starts_at=$2 WHERE tenant_id=$1::text::uuid"
                ))
                .bind(&tenant)
                .bind(back)
                .execute(&self.f.db)
                .await
                .unwrap();
            }
            sqlx::query("INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2::text::uuid,'credit',1000,$3,$4)")
                .bind(Uuid::now_v7()).bind(&tenant).bind(format!("seed:{tenant}")).bind(back).execute(&self.f.db).await.unwrap();
        }
        tenant
    }
    fn policy(&self) -> events::Policy {
        events::Policy {
            lateness: Duration::seconds(self.f.config.usage_lateness_seconds),
            reporting_grace: Duration::seconds(self.f.config.reporting_grace_seconds),
            limits: Limits {
                max_groups_per_meter: self.f.config.max_meter_groups,
                max_distinct_values_per_group: self.f.config.max_meter_group_values,
            },
        }
    }
    /// Accepts an event occurring at `occurred`, as if the request arrived at `now`.
    async fn ingest(
        &self,
        event: &str,
        quantity: &str,
        occurred: DateTime<Utc>,
        now: DateTime<Utc>,
    ) {
        let mut attributes = BTreeMap::new();
        attributes.insert("quantity".to_owned(), Value::number(quantity).unwrap());
        let input = events::Input {
            tenant_id: self.tenant.clone(),
            service_id: self.service.clone(),
            key: event.to_owned(),
            event_type: "request".into(),
            occurred_at: occurred,
            attributes,
        };
        let mut tx = self.f.db.begin().await.unwrap();
        events::ingest(&mut tx, now, &self.policy(), input)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn balance_is(&self, expected: &str) -> bool {
        sqlx::query_scalar("SELECT COALESCE(sum(amount),0)=$2::numeric FROM ledger WHERE tenant_id=$1::text::uuid AND kind IN ('credit','usage','refund','correction')")
            .bind(&self.tenant).bind(expected).fetch_one(&self.f.db).await.unwrap()
    }
    /// Grants of one period: (source key, amount, applied at).
    async fn grants(&self, start: DateTime<Utc>) -> Vec<(String, String, DateTime<Utc>)> {
        sqlx::query_as("SELECT source_key,amount::text,applied_at FROM allowance_grants WHERE tenant_id=$1::text::uuid AND period_start=$2 ORDER BY source_key")
            .bind(&self.tenant).bind(start).fetch_all(&self.f.db).await.unwrap()
    }
    /// Ledger entries of the period: (kind, amount, source key prefix).
    async fn ledger(&self, start: DateTime<Utc>) -> Vec<(String, String)> {
        sqlx::query_as("SELECT kind,amount::text FROM ledger WHERE tenant_id=$1::text::uuid AND period_start=$2 ORDER BY created_at,id")
            .bind(&self.tenant).bind(start).fetch_all(&self.f.db).await.unwrap()
    }

    async fn audits(&self, action: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM audit WHERE tenant_id=$1::text::uuid AND action=$2",
        )
        .bind(&self.tenant)
        .bind(action)
        .fetch_one(&self.f.db)
        .await
        .unwrap()
    }
    fn prev_key(&self) -> String {
        format!(
            "free:{}:{}:{}:requests",
            self.tenant,
            self.prev.format("%Y-%m"),
            self.service
        )
    }
    fn restarted(&self) -> Worker {
        Worker::new(self.f.db.clone(), self.f.config.clone()).unwrap()
    }
    async fn recover(
        &self,
        tenant: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> anyhow::Result<recovery::Recovery> {
        let mut conn = self.f.db.acquire().await?;
        recovery::recover_free_period(&mut conn, tenant, start, end).await
    }
}

/// Quantity 50 against an included 100 bills nothing, whether the event was accepted in the
/// skipped month and processed in the current one or first delivered within the lateness window.
#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn skipped_month_usage_uses_a_recovered_baseline_instead_of_purchased_credit() {
    for delivered_late in [false, true] {
        let w = World::new().await;
        let (occurred, now) = if delivered_late {
            (w.cur - Duration::days(2), w.cur + Duration::days(2))
        } else {
            (w.prev + Duration::days(14), w.prev + Duration::days(14))
        };
        w.ingest("february", "50", occurred, now).await;
        assert!(w.grants(w.prev).await.is_empty());
        w.f.process_metering().await;

        let grants = w.grants(w.prev).await;
        assert_eq!(grants.len(), 1, "late={delivered_late}");
        assert_eq!(grants[0].0, w.prev_key());
        assert_eq!(grants[0].1, "100");
        assert_eq!(grants[0].2, w.prev, "applied at the month start");
        assert!(w.balance_is("1000").await);
        assert!(w.ledger(w.prev).await.is_empty(), "nothing was charged");
        // The current month keeps its own allowance; it never covered the skipped month.
        let current = w.grants(w.cur).await;
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].1, "100");
        assert_ne!(current[0].2, w.prev);

        // Repeats, a fresh worker and ordinary maintenance add nothing.
        w.f.process_metering().await;
        w.restarted().maintain(Utc::now()).await.unwrap();
        assert_eq!(w.grants(w.prev).await.len(), 1);
        assert_eq!(w.grants(w.cur).await.len(), 1);
        assert!(w.balance_is("1000").await);
    }
}

/// Several events share one recovered allowance; a month whose grant was committed earlier is
/// left untouched and rates the same way.
#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn events_of_a_skipped_month_share_one_allowance_with_or_without_a_committed_grant() {
    for committed in [false, true] {
        let w = World::new().await;
        if committed {
            let mut conn = w.f.db.acquire().await.unwrap();
            postgres::grant_allowances_held_at(
                &mut conn,
                &w.tenant,
                w.prev,
                w.cur,
                w.prev,
                &format!("free:{}:{}", w.tenant, w.prev.format("%Y-%m")),
            )
            .await
            .unwrap();
        }
        for (index, quantity) in ["50", "50", "50"].iter().enumerate() {
            let at = w.prev + Duration::days(3 + index as i64);
            w.ingest(
                &format!("e{index}"),
                quantity,
                at,
                w.cur + Duration::days(1),
            )
            .await;
        }
        w.f.process_metering().await;
        let grants = w.grants(w.prev).await;
        assert_eq!(grants.len(), 1, "committed={committed}");
        assert_eq!(grants[0].1, "100");
        // 150 used against one shared 100 allowance charges 50.
        assert!(w.balance_is("950").await, "committed={committed}");
        let charged: String = sqlx::query_scalar("SELECT (-COALESCE(sum(amount),0))::text FROM ledger WHERE tenant_id=$1::text::uuid AND kind IN ('usage','correction')")
            .bind(&w.tenant).fetch_one(&w.f.db).await.unwrap();
        assert_eq!(charged.parse::<f64>().unwrap(), 50.0);
    }
}

/// The baseline is what was held at the month boundary: neither a later catalog price, nor the
/// plan the tenant holds now, nor a mid-month change replaces it.
#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn the_recovered_baseline_follows_the_entitlements_held_at_the_month_boundary() {
    let w = World::new().await;
    // A later publication does not touch the tenant's selection, and a mid-month change moves the
    // tenant to a larger allowance that the previous month's boundary never held.
    let raised = w.price("300").await;
    sqlx::query(
        "UPDATE entitlements SET ends_at=$2 WHERE tenant_id=$1::text::uuid AND ends_at IS NULL",
    )
    .bind(&w.tenant)
    .bind(w.prev + Duration::days(10))
    .execute(&w.f.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2::text::uuid,$3::text::uuid,$4)")
        .bind(Uuid::now_v7()).bind(&w.tenant).bind(&raised).bind(w.prev + Duration::days(10)).execute(&w.f.db).await.unwrap();
    w.ingest(
        "late",
        "150",
        w.prev + Duration::days(20),
        w.cur + Duration::days(1),
    )
    .await;
    w.f.process_metering().await;
    let grants = w.grants(w.prev).await;
    assert_eq!(grants.len(), 1);
    assert_eq!(
        grants[0].1, "100",
        "the boundary held 100, not the current 300"
    );
    assert!(w.balance_is("950").await);
}

/// Nothing is minted for a meter added later, a tenant that did not exist yet, a month that starts
/// with paid coverage, or a period that is not a calendar month.
#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn recovery_never_fabricates_a_baseline() {
    let w = World::new().await;
    // A second meter and price first held in the middle of the skipped month.
    w.f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"declare meter","meter":{"serviceId":w.service,"key":"extra","name":"extra","eventType":"extra","aggregation":"METER_AGGREGATION_COUNT"}})).await;
    let extra = w.f.rpc("owner","CreatePrice",json!({"requestKey":key(),"reason":"publish price","price":{"key":"extra-usd","planId":w.plan,"serviceId":w.service,"meter":"extra","unitPrice":"1","included":"40","currency":"USD"}})).await;
    sqlx::query("INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2::text::uuid,$3::text::uuid,$4)")
        .bind(Uuid::now_v7()).bind(&w.tenant).bind(extra["price"]["id"].as_str().unwrap()).bind(w.prev + Duration::days(10)).execute(&w.f.db).await.unwrap();
    assert_eq!(
        w.recover(&w.tenant, w.prev, w.cur).await.unwrap(),
        recovery::Recovery::Granted(1)
    );
    let grants = w.grants(w.prev).await;
    assert_eq!(grants.len(), 1, "{grants:?}");
    assert_eq!(grants[0].0, w.prev_key());
    // A repeat adds nothing.
    assert_eq!(
        w.recover(&w.tenant, w.prev, w.cur).await.unwrap(),
        recovery::Recovery::Skipped(recovery::Skip::AlreadyInitialized)
    );

    // Created after the boundary: its entitlements start now.
    let late = w.tenant("Late", false).await;
    assert_eq!(
        w.recover(&late, w.prev, w.cur).await.unwrap(),
        recovery::Recovery::Skipped(recovery::Skip::NotFree)
    );
    let none: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM allowance_grants WHERE tenant_id=$1::text::uuid AND period_start=$2",
    )
    .bind(&late)
    .bind(w.prev)
    .fetch_one(&w.f.db)
    .await
    .unwrap();
    assert_eq!(none, 0);

    // A paid period recorded for the month start owns the key; the free period starts later.
    let paid = w.tenant("Paid", true).await;
    sqlx::query("INSERT INTO billing_periods VALUES($1::text::uuid,$2,$3,false)")
        .bind(&paid)
        .bind(w.prev)
        .bind(w.cur)
        .execute(&w.f.db)
        .await
        .unwrap();
    assert_eq!(
        w.recover(&paid, w.prev, w.cur).await.unwrap(),
        recovery::Recovery::Skipped(recovery::Skip::PaidPeriodCollision)
    );

    // Periods that are not whole calendar months belong to creation, cancellation and refresh.
    let held = w.tenant("Held", true).await;
    assert_eq!(
        w.recover(&held, w.prev + Duration::microseconds(1), w.cur)
            .await
            .unwrap(),
        recovery::Recovery::Skipped(recovery::Skip::NotCalendarMonth)
    );
    assert_eq!(
        w.recover(&held, w.prev, w.cur + Duration::days(1))
            .await
            .unwrap(),
        recovery::Recovery::Skipped(recovery::Skip::NotCalendarMonth)
    );
}

/// Contradictory retained history is reported and grants nothing: the metering work fails
/// permanently with the existing failure audit.
#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn malformed_history_is_reported_and_never_guessed() {
    let w = World::new().await;
    w.ingest(
        "february",
        "50",
        w.prev + Duration::days(14),
        w.cur + Duration::days(1),
    )
    .await;
    // A second plan selection that overlaps the month start.
    sqlx::query("INSERT INTO tenant_catalog_selections(id,tenant_id,product_id,version_id,starts_at,ends_at) SELECT $2,cs.tenant_id,cs.product_id,cs.version_id,cs.starts_at,$3 FROM tenant_catalog_selections cs JOIN catalog_entities ce ON ce.id=cs.product_id WHERE cs.tenant_id=$1::text::uuid AND ce.kind='plan' AND cs.ends_at IS NULL")
            .bind(&w.tenant).bind(Uuid::now_v7()).bind(w.prev + Duration::days(1)).execute(&w.f.db).await.unwrap();

    w.f.worker.process_metering(10).await.unwrap();
    let (state, category): (String, String) =
        sqlx::query_as("SELECT state,error_category FROM jobs WHERE kind='metering'")
            .fetch_one(&w.f.db)
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), category.as_str()),
        ("failed", "metering_invalid")
    );
    assert_eq!(w.audits("metering.failed").await, 1);
    assert!(w.balance_is("1000").await, "no silent charge either");
    assert!(w.grants(w.prev).await.is_empty());
}

/// The paid-entitlement check reads the plan policy pinned for the tenant at the month boundary.
/// A free plan that held a recurring base-plan price and was later republished as paid still
/// grants its skipped free month, while a paid plan entitlement with no pinned free selection at
/// the boundary remains inconsistent history.
#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn held_entitlements_use_the_plan_policy_pinned_at_the_month_boundary() {
    let w = World::new().await;
    let back = w.cur - Months::new(3);
    // Tenant creation needs the default free plan, so create the second tenant first.
    let other = w.tenant("Other", true).await;
    let recurring: Uuid = sqlx::query_scalar("INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,kind) SELECT $1,plan_id,service_id,meter,0,0,currency,$2,now(),2 FROM prices WHERE plan_id=$3::text::uuid AND kind<>2 LIMIT 1 RETURNING id")
        .bind(Uuid::now_v7()).bind(format!("pri_recovery_{}", Uuid::now_v7())).bind(&w.plan).fetch_one(&w.f.db).await.unwrap();
    sqlx::query(
        "INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2::text::uuid,$3,$4)",
    )
    .bind(Uuid::now_v7())
    .bind(&w.tenant)
    .bind(recurring)
    .bind(back)
    .execute(&w.f.db)
    .await
    .unwrap();
    // The operator later publishes the plan as paid; the tenant's pinned selection keeps the
    // free version that applied at the boundary.
    sqlx::query("UPDATE plans SET free=false,is_default=false WHERE id=$1::text::uuid")
        .bind(&w.plan)
        .execute(&w.f.db)
        .await
        .unwrap();
    assert_eq!(
        w.recover(&w.tenant, w.prev, w.cur).await.unwrap(),
        recovery::Recovery::Granted(1)
    );
    assert_eq!(w.grants(w.prev).await.len(), 1);

    // A paid plan entitlement without any pinned selection of that plan is still contradictory.
    let paid_plan = w.f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"publish paid plan","plan":{"key":"paid","name":"paid","rank":2,"free":false}})).await["plan"]["id"].as_str().unwrap().to_owned();
    let paid_price: Uuid = sqlx::query_scalar("INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,kind) VALUES($1,$2::text::uuid,$3::text::uuid,'requests',0,0,'USD',$4,now(),2) RETURNING id")
        .bind(Uuid::now_v7()).bind(&paid_plan).bind(&w.service).bind(format!("pri_recovery_{}", Uuid::now_v7())).fetch_one(&w.f.db).await.unwrap();
    sqlx::query(
        "INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2::text::uuid,$3,$4)",
    )
    .bind(Uuid::now_v7())
    .bind(&other)
    .bind(paid_price)
    .bind(back)
    .execute(&w.f.db)
    .await
    .unwrap();
    let error = w.recover(&other, w.prev, w.cur).await.unwrap_err();
    assert!(
        error.downcast_ref::<recovery::HistoryInvalid>().is_some(),
        "{error}"
    );
}
