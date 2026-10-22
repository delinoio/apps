// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Fixture, key};
use chrono::Utc;
use serde_json::{Value, json};

async fn put(f: &Fixture, method: &str, field: &str, value: Value) -> Value {
    let mut request = json!({"requestKey":key(),"reason":"integration parity fixture"});
    request[field] = value;
    if method == "PutPlan" {
        request["replaceFeatureValues"] = json!(true);
    }
    f.rpc("owner", method, request).await[field].clone()
}
async fn service(f: &Fixture) -> Value {
    put(f,"PutService","service",json!({"key":"metered","name":"Metered","clientId":"service","capabilities":["execute"],"scopes":["authorize","usage:write","usage:read"]})).await
}
async fn plan(f: &Fixture, features: Value) -> Value {
    put(
        f,
        "PutPlan",
        "plan",
        json!({"key":"free","name":"Free","free":true,"defaultFree":true,"featureValues":features}),
    )
    .await
}
async fn aggregate(f: &Fixture, tenant: &Value, meter: &str) -> Vec<Value> {
    f.rpc(
        "service",
        "ListMeterAggregates",
        json!({"tenantId":tenant,"meterKey":meter}),
    )
    .await["aggregates"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}
fn attrs(region: &str, amount: &str, latency: &str, user: &str) -> Value {
    json!({"region":{"text":region},"bytes":{"number":amount},"latency":{"number":latency},"user":{"text":user}})
}
async fn ingest(
    f: &Fixture,
    tenant: &Value,
    service: &Value,
    event: &str,
    attributes: Value,
    occurred: &str,
) -> Value {
    f.rpc("service","IngestEvent",json!({"requestKey":key(),"event":{"tenantId":tenant,"serviceId":service,"eventKey":event,"eventType":"request","occurredAt":occurred,"attributes":attributes}})).await
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn event_replay_corrections_and_all_aggregations() {
    let f = Fixture::setup().await;
    plan(&f, json!([])).await;
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let tenant = &me["tenants"][0]["id"];
    let svc = service(&f).await;
    for (name, aggregation, attribute, grouped) in [
        ("requests", "COUNT", "", true),
        ("bytes", "SUM", "bytes", true),
        ("minimum", "MIN", "latency", false),
        ("maximum", "MAX", "latency", false),
        ("average", "AVG", "latency", false),
        ("users", "UNIQUE_COUNT", "user", true),
    ] {
        put(&f,"PutMeter","meter",json!({"serviceId":svc["id"],"key":name,"name":name,"eventType":"request","aggregation":format!("METER_AGGREGATION_{aggregation}"),"valueAttribute":attribute,"groupBy":if grouped {vec!["region"]}else{vec![]}})).await;
    }
    let occurred = Utc::now().to_rfc3339();
    let mut invalid = attrs("eu", "10", "2", "a");
    invalid.as_object_mut().unwrap().remove("latency");
    f.error("service","IngestEvent",json!({"requestKey":key(),"event":{"tenantId":tenant,"serviceId":svc["id"],"eventKey":"invalid","eventType":"request","occurredAt":occurred,"attributes":invalid}}),"invalid_argument").await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM metering_events")
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(count, 0, "invalid event must not leave partial effects");
    let first = ingest(
        &f,
        tenant,
        &svc["id"],
        "a",
        attrs("eu", "10.50", "-10.1", "u1"),
        &occurred,
    )
    .await;
    let replay = ingest(
        &f,
        tenant,
        &svc["id"],
        "a",
        attrs("eu", "10.5", "-10.1", "u1"),
        &occurred,
    )
    .await;
    assert_eq!(first["eventId"], replay["eventId"]);
    assert_eq!(first["revision"], 1);
    f.error("service","IngestEvent",json!({"requestKey":key(),"event":{"tenantId":tenant,"serviceId":svc["id"],"eventKey":"a","eventType":"request","occurredAt":occurred,"attributes":attrs("eu","11","-10.1","u1")}}),"already_exists").await;
    ingest(
        &f,
        tenant,
        &svc["id"],
        "b",
        attrs("eu", "20", "0.2", "u1"),
        &occurred,
    )
    .await;
    ingest(
        &f,
        tenant,
        &svc["id"],
        "c",
        attrs("us", "0", "0.3", "u2"),
        &occurred,
    )
    .await;
    assert!(aggregate(&f, tenant, "requests").await.is_empty());
    f.process_metering().await;
    for (meter, value) in [
        ("minimum", "-10.1"),
        ("maximum", "0.3"),
        ("average", "-3.2"),
    ] {
        assert_eq!(aggregate(&f, tenant, meter).await[0]["value"], value);
    }
    let bytes = aggregate(&f, tenant, "bytes").await;
    assert_eq!(bytes.len(), 2);
    assert_eq!(
        bytes
            .iter()
            .find(|v| v["groupValues"]["region"]["text"] == "eu")
            .unwrap()["value"],
        "30.5"
    );
    let users = aggregate(&f, tenant, "users").await;
    assert!(users.iter().all(|v| v["value"] == "1"));
    let replacement = json!({"requestKey":key(),"tenantId":tenant,"eventKey":"b","expectedRevision":1,"kind":"EVENT_CORRECTION_KIND_REPLACE","eventType":"request","attributes":attrs("us","2","4","u3")});
    let changed = f.rpc("service", "CorrectEvent", replacement.clone()).await;
    assert_eq!(changed["revision"], 2);
    let again = f.rpc("service", "CorrectEvent", replacement).await;
    assert_eq!(again["revision"], 2);
    f.error("service","CorrectEvent",json!({"requestKey":key(),"tenantId":tenant,"eventKey":"b","expectedRevision":1,"kind":"EVENT_CORRECTION_KIND_CANCEL"}),"aborted").await;
    f.rpc("service","CorrectEvent",json!({"requestKey":key(),"tenantId":tenant,"eventKey":"a","expectedRevision":1,"kind":"EVENT_CORRECTION_KIND_CANCEL"})).await;
    f.process_metering().await;
    let bytes = aggregate(&f, tenant, "bytes").await;
    assert_eq!(
        bytes.len(),
        1,
        "old group must disappear after correction and cancellation"
    );
    assert_eq!(bytes[0]["value"], "2");
    assert_eq!(aggregate(&f, tenant, "average").await[0]["value"], "2.15");
    let event = f
        .rpc(
            "service",
            "GetEvent",
            json!({"tenantId":tenant,"eventKey":"a"}),
        )
        .await;
    assert_eq!(event["cancelled"], true);
    assert_eq!(event["appliedRevision"], 2);
    f.error("service","CorrectEvent",json!({"requestKey":key(),"tenantId":tenant,"eventKey":"a","expectedRevision":2,"kind":"EVENT_CORRECTION_KIND_CANCEL"}),"failed_precondition").await;
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn feature_versions_limits_groups_and_permission() {
    let f = Fixture::setup().await;
    plan(&f, json!([])).await;
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let svc = service(&f).await;
    put(&f,"PutMeter","meter",json!({"serviceId":svc["id"],"key":"requests","name":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT","groupBy":["region"]})).await;
    let access = put(
        &f,
        "PutFeature",
        "feature",
        json!({"serviceId":svc["id"],"key":"api","name":"API","kind":"FEATURE_KIND_ACCESS"}),
    )
    .await;
    let metered = put(&f,"PutFeature","feature",json!({"serviceId":svc["id"],"key":"limit","name":"Limit","kind":"FEATURE_KIND_METERED","meterKey":"requests"})).await;
    let config = put(&f,"PutFeature","feature",json!({"serviceId":svc["id"],"key":"region","name":"Region","kind":"FEATURE_KIND_CONFIGURATION","valueType":"CONFIGURATION_VALUE_TYPE_STRING"})).await;
    let values = json!([{"featureId":access["id"],"enabled":true},{"featureId":metered["id"],"limit":"2"},{"featureId":config["id"],"configuration":{"text":"eu"}}]);
    let v1 = plan(&f, values.clone()).await;
    let alpha = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Alpha"}),
        )
        .await;
    let tenant = &alpha["tenant"]["id"];
    let mut v2values = values.clone();
    v2values[1]["limit"] = json!("5");
    let v2 = plan(&f, v2values).await;
    assert_ne!(v1["activeVersionId"], v2["activeVersionId"]);
    let beta = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Beta"}),
        )
        .await;
    let auth = |t: &Value, feature: &str, groups: Value| json!({"tenantId":t,"userId":me["userId"],"capability":"execute","featureKey":feature,"groupValues":groups});
    let group = json!({"region":{"text":"eu"}});
    for (t, limit) in [(tenant, "2"), (&beta["tenant"]["id"], "5")] {
        let result = f
            .rpc("service", "Authorize", auth(t, "limit", group.clone()))
            .await;
        assert_eq!(result["allowed"], true);
        assert_eq!(result["feature"]["limit"], limit);
        assert_eq!(result["feature"]["usage"], "0");
    }
    let old = f
        .rpc(
            "service",
            "Authorize",
            auth(&me["tenants"][0]["id"], "api", json!({})),
        )
        .await;
    assert_ne!(
        old["allowed"], true,
        "old tenant keeps its featureless version"
    );
    let result = f
        .rpc("service", "Authorize", auth(tenant, "region", json!({})))
        .await;
    assert_eq!(result["feature"]["configuration"]["text"], "eu");
    f.error(
        "service",
        "Authorize",
        auth(tenant, "limit", json!({})),
        "invalid_argument",
    )
    .await;
    f.error(
        "service",
        "Authorize",
        auth(tenant, "api", group.clone()),
        "invalid_argument",
    )
    .await;
    let occurred = Utc::now().to_rfc3339();
    for event in ["a", "b"] {
        ingest(&f, tenant, &svc["id"], event, group.clone(), &occurred).await;
    }
    let pending = f
        .rpc("service", "Authorize", auth(tenant, "limit", group.clone()))
        .await;
    assert_eq!(pending["feature"]["usage"], "0");
    f.process_metering().await;
    let full = f
        .rpc("service", "Authorize", auth(tenant, "limit", group.clone()))
        .await;
    assert_ne!(full["allowed"], true);
    assert_eq!(full["feature"]["usage"], "2");
    assert!(
        full["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("BLOCK_REASON_FEATURE_LIMIT_REACHED"))
    );
    let independent = f
        .rpc(
            "service",
            "Authorize",
            auth(tenant, "limit", json!({"region":{"text":"us"}})),
        )
        .await;
    assert_eq!(independent["allowed"], true);
    let mut denied = auth(tenant, "api", json!({}));
    denied["userId"] = json!(key());
    let denied = f.rpc("service", "Authorize", denied).await;
    assert_ne!(denied["allowed"], true);
    assert!(denied["feature"].is_null());
    f.rpc("service","CorrectEvent",json!({"requestKey":key(),"tenantId":tenant,"eventKey":"a","expectedRevision":1,"kind":"EVENT_CORRECTION_KIND_CANCEL"})).await;
    f.process_metering().await;
    let recovered = f
        .rpc("service", "Authorize", auth(tenant, "limit", group))
        .await;
    assert_eq!(recovered["allowed"], true);
    assert_eq!(recovered["feature"]["usage"], "1");
    let restored = plan(&f, values).await;
    assert_eq!(
        restored["activeVersionId"], v1["activeVersionId"],
        "semantic version restoration must reuse original identity"
    );
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn catalog_prices_reuse_versions_and_keep_tenant_selections() {
    let f = Fixture::setup().await;
    let svc = service(&f).await;
    let meter = json!({"serviceId":svc["id"],"key":"requests","name":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT"});
    let stored = put(&f, "PutMeter", "meter", meter.clone()).await;
    let mut changed = meter;
    changed["aggregation"] = json!("METER_AGGREGATION_SUM");
    changed["valueAttribute"] = json!("quantity");
    f.error(
        "owner",
        "PutMeter",
        json!({"requestKey":key(),"reason":"reject incompatible meter","meter":changed}),
        "invalid_argument",
    )
    .await;
    let legacy = put(
        &f,
        "PutMeter",
        "meter",
        json!({"serviceId":svc["id"],"name":"requests"}),
    )
    .await;
    assert_eq!(legacy["id"], stored["id"]);
    assert_eq!(legacy["aggregation"], "METER_AGGREGATION_COUNT");
    let base = plan(&f, json!([])).await;
    let mut price = json!({"key":"requests-usd","planId":base["id"],"serviceId":svc["id"],"meter":"requests","unitPrice":"1.0","included":"2.00","currency":"USD"});
    let a = put(&f, "CreatePrice", "price", price.clone()).await;
    let alpha = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Alpha"}),
        )
        .await;
    price["unitPrice"] = json!("1.00");
    let same = put(&f, "CreatePrice", "price", price.clone()).await;
    assert_eq!(a["id"], same["id"]);
    price["unitPrice"] = json!("2");
    let b = put(&f, "CreatePrice", "price", price.clone()).await;
    assert_ne!(a["id"], b["id"]);
    assert_eq!(a["stableId"], b["stableId"]);
    let beta = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Beta"}),
        )
        .await;
    price["unitPrice"] = json!("1");
    let restored = put(&f, "CreatePrice", "price", price).await;
    assert_eq!(restored["id"], a["id"]);
    for (tenant, expected) in [
        (&alpha["tenant"]["id"], &a["id"]),
        (&beta["tenant"]["id"], &b["id"]),
    ] {
        let selected: String = sqlx::query_scalar("SELECT price_id::text FROM entitlements WHERE tenant_id=$1::text::uuid AND ends_at IS NULL")
            .bind(tenant.as_str().unwrap()).fetch_one(&f.db).await.unwrap();
        assert_eq!(selected, expected.as_str().unwrap());
    }
}

/// Admission rows that bound meter cardinality, in a comparable form.
#[derive(Debug, PartialEq)]
struct Admission {
    periods: Vec<i64>,
    groups: Vec<(String, i64, i64)>,
    values: Vec<(String, String, String, i64)>,
}
async fn admission(f: &Fixture) -> Admission {
    Admission {
        periods: sqlx::query_scalar("SELECT groups FROM metering_admission_periods ORDER BY 1")
            .fetch_all(&f.db)
            .await
            .unwrap(),
        groups: sqlx::query_as("SELECT encode(group_digest,'hex'),events,distinct_values FROM metering_admission_groups ORDER BY 1")
            .fetch_all(&f.db)
            .await
            .unwrap(),
        values: sqlx::query_as("SELECT encode(group_digest,'hex'),value_kind,value,events FROM metering_admission_values ORDER BY 1,2,3")
            .fetch_all(&f.db)
            .await
            .unwrap(),
    }
}
/// Revisions, contributions and queued jobs that a rejected correction must not touch.
async fn durable(f: &Fixture) -> (Vec<(String, i32)>, i64, i64) {
    (
        sqlx::query_as("SELECT event_key,latest_revision FROM metering_events ORDER BY 1")
            .fetch_all(&f.db)
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT count(*) FROM metering_contributions")
            .fetch_one(&f.db)
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='metering'")
            .fetch_one(&f.db)
            .await
            .unwrap(),
    )
}
fn domain_attrs(
    region: &str,
    latency: &str,
    user: &str,
    bytes: &str,
) -> std::collections::BTreeMap<String, delcore_api::metering::Value> {
    use delcore_api::metering::Value;
    [
        ("region", Value::string(region)),
        ("bytes", Value::number(bytes).unwrap()),
        ("latency", Value::number(latency).unwrap()),
        ("user", Value::string(user)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}
/// Lowered bounds are applied through the library API because the fixture server keeps
/// the configured limits; the stored state is exactly what an earlier, larger bound left.
struct Lowered<'a> {
    f: &'a Fixture,
    tenant: String,
    service: String,
    policy: delcore_api::events::Policy,
    occurred: chrono::DateTime<Utc>,
}
impl Lowered<'_> {
    async fn correct(
        &self,
        key: &str,
        expected: i32,
        kind: delcore_api::metering::RevisionKind,
        attributes: std::collections::BTreeMap<String, delcore_api::metering::Value>,
    ) -> Result<delcore_api::events::Accepted, delcore_api::Fault> {
        let mut tx = delcore_api::postgres::begin(&self.f.db).await.unwrap();
        let result = delcore_api::events::correct(
            &mut tx,
            Utc::now(),
            &self.policy,
            delcore_api::events::Correction {
                tenant_id: self.tenant.clone(),
                service_id: self.service.clone(),
                key: key.into(),
                expected_revision: expected,
                kind,
                event_type: if attributes.is_empty() {
                    String::new()
                } else {
                    "request".into()
                },
                attributes,
            },
        )
        .await;
        // Roll back explicitly so a rejection releases its locks before the next call.
        if result.is_ok() {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        result
    }
    async fn ingest(
        &self,
        key: &str,
        attributes: std::collections::BTreeMap<String, delcore_api::metering::Value>,
    ) -> Result<delcore_api::events::Accepted, delcore_api::Fault> {
        let mut tx = delcore_api::postgres::begin(&self.f.db).await.unwrap();
        let result = delcore_api::events::ingest(
            &mut tx,
            Utc::now(),
            &self.policy,
            delcore_api::events::Input {
                tenant_id: self.tenant.clone(),
                service_id: self.service.clone(),
                key: key.into(),
                event_type: "request".into(),
                occurred_at: self.occurred,
                attributes,
            },
        )
        .await;
        // Roll back explicitly so a rejection releases its locks before the next call.
        if result.is_ok() {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        result
    }
}
const CARDINALITY_EXCEEDED: i32 = 10;
const STALE_REVISION: i32 = 7;

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn corrections_keep_existing_admission_identities_below_lowered_bounds() {
    use delcore_api::metering::RevisionKind::{Cancel, Replace};
    // Each case varies one identity: the group for COUNT, or the retained typed value
    // inside one group for UNIQUE_COUNT, MIN and MAX.
    for (aggregation, attribute, by_group) in [
        ("COUNT", "", true),
        ("UNIQUE_COUNT", "user", false),
        ("MIN", "latency", false),
        ("MAX", "latency", false),
    ] {
        let f = Fixture::setup().await;
        plan(&f, json!([])).await;
        let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
        let tenant = &me["tenants"][0]["id"];
        let svc = service(&f).await;
        put(&f,"PutMeter","meter",json!({"serviceId":svc["id"],"key":"m","name":"m","eventType":"request","aggregation":format!("METER_AGGREGATION_{aggregation}"),"valueAttribute":attribute,"groupBy":["region"]})).await;
        // Identity `n` is group r<n> for COUNT, otherwise value n inside group "eu".
        let identity = |n: u32, bytes: &str| {
            let (region, latency, user) = if by_group {
                (format!("r{n}"), "1".to_owned(), "u1".to_owned())
            } else {
                ("eu".to_owned(), n.to_string(), format!("u{n}"))
            };
            domain_attrs(&region, &latency, &user, bytes)
        };
        let rpc_identity = |n: u32| {
            let (region, latency, user) = if by_group {
                (format!("r{n}"), "1".to_owned(), "u1".to_owned())
            } else {
                ("eu".to_owned(), n.to_string(), format!("u{n}"))
            };
            attrs(&region, "0", &latency, &user)
        };
        let occurred = Utc::now();
        for n in 1..=3 {
            ingest(
                &f,
                tenant,
                &svc["id"],
                &format!("k{n}"),
                rpc_identity(n),
                &occurred.to_rfc3339(),
            )
            .await;
        }
        let state = Lowered {
            f: &f,
            tenant: tenant.as_str().unwrap().into(),
            service: svc["id"].as_str().unwrap().into(),
            policy: delcore_api::events::Policy {
                lateness: chrono::Duration::seconds(f.config.usage_lateness_seconds),
                reporting_grace: chrono::Duration::seconds(f.config.reporting_grace_seconds),
                limits: delcore_api::metering::Limits {
                    max_groups_per_meter: 1,
                    max_distinct_values_per_group: 1,
                },
            },
            occurred,
        };
        let initial = admission(&f).await;
        let (group_rows, value_rows) = if by_group { (3, 0) } else { (1, 3) };
        assert_eq!(initial.groups.len(), group_rows, "{aggregation}");
        assert_eq!(initial.values.len(), value_rows, "{aggregation}");
        assert_eq!(initial.periods, vec![group_rows as i64], "{aggregation}");
        if by_group {
            assert!(initial.groups.iter().all(|g| g.1 == 1 && g.2 == 0));
        } else {
            assert_eq!((initial.groups[0].1, initial.groups[0].2), (3, 3));
            assert!(initial.values.iter().all(|v| v.3 == 1));
        }

        // The sole reference of an over-bound identity can be replaced while keeping it.
        let moved = state
            .correct("k1", 1, Replace, identity(1, "7"))
            .await
            .unwrap();
        assert_eq!(moved.revision, 2, "{aggregation}");
        assert_eq!(admission(&f).await, initial, "{aggregation}");
        let replay = state
            .correct("k1", 1, Replace, identity(1, "7"))
            .await
            .unwrap();
        assert_eq!(replay.revision, 2, "{aggregation}");
        assert_eq!(admission(&f).await, initial, "{aggregation}");
        let stale = state
            .correct("k1", 1, Replace, identity(1, "8"))
            .await
            .unwrap_err();
        assert_eq!(stale.reason, STALE_REVISION, "{aggregation}");
        f.process_metering().await;
        assert_eq!(
            aggregate(&f, tenant, "m").await.len(),
            group_rows,
            "{aggregation}"
        );

        // A genuinely new group or value, even in a retained group, is still bounded.
        let before = (admission(&f).await, durable(&f).await);
        let rejected = state
            .correct("k1", 2, Replace, identity(4, "7"))
            .await
            .unwrap_err();
        assert_eq!(rejected.reason, CARDINALITY_EXCEEDED, "{aggregation}");
        assert_eq!((admission(&f).await, durable(&f).await), before);
        let rejected = state.ingest("k5", identity(5, "0")).await.unwrap_err();
        assert_eq!(rejected.reason, CARDINALITY_EXCEEDED, "{aggregation}");
        assert_eq!((admission(&f).await, durable(&f).await), before);

        // Another event may join an existing identity, which then has two references.
        state.ingest("k4", identity(2, "0")).await.unwrap();
        let shared = admission(&f).await;
        assert_eq!(shared.groups.iter().map(|g| g.1).sum::<i64>(), 4);
        assert_eq!(shared.periods, initial.periods);
        state
            .correct("k4", 1, Replace, identity(2, "9"))
            .await
            .unwrap();
        assert_eq!(admission(&f).await, shared, "{aggregation}");
        state
            .correct("k4", 2, Cancel, Default::default())
            .await
            .unwrap();
        assert_eq!(admission(&f).await, initial, "{aggregation}");

        // Removing identities releases capacity, so the sole identity can move.
        for key in ["k2", "k3"] {
            state
                .correct(key, 1, Cancel, Default::default())
                .await
                .unwrap();
        }
        let single = admission(&f).await;
        assert_eq!(single.periods, vec![1], "{aggregation}");
        assert_eq!(single.groups.len(), 1, "{aggregation}");
        assert_eq!(single.values.len(), value_rows.min(1), "{aggregation}");
        state
            .correct("k1", 2, Replace, identity(4, "7"))
            .await
            .unwrap();
        let moved = admission(&f).await;
        assert_eq!(moved.periods, vec![1], "{aggregation}");
        assert_eq!(moved.groups.len(), 1, "{aggregation}");
        assert_eq!(moved.values.len(), value_rows.min(1), "{aggregation}");
        assert_ne!(moved, single, "the moved identity must replace the old one");
        state
            .correct("k1", 3, Cancel, Default::default())
            .await
            .unwrap();
        assert_eq!(
            admission(&f).await,
            Admission {
                periods: vec![0],
                groups: vec![],
                values: vec![]
            },
            "{aggregation}"
        );
        // Reintroduction is checked against the current bound.
        state.ingest("k6", identity(6, "0")).await.unwrap();
        let rejected = state.ingest("k7", identity(7, "0")).await.unwrap_err();
        assert_eq!(rejected.reason, CARDINALITY_EXCEEDED, "{aggregation}");
        f.process_metering().await;
        // The worker's empty-queue probe leaves its rollback pending until the pool reuses
        // that connection; run a query so the fixture schema can be dropped.
        let _ = admission(&f).await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn call_time_prices_apply_to_existing_tenants_and_late_usage() {
    let f = Fixture::setup().await;
    let base = plan(&f, json!([])).await;
    let svc = service(&f).await;
    put(&f,"PutMeter","meter",json!({"serviceId":svc["id"],"key":"requests","name":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT"})).await;
    let mut price = json!({"key":"requests-usd","planId":base["id"],"serviceId":svc["id"],"meter":"requests","unitPrice":"1","included":"0","currency":"USD"});
    let a = put(&f, "CreatePrice", "price", price.clone()).await;
    let alpha = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Alpha"}),
        )
        .await;
    let alpha = &alpha["tenant"]["id"];
    let at_a = Utc::now().to_rfc3339();
    let first = ingest(&f, alpha, &svc["id"], "first", json!({}), &at_a).await;

    price["unitPrice"] = json!("2");
    let b = put(&f, "CreatePrice", "price", price.clone()).await;
    let beta = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Beta"}),
        )
        .await;
    let beta = &beta["tenant"]["id"];
    let at_b = Utc::now().to_rfc3339();
    let changed = ingest(&f, alpha, &svc["id"], "changed", json!({}), &at_b).await;
    ingest(&f, beta, &svc["id"], "beta-b", json!({}), &at_b).await;

    price["unitPrice"] = json!("1");
    let restored = put(&f, "CreatePrice", "price", price.clone()).await;
    assert_eq!(restored["id"], a["id"]);
    // An old, unversioned entitlement installed by a delayed subscription
    // transition cannot override the product's effective catalog price.
    let legacy = key();
    sqlx::query("INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,kind) VALUES($1::text::uuid,$2::text::uuid,$3::text::uuid,'requests',100,0,'USD','',now(),1)")
        .bind(&legacy).bind(base["id"].as_str().unwrap()).bind(svc["id"].as_str().unwrap()).execute(&f.db).await.unwrap();
    sqlx::query("INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1::text::uuid,$2::text::uuid,$3::text::uuid,clock_timestamp())")
        .bind(key()).bind(alpha.as_str().unwrap()).bind(legacy).execute(&f.db).await.unwrap();
    let at_restored = Utc::now().to_rfc3339();
    ingest(&f, alpha, &svc["id"], "restored", json!({}), &at_restored).await;
    ingest(&f, beta, &svc["id"], "beta-a", json!({}), &at_restored).await;
    // Delivery and retries use the original execution start even after another
    // activation. The currently active version must not reprice these events.
    let late = ingest(&f, alpha, &svc["id"], "late", json!({}), &at_b).await;
    let replay = ingest(&f, alpha, &svc["id"], "first", json!({}), &at_a).await;
    assert_eq!(replay["eventId"], first["eventId"]);
    f.rpc("service", "CorrectEvent", json!({"requestKey":key(),"tenantId":alpha,"eventKey":"changed","expectedRevision":1,"kind":"EVENT_CORRECTION_KIND_REPLACE","eventType":"request","attributes":{"retry":{"boolean":true}}})).await;
    f.process_metering().await;

    for (event, expected) in [(&first, &a), (&changed, &b), (&late, &b)] {
        let attributed: String = sqlx::query_scalar(
            "SELECT price_id::text FROM metering_event_prices WHERE event_id=$1::text::uuid",
        )
        .bind(event["eventId"].as_str().unwrap())
        .fetch_one(&f.db)
        .await
        .unwrap();
        assert_eq!(attributed, expected["id"].as_str().unwrap());
    }
    let alpha_charge: String = sqlx::query_scalar(
        "SELECT (-sum(amount))::text FROM ledger WHERE tenant_id=$1::text::uuid AND kind IN ('usage','correction')",
    )
    .bind(alpha.as_str().unwrap())
    .fetch_one(&f.db)
    .await
    .unwrap();
    assert_eq!(alpha_charge, "6");
    let beta_charge: String = sqlx::query_scalar(
        "SELECT (-sum(amount))::text FROM ledger WHERE tenant_id=$1::text::uuid AND kind IN ('usage','correction')",
    )
    .bind(beta.as_str().unwrap())
    .fetch_one(&f.db)
    .await
    .unwrap();
    assert_eq!(beta_charge, "3");

    // Reapplying the same version is not another activation.
    put(&f, "CreatePrice", "price", price).await;
    let activations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM catalog_price_activations WHERE price_id=$1::text::uuid",
    )
    .bind(a["stableId"].as_str().unwrap())
    .fetch_one(&f.db)
    .await
    .unwrap();
    assert_eq!(activations, 3);
    assert!(
        sqlx::query(
            "UPDATE catalog_price_activations SET effective_at=now() WHERE price_id=$1::text::uuid"
        )
        .bind(a["stableId"].as_str().unwrap())
        .execute(&f.db)
        .await
        .is_err()
    );
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn thirty_day_expiry_preserves_accepted_accounting_and_pending_work() {
    let f = Fixture::setup().await;
    let base = plan(&f, json!([])).await;
    let svc = service(&f).await;
    put(&f,"PutMeter","meter",json!({"serviceId":svc["id"],"key":"requests","name":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT"})).await;
    put(&f,"CreatePrice","price",json!({"key":"requests-usd","planId":base["id"],"serviceId":svc["id"],"meter":"requests","unitPrice":"1","included":"0","currency":"USD"})).await;
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let tenant = &me["tenants"][0]["id"];
    let at = Utc::now().to_rfc3339();
    ingest(&f, tenant, &svc["id"], "accepted", json!({}), &at).await;
    f.process_metering().await;
    ingest(&f, tenant, &svc["id"], "pending", json!({}), &at).await;
    sqlx::query("UPDATE jobs SET state='failed' WHERE kind='metering' AND state='pending'")
        .execute(&f.db)
        .await
        .unwrap();
    let now = Utc::now() + chrono::Duration::days(31);
    f.worker.maintain(now).await.unwrap();
    let (events, ledger, failed, requests, audit): (i64, i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM metering_events),(SELECT count(*) FROM ledger WHERE kind='usage'),(SELECT count(*) FROM jobs WHERE kind='metering' AND state='failed'),(SELECT count(*) FROM requests),(SELECT count(*) FROM audit)")
        .fetch_one(&f.db).await.unwrap();
    assert_eq!((events, ledger, failed, requests, audit), (2, 1, 1, 0, 0));
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn addon_call_time_prices_respect_product_ownership() {
    let f = Fixture::setup().await;
    plan(&f, json!([])).await;
    let svc = service(&f).await;
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let tenant = &me["tenants"][0]["id"];
    put(&f,"PutMeter","meter",json!({"serviceId":svc["id"],"key":"requests","name":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT"})).await;
    let addon = put(
        &f,
        "PutAddOn",
        "addOn",
        json!({"key":"extra","name":"Extra","serviceId":svc["id"]}),
    )
    .await;
    let mut price = json!({"key":"requests-usd","addOnId":addon["id"],"serviceId":svc["id"],"meter":"requests","unitPrice":"1","included":"0","currency":"USD"});
    let a = put(&f, "CreatePrice", "price", price.clone()).await;
    sqlx::query("INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1::text::uuid,$2::text::uuid,$3::text::uuid,clock_timestamp())")
        .bind(key()).bind(tenant.as_str().unwrap()).bind(a["id"].as_str().unwrap()).execute(&f.db).await.unwrap();
    let before_change = Utc::now().to_rfc3339();
    price["unitPrice"] = json!("2");
    let b = put(&f, "CreatePrice", "price", price).await;
    let unrelated = put(
        &f,
        "PutPlan",
        "plan",
        json!({"key":"other","name":"Other","rank":1,"free":true}),
    )
    .await;
    put(&f,"CreatePrice","price",json!({"key":"requests-usd","planId":unrelated["id"],"serviceId":svc["id"],"meter":"requests","unitPrice":"100","included":"0","currency":"USD"})).await;
    let after_change = Utc::now().to_rfc3339();
    let old = ingest(&f, tenant, &svc["id"], "old", json!({}), &before_change).await;
    let new = ingest(&f, tenant, &svc["id"], "new", json!({}), &after_change).await;
    for (event, price) in [(&old, &a), (&new, &b)] {
        let attributed: String = sqlx::query_scalar(
            "SELECT price_id::text FROM metering_event_prices WHERE event_id=$1::text::uuid",
        )
        .bind(event["eventId"].as_str().unwrap())
        .fetch_one(&f.db)
        .await
        .unwrap();
        assert_eq!(attributed, price["id"].as_str().unwrap());
    }
}

/// A reporting request that read its timestamp before waiting for the state-transition lock must
/// be validated against the time at which it actually holds the lock. Otherwise it could commit a
/// metering job after the deleting tenant's final cancellation check already ran.
#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn event_waiting_for_the_state_lock_is_validated_after_reporting_grace_ends() {
    let f = Fixture::setup().await;
    plan(&f, json!([])).await;
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let tenant = &me["tenants"][0]["id"];
    let svc = service(&f).await;
    put(&f,"PutMeter","meter",json!({"serviceId":svc["id"],"key":"requests","name":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT","valueAttribute":"","groupBy":[]})).await;
    // Reporting grace ends two seconds from now.
    sqlx::query("UPDATE tenants SET deleting_at=now()-make_interval(secs=>$1::double precision)+interval '2 seconds' WHERE id=$2::text::uuid")
        .bind(f.config.reporting_grace_seconds as f64).bind(tenant.as_str().unwrap()).execute(&f.db).await.unwrap();
    // The event occurred before deletion started and is still inside the lateness window when the
    // request starts, so a timestamp from before the lock wait would accept it.
    let occurred = (Utc::now() - chrono::Duration::seconds(f.config.usage_lateness_seconds)
        + chrono::Duration::milliseconds(1500))
    .to_rfc3339();
    let mut holder = f.db.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(41820003)")
        .execute(&mut *holder)
        .await
        .unwrap();
    // The request starts inside the grace period but only gets the lock after it has ended.
    let release = async {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        holder.commit().await.unwrap();
    };
    let request = f.error("service","IngestEvent",json!({"requestKey":key(),"event":{"tenantId":tenant,"serviceId":svc["id"],"eventKey":"late","eventType":"request","occurredAt":occurred,"attributes":{}}}),"failed_precondition");
    let ((), error) = tokio::join!(release, request);
    let message = error["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("grace") || message.contains("lateness"),
        "expected a reporting window rejection: {error}"
    );
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM metering_events")
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(events, 0, "the late request must leave no accepted event");
}

/// Observable selection state of a tenant: base plan, active catalog versions and open entitlements.
async fn selection_state(f: &Fixture, tenant: &Value) -> (String, Vec<String>, i64) {
    let tenant = tenant.as_str().unwrap();
    let plan: String =
        sqlx::query_scalar("SELECT plan_id::text FROM tenants WHERE id=$1::text::uuid")
            .bind(tenant)
            .fetch_one(&f.db)
            .await
            .unwrap();
    let versions: Vec<String> = sqlx::query_scalar("SELECT version_id::text FROM tenant_catalog_selections WHERE tenant_id=$1::text::uuid AND ends_at IS NULL ORDER BY version_id::text")
        .bind(tenant)
        .fetch_all(&f.db)
        .await
        .unwrap();
    let entitlements: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM entitlements WHERE tenant_id=$1::text::uuid AND ends_at IS NULL",
    )
    .bind(tenant)
    .fetch_one(&f.db)
    .await
    .unwrap();
    (plan, versions, entitlements)
}
async fn job_state(f: &Fixture, job: &Value) -> (String, String) {
    sqlx::query_as("SELECT state,payload->>'timing' FROM jobs WHERE id=$1::text::uuid")
        .bind(job.as_str().unwrap())
        .fetch_one(&f.db)
        .await
        .map(|(state, timing): (String, Option<String>)| (state, timing.unwrap_or_default()))
        .unwrap()
}
async fn audit_count(f: &Fixture, tenant: &Value, action: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit WHERE tenant_id=$1::text::uuid AND action=$2")
        .bind(tenant.as_str().unwrap())
        .bind(action)
        .fetch_one(&f.db)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn explicit_changes_accept_price_free_free_plan_versions() {
    let f = Fixture::setup().await;
    plan(&f, json!([])).await;
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let svc = service(&f).await;
    put(&f,"PutMeter","meter",json!({"serviceId":svc["id"],"key":"requests","name":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT","groupBy":["region"]})).await;
    let access = put(
        &f,
        "PutFeature",
        "feature",
        json!({"serviceId":svc["id"],"key":"api","name":"API","kind":"FEATURE_KIND_ACCESS"}),
    )
    .await;
    let metered = put(&f,"PutFeature","feature",json!({"serviceId":svc["id"],"key":"limit","name":"Limit","kind":"FEATURE_KIND_METERED","meterKey":"requests"})).await;
    let values =
        json!([{"featureId":access["id"],"enabled":true},{"featureId":metered["id"],"limit":"2"}]);
    let v1 = plan(&f, values.clone()).await;
    let free_id = v1["id"].clone();
    let alpha = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Alpha"}),
        )
        .await;
    let tenant = &alpha["tenant"]["id"];
    let mut v2values = values.clone();
    v2values[1]["limit"] = json!("5");
    let v2 = plan(&f, v2values).await;
    assert_ne!(v1["activeVersionId"], v2["activeVersionId"]);
    let group = json!({"region":{"text":"eu"}});
    let limit = |tenant: Value| {
        let f = &f;
        let me = &me;
        let group = &group;
        async move {
            let result = f.rpc("service","Authorize",json!({"tenantId":tenant,"userId":me["userId"],"capability":"execute","featureKey":"limit","groupValues":group})).await;
            result["feature"]["limit"].clone()
        }
    };
    let change = |tenant: Value, plan: Value, prices: Value| json!({"requestKey":key(),"tenantId":tenant,"planId":plan,"priceIds":prices,"returnUrl":f.config.return_urls[0]});
    assert_eq!(
        selection_state(&f, tenant).await.1,
        vec![v1["activeVersionId"].as_str().unwrap().to_owned()]
    );
    assert_eq!(limit(tenant.clone()).await, "2");

    // Explicit refresh with an empty list selects the active price-free version immediately.
    let job = f
        .rpc(
            "owner",
            "ChangeSubscription",
            change(tenant.clone(), free_id.clone(), json!([])),
        )
        .await["jobId"]
        .clone();
    assert_eq!(job_state(&f, &job).await.0, "done");
    assert_eq!(
        audit_count(&f, tenant, "subscription.free_refresh").await,
        1
    );
    assert_eq!(
        selection_state(&f, tenant).await.1,
        vec![v2["activeVersionId"].as_str().unwrap().to_owned()]
    );
    assert_eq!(limit(tenant.clone()).await, "5");
    // A second refresh of the unchanged version keeps the selection and adds no entitlements.
    let before = selection_state(&f, tenant).await;
    let again = f
        .rpc(
            "owner",
            "ChangeSubscription",
            change(tenant.clone(), free_id.clone(), json!([])),
        )
        .await["jobId"]
        .clone();
    assert_eq!(job_state(&f, &again).await.0, "done");
    assert_eq!(selection_state(&f, tenant).await, before);
    assert_eq!(
        audit_count(&f, tenant, "subscription.free_refresh").await,
        2
    );
    assert_eq!(limit(tenant.clone()).await, "5");

    // Switching to a higher-ranked price-free free plan is local and immediate.
    let growth = put(&f,"PutPlan","plan",json!({"key":"growth","name":"Growth","rank":1,"free":true,"featureValues":[{"featureId":access["id"],"enabled":true}]})).await;
    let job = f
        .rpc(
            "owner",
            "ChangeSubscription",
            change(tenant.clone(), growth["id"].clone(), json!([])),
        )
        .await["jobId"]
        .clone();
    assert_eq!(
        job_state(&f, &job).await,
        ("done".to_owned(), String::new())
    );
    let (plan_id, versions, entitlements) = selection_state(&f, tenant).await;
    assert_eq!(plan_id, growth["id"].as_str().unwrap());
    assert_eq!(
        versions,
        vec![growth["activeVersionId"].as_str().unwrap().to_owned()]
    );
    assert_eq!(entitlements, 0);
    let payload: Value = sqlx::query_scalar("SELECT payload FROM jobs WHERE id=$1::text::uuid")
        .bind(job.as_str().unwrap())
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert!(
        payload.get("external_price_ids").is_none(),
        "no provider operation: {payload}"
    );

    // A lower-ranked price-free target is scheduled for the free period boundary.
    let job = f
        .rpc(
            "owner",
            "ChangeSubscription",
            change(tenant.clone(), free_id.clone(), json!([])),
        )
        .await["jobId"]
        .clone();
    assert_eq!(
        job_state(&f, &job).await,
        ("pending".to_owned(), "free_next_period".to_owned())
    );
    let scheduled = selection_state(&f, tenant).await;
    assert_eq!(scheduled.0, growth["id"].as_str().unwrap());
    f.error(
        "owner",
        "ChangeSubscription",
        change(tenant.clone(), free_id.clone(), json!([])),
        "failed_precondition",
    )
    .await;
    let due: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT available_at FROM jobs WHERE id=$1::text::uuid")
            .bind(job.as_str().unwrap())
            .fetch_one(&f.db)
            .await
            .unwrap();
    f.worker.maintain(due).await.unwrap();
    assert_eq!(job_state(&f, &job).await.0, "done");
    let (plan_id, versions, _) = selection_state(&f, tenant).await;
    assert_eq!(plan_id, free_id.as_str().unwrap());
    assert_eq!(
        versions,
        vec![v2["activeVersionId"].as_str().unwrap().to_owned()]
    );

    // Empty lists remain invalid when the target product needs prices.
    let paid = put(
        &f,
        "PutPlan",
        "plan",
        json!({"key":"paid","name":"Paid","rank":2,"free":false}),
    )
    .await;
    let usage = put(
        &f,
        "PutPlan",
        "plan",
        json!({"key":"usage","name":"Usage","rank":3,"free":true}),
    )
    .await;
    let usage_price = put(&f,"CreatePrice","price",json!({"key":"requests-usd","planId":usage["id"],"serviceId":svc["id"],"meter":"requests","unitPrice":"1.0","included":"2.00","currency":"USD"})).await;
    let snapshot = selection_state(&f, tenant).await;
    let jobs_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE tenant_id=$1::text::uuid AND kind='subscription'",
    )
    .bind(tenant.as_str().unwrap())
    .fetch_one(&f.db)
    .await
    .unwrap();
    let rejected = [
        change(tenant.clone(), paid["id"].clone(), json!([])),
        change(tenant.clone(), usage["id"].clone(), json!([])),
        change(tenant.clone(), free_id.clone(), json!([usage_price["id"]])),
        change(
            tenant.clone(),
            usage["id"].clone(),
            json!([usage_price["id"], usage_price["id"]]),
        ),
        change(tenant.clone(), usage["id"].clone(), json!([key()])),
        change(tenant.clone(), usage["id"].clone(), json!(vec![key(); 101])),
        change(tenant.clone(), usage["id"].clone(), json!(["not-a-uuid"])),
        change(tenant.clone(), json!("not-a-uuid"), json!([])),
    ];
    for request in rejected {
        f.error("owner", "ChangeSubscription", request, "invalid_argument")
            .await;
    }
    let mut foreign_return = change(tenant.clone(), free_id.clone(), json!([]));
    foreign_return["returnUrl"] = json!("https://evil.example/return");
    f.error(
        "owner",
        "ChangeSubscription",
        foreign_return,
        "invalid_argument",
    )
    .await;
    assert_eq!(selection_state(&f, tenant).await, snapshot);
    let jobs_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE tenant_id=$1::text::uuid AND kind='subscription'",
    )
    .bind(tenant.as_str().unwrap())
    .fetch_one(&f.db)
    .await
    .unwrap();
    assert_eq!(jobs_before, jobs_after);

    // The exact catalog price set of a free plan with usage prices is still accepted.
    let job = f
        .rpc(
            "owner",
            "ChangeSubscription",
            change(
                tenant.clone(),
                usage["id"].clone(),
                json!([usage_price["id"]]),
            ),
        )
        .await["jobId"]
        .clone();
    assert_eq!(job_state(&f, &job).await.0, "done");
    assert_eq!(selection_state(&f, tenant).await.2, 1);

    // A tenant with an external subscription must use cancellation instead.
    let beta = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Beta"}),
        )
        .await;
    let beta_id = &beta["tenant"]["id"];
    sqlx::query(
        "UPDATE tenants SET external_subscription_id='sub_fixture' WHERE id=$1::text::uuid",
    )
    .bind(beta_id.as_str().unwrap())
    .execute(&f.db)
    .await
    .unwrap();
    let before = selection_state(&f, beta_id).await;
    f.error(
        "owner",
        "ChangeSubscription",
        change(beta_id.clone(), growth["id"].clone(), json!([])),
        "failed_precondition",
    )
    .await;
    assert_eq!(selection_state(&f, beta_id).await, before);
}
