// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Fixture, key};
use chrono::Utc;
use delcore_api::{config::Config, worker::Worker};
use serde_json::{Value, json};
use sqlx::Row;

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

/// Operator-visible tenant of the fixture owner.
async fn owner_tenant(f: &Fixture) -> String {
    f.rpc("owner", "GetCurrentUser", json!({})).await["tenants"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
/// Queue a metering job directly so a test controls payload and attempt count.
async fn seed_job(f: &Fixture, tenant: &str, payload: Value, attempts: i32) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,state,attempts,available_at,created_at,updated_at) VALUES($1,$2::text::uuid,'metering',$3,$4,'pending',$5,now(),now(),now())")
        .bind(id).bind(tenant).bind(format!("metering:seed:{id}")).bind(payload).bind(attempts)
        .execute(&f.db).await.unwrap();
    id
}
struct JobRow {
    state: String,
    category: String,
    attempts: i32,
    failed: bool,
    due: bool,
}
async fn job_row(f: &Fixture, id: uuid::Uuid) -> JobRow {
    let row = sqlx::query("SELECT state,error_category,attempts,failed_at IS NOT NULL,available_at>now() FROM jobs WHERE id=$1")
        .bind(id).fetch_one(&f.db).await.unwrap();
    JobRow {
        state: row.get(0),
        category: row.get(1),
        attempts: row.get(2),
        failed: row.get(3),
        due: row.get(4),
    }
}
/// Audit rows are read with a uuid-typed column so a non-UUID value cannot pass.
async fn failure_audits(
    f: &Fixture,
    tenant: &str,
) -> Vec<(String, Option<String>, String, uuid::Uuid)> {
    sqlx::query("SELECT actor_id,tenant_id::text,reason,correlation_id FROM audit WHERE action='metering.failed' AND tenant_id=$1::text::uuid ORDER BY created_at")
        .bind(tenant).fetch_all(&f.db).await.unwrap().into_iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3))).collect()
}
async fn count_failure_audits(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit WHERE action='metering.failed'")
        .fetch_one(&f.db)
        .await
        .unwrap()
}
fn assert_failure_audit(
    rows: &[(String, Option<String>, String, uuid::Uuid)],
    tenant: &str,
    reason: &str,
) {
    assert_eq!(rows.len(), 1, "exactly one metering.failed audit row");
    assert_eq!(rows[0].0, "system:metering");
    assert_eq!(rows[0].1.as_deref(), Some(tenant));
    assert_eq!(rows[0].2, reason);
    assert!(!rows[0].3.is_nil());
}
/// Accept one real event so a valid metering job and event row exist.
async fn accepted_event(f: &Fixture) -> (String, uuid::Uuid) {
    plan(f, json!([])).await;
    let tenant = owner_tenant(f).await;
    let svc = service(f).await;
    put(f,"PutMeter","meter",json!({"serviceId":svc["id"],"key":"requests","name":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT","groupBy":["region"]})).await;
    let accepted = ingest(
        f,
        &json!(tenant),
        &svc["id"],
        "e1",
        attrs("eu", "1", "1", "u1"),
        &Utc::now().to_rfc3339(),
    )
    .await;
    let event = accepted["eventId"].as_str().unwrap().to_owned();
    let job =
        sqlx::query_scalar("SELECT id FROM jobs WHERE kind='metering' AND payload->>'event_id'=$1")
            .bind(&event)
            .fetch_one(&f.db)
            .await
            .unwrap();
    (tenant, job)
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn permanent_metering_failures_commit_state_and_audit_atomically() {
    let f = Fixture::setup().await;
    plan(&f, json!([])).await;
    let tenant = owner_tenant(&f).await;
    let invalid = seed_job(&f, &tenant, json!({}), 0).await;
    f.process_metering().await;
    let row = job_row(&f, invalid).await;
    assert_eq!(
        (
            row.state.as_str(),
            row.category.as_str(),
            row.attempts,
            row.failed
        ),
        ("failed", "invalid_payload", 1, true)
    );
    assert_failure_audit(
        &failure_audits(&f, &tenant).await,
        &tenant,
        "category=invalid_payload attempts=1",
    );
    // A later worker pass neither reprocesses the failed job nor duplicates its audit.
    f.process_metering().await;
    assert_eq!(job_row(&f, invalid).await.attempts, 1);
    assert_eq!(count_failure_audits(&f).await, 1);

    let missing = seed_job(
        &f,
        &tenant,
        json!({"event_id":"01900000-0000-7000-8000-000000000099"}),
        0,
    )
    .await;
    f.process_metering().await;
    let row = job_row(&f, missing).await;
    assert_eq!(
        (
            row.state.as_str(),
            row.category.as_str(),
            row.attempts,
            row.failed
        ),
        ("failed", "metering_invalid", 1, true)
    );
    let audits = failure_audits(&f, &tenant).await;
    assert_eq!(audits.len(), 2);
    assert!(
        audits
            .iter()
            .any(|a| a.2 == "category=metering_invalid attempts=1")
    );
    assert_ne!(
        audits[0].3, audits[1].3,
        "each failure gets its own correlation ID"
    );

    // Operators see the failure and RetryJob keeps its documented reset behavior.
    let jobs = f.rpc("owner", "ListJobs", json!({})).await;
    let listed = jobs["jobs"].as_array().unwrap();
    for id in [invalid, missing] {
        let item = listed.iter().find(|j| j["id"] == id.to_string()).unwrap();
        assert_eq!(item["state"], "JOB_STATE_FAILED");
        assert_eq!(item["attempts"], 1);
    }
    let status = f
        .rpc("owner", "GetMeteringStatus", json!({"tenantId":tenant}))
        .await;
    assert_eq!(status["failedJobs"].to_string().trim_matches('"'), "2");
    f.rpc(
        "owner",
        "RetryJob",
        json!({"requestKey":key(),"jobId":invalid.to_string(),"reason":"operator retry after fix"}),
    )
    .await;
    let row = job_row(&f, invalid).await;
    assert_eq!((row.state.as_str(), row.attempts), ("pending", 0));
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn terminal_metering_failure_does_not_starve_other_jobs_or_release_deletion() {
    let f = Fixture::setup().await;
    plan(&f, json!([])).await;
    let tenant = owner_tenant(&f).await;
    let invalid = seed_job(&f, &tenant, json!({}), 0).await;
    // A non-object email payload fails locally without contacting a provider.
    let email = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1,$2::text::uuid,'email',$3,'[]'::jsonb,now(),now(),now())")
        .bind(email).bind(&tenant).bind(format!("email:seed:{email}")).execute(&f.db).await.unwrap();
    f.worker
        .step()
        .await
        .expect("a terminal metering failure must not fail the worker step");
    assert_eq!(job_row(&f, invalid).await.state, "failed");
    let row = job_row(&f, email).await;
    assert_eq!(
        (row.state.as_str(), row.category.as_str()),
        ("failed", "invalid_payload")
    );
    f.worker.step().await.unwrap();
    assert_eq!(job_row(&f, invalid).await.attempts, 1);
    assert_eq!(count_failure_audits(&f).await, 1);
    // Failed metering work keeps holding final deletion until it is recovered.
    let held: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE tenant_id=$1::text::uuid AND kind<>'email' AND state<>'done')")
        .bind(&tenant).fetch_one(&f.db).await.unwrap();
    assert!(held);
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn exhausted_metering_retries_commit_failed_state_with_audit() {
    let f = Fixture::setup().await;
    let (tenant, job) = accepted_event(&f).await;
    // A trigger in this test's private schema makes applying the event fail with a
    // database error, which is transient. A held row lock would leave the abandoned
    // claim transaction waiting on the server and blocking the failure transaction.
    sqlx::query("CREATE FUNCTION fail_apply() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected apply failure'; END $$")
        .execute(&f.db).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_apply BEFORE UPDATE OF applied_revision ON metering_events FOR EACH ROW EXECUTE FUNCTION fail_apply()")
        .execute(&f.db).await.unwrap();
    let worker = Worker::new(
        f.db.clone(),
        Config {
            retry_limit: 3,
            ..f.config.clone()
        },
    )
    .unwrap();

    // Below the limit the failure defers without a terminal audit.
    sqlx::query("UPDATE jobs SET attempts=1 WHERE id=$1")
        .bind(job)
        .execute(&f.db)
        .await
        .unwrap();
    assert_eq!(worker.process_metering(1).await.unwrap(), 1);
    let row = job_row(&f, job).await;
    assert_eq!(
        (
            row.state.as_str(),
            row.category.as_str(),
            row.attempts,
            row.due
        ),
        ("pending", "metering_transient", 2, true)
    );
    assert_eq!(count_failure_audits(&f).await, 0);

    // The attempt that reaches the limit becomes terminal in the same transaction as its audit.
    sqlx::query("UPDATE jobs SET available_at=now() WHERE id=$1")
        .bind(job)
        .execute(&f.db)
        .await
        .unwrap();
    assert_eq!(worker.process_metering(1).await.unwrap(), 1);
    let row = job_row(&f, job).await;
    assert_eq!(
        (
            row.state.as_str(),
            row.category.as_str(),
            row.attempts,
            row.failed
        ),
        ("failed", "retry_exhausted", 3, true)
    );
    assert_failure_audit(
        &failure_audits(&f, &tenant).await,
        &tenant,
        "category=retry_exhausted attempts=3",
    );
    assert_eq!(worker.process_metering(10).await.unwrap(), 0);
    assert_eq!(count_failure_audits(&f).await, 1);
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn valid_metering_job_completes_without_failure_audit() {
    let f = Fixture::setup().await;
    let (_, job) = accepted_event(&f).await;
    f.process_metering().await;
    let row = job_row(&f, job).await;
    assert_eq!(
        (row.state.as_str(), row.category.as_str(), row.attempts),
        ("done", "", 1)
    );
    assert_eq!(count_failure_audits(&f).await, 0);
}
