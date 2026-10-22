// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Fixture, key};
use chrono::{Duration, Utc};
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

async fn change(f: &Fixture, tenant: &Value, plan: &Value, prices: Value) -> Value {
    f.rpc("owner","ChangeSubscription",json!({"requestKey":key(),"tenantId":tenant,"planId":plan["id"],"priceIds":prices,"returnUrl":f.config.return_urls[0]})).await
}
async fn change_error(f: &Fixture, tenant: &Value, plan: &Value, prices: Value, code: &str) {
    f.error("owner","ChangeSubscription",json!({"requestKey":key(),"tenantId":tenant,"planId":plan["id"],"priceIds":prices,"returnUrl":f.config.return_urls[0]}),code).await;
}
async fn limit(f: &Fixture, tenant: &Value, user: &Value) -> Value {
    f.rpc("service","Authorize",json!({"tenantId":tenant,"userId":user,"capability":"execute","featureKey":"limit","groupValues":{}})).await["feature"]["limit"].clone()
}
async fn job_state(f: &Fixture, job: &Value) -> String {
    sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1::text::uuid")
        .bind(job.as_str().unwrap())
        .fetch_one(&f.db)
        .await
        .unwrap()
}
async fn count(f: &Fixture, table: &str, tenant: &Value) -> i64 {
    sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {table} WHERE tenant_id=$1::text::uuid"
    ))
    .bind(tenant.as_str().unwrap())
    .fetch_one(&f.db)
    .await
    .unwrap()
}
async fn selected_plan(f: &Fixture, tenant: &Value) -> String {
    sqlx::query_scalar("SELECT plan_id::text FROM tenants WHERE id=$1::text::uuid")
        .bind(tenant.as_str().unwrap())
        .fetch_one(&f.db)
        .await
        .unwrap()
}
async fn limit_feature(f: &Fixture, svc: &Value) -> Value {
    put(&f,"PutMeter","meter",json!({"serviceId":svc["id"],"key":"requests","name":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT"})).await;
    put(&f,"PutFeature","feature",json!({"serviceId":svc["id"],"key":"limit","name":"Limit","kind":"FEATURE_KIND_METERED","meterKey":"requests"})).await
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

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn price_free_free_plans_change_versions_with_empty_prices() {
    let f = Fixture::setup().await;
    plan(&f, json!([])).await;
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let svc = service(&f).await;
    let feature = limit_feature(&f, &svc).await;
    let values = |n: &str| json!([{"featureId":feature["id"],"limit":n}]);
    let v1 = plan(&f, values("2")).await;
    let alpha = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Alpha"}),
        )
        .await;
    let tenant = &alpha["tenant"]["id"];
    let v2 = plan(&f, values("5")).await;
    assert_ne!(v1["activeVersionId"], v2["activeVersionId"]);
    assert_eq!(limit(&f, tenant, &me["userId"]).await, "2");

    let refresh = change(&f, tenant, &v2, json!([])).await;
    assert_eq!(job_state(&f, &refresh["jobId"]).await, "done");
    assert_eq!(limit(&f, tenant, &me["userId"]).await, "5");
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit WHERE tenant_id=$1::text::uuid AND action='subscription.free_refresh'").bind(tenant.as_str().unwrap()).fetch_one(&f.db).await.unwrap();
    assert_eq!(audits, 1);

    let higher = put(
        &f,
        "PutPlan",
        "plan",
        json!({"key":"plus","name":"Plus","rank":5,"free":true,"featureValues":values("9")}),
    )
    .await;
    let upgrade = change(&f, tenant, &higher, json!([])).await;
    assert_eq!(job_state(&f, &upgrade["jobId"]).await, "done");
    assert_eq!(selected_plan(&f, tenant).await, higher["id"]);
    assert_eq!(limit(&f, tenant, &me["userId"]).await, "9");

    let downgrade = change(&f, tenant, &v2, json!([])).await;
    assert_eq!(job_state(&f, &downgrade["jobId"]).await, "pending");
    assert_eq!(selected_plan(&f, tenant).await, higher["id"]);
    assert_eq!(limit(&f, tenant, &me["userId"]).await, "9");
    // Move the scheduled period boundary into the past instead of the clock forward.
    sqlx::query("UPDATE jobs SET available_at=$2 WHERE id=$1::text::uuid")
        .bind(downgrade["jobId"].as_str().unwrap())
        .bind(Utc::now() - Duration::minutes(1))
        .execute(&f.db)
        .await
        .unwrap();
    f.worker.maintain(Utc::now()).await.unwrap();
    assert_eq!(job_state(&f, &downgrade["jobId"]).await, "done");
    assert_eq!(selected_plan(&f, tenant).await, v2["id"]);
    assert_eq!(limit(&f, tenant, &me["userId"]).await, "5");
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn empty_price_changes_require_price_free_free_plans() {
    let f = Fixture::setup().await;
    let free = plan(&f, json!([])).await;
    let svc = service(&f).await;
    limit_feature(&f, &svc).await;
    let paid = put(
        &f,
        "PutPlan",
        "plan",
        json!({"key":"pro","name":"Pro","rank":10,"free":false}),
    )
    .await;
    let priced = put(
        &f,
        "PutPlan",
        "plan",
        json!({"key":"priced","name":"Priced","rank":1,"free":true}),
    )
    .await;
    let price = put(&f,"CreatePrice","price",json!({"key":"requests-usd","planId":priced["id"],"serviceId":svc["id"],"meter":"requests","unitPrice":"1","included":"2","currency":"USD"})).await;
    let alpha = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Alpha"}),
        )
        .await;
    let tenant = &alpha["tenant"]["id"];
    let before = (
        selected_plan(&f, tenant).await,
        count(&f, "jobs", tenant).await,
        count(&f, "entitlements", tenant).await,
    );
    change_error(&f, tenant, &paid, json!([]), "invalid_argument").await;
    change_error(&f, tenant, &priced, json!([]), "invalid_argument").await;
    change_error(&f, tenant, &free, json!([price["id"]]), "invalid_argument").await;
    change_error(
        &f,
        tenant,
        &priced,
        json!([price["id"], price["id"]]),
        "invalid_argument",
    )
    .await;
    let many: Vec<String> = (0..101).map(|_| key()).collect();
    change_error(&f, tenant, &priced, json!(many), "invalid_argument").await;
    let after = (
        selected_plan(&f, tenant).await,
        count(&f, "jobs", tenant).await,
        count(&f, "entitlements", tenant).await,
    );
    assert_eq!(before, after);

    sqlx::query(
        "UPDATE tenants SET external_subscription_id='sub_external' WHERE id=$1::text::uuid",
    )
    .bind(tenant.as_str().unwrap())
    .execute(&f.db)
    .await
    .unwrap();
    change_error(&f, tenant, &free, json!([]), "failed_precondition").await;
    assert_eq!(count(&f, "jobs", tenant).await, before.1);
}
