// SPDX-License-Identifier: FSL-1.1-ALv2
//! PostgreSQL integration tests: migrations, outbox durability, restart, duplicate delivery,
//! the 30-day expiry boundary and end-to-end billing. They need a disposable server named by
//! `TEST_DATABASE_URL`; each test creates and drops its own schema.

mod common;

use chrono::{DateTime, Duration, Utc};
use common::*;
use delitools_api::{
    outbox::{Delivery, Outbox},
    store,
    usage::{Direction, NewEvent},
};
use serde_json::{Value, json};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

/// PostgreSQL stores microseconds; truncating here keeps timestamp comparisons exact
/// on platforms whose clock has nanosecond precision.
fn now_micros() -> DateTime<Utc> {
    DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap()
}

const NOTE: &str = "requires a disposable TEST_DATABASE_URL";

fn event(
    request: Uuid,
    attempt: u32,
    direction: Direction,
    meter: &str,
    quantity: i64,
) -> NewEvent {
    NewEvent {
        direction,
        event_key: format!("{request}:{attempt}:{}", direction.as_str()),
        meter_key: meter.into(),
        quantity,
    }
}

async fn insert(
    outbox: &Outbox,
    tenant: &str,
    request: Uuid,
    attempt: u32,
    created: DateTime<Utc>,
) {
    outbox
        .insert_attempt(
            Uuid::parse_str(tenant).unwrap(),
            request,
            attempt,
            created,
            &[
                event(request, attempt, Direction::Input, "gem_in", 100),
                event(request, attempt, Direction::Output, "gem_out", 40),
            ],
            created,
        )
        .await
        .unwrap();
}

async fn count(db: &sqlx::PgPool, state: Option<&str>) -> i64 {
    match state {
        Some(state) => sqlx::query_scalar("SELECT count(*) FROM usage_outbox WHERE state=$1")
            .bind(state)
            .fetch_one(db)
            .await
            .unwrap(),
        None => sqlx::query_scalar("SELECT count(*) FROM usage_outbox")
            .fetch_one(db)
            .await
            .unwrap(),
    }
}

fn delivery(h: &Harness, pool: &sqlx::PgPool) -> Delivery {
    Delivery {
        outbox: Outbox::new(pool.clone()),
        delcore: h.server.delcore.clone(),
        settings: h.config.outbox.clone(),
        metrics: h.server.metrics.clone(),
    }
}

// ---------------------------------------------------------------------------------------------
// Migrations and schema checks
// ---------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn migrations_are_explicit_idempotent_and_checked() {
    let _ = NOTE;
    let db = TestDb::new().await;
    assert!(
        store::check(&db.pool).await.is_err(),
        "an unmigrated database is incompatible"
    );
    store::migrate(&db.pool).await.unwrap();
    store::migrate(&db.pool).await.unwrap();
    store::check(&db.pool).await.unwrap();
    let versions: i64 = sqlx::query_scalar("SELECT count(*) FROM delitools_schema")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(versions as usize, store::migration_count());

    // A changed digest is a checksum mismatch for both commands.
    sqlx::query("UPDATE delitools_schema SET digest='tampered' WHERE version=1")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        store::check(&db.pool)
            .await
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
    assert!(
        store::migrate(&db.pool)
            .await
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
    db.finish().await;

    // A database newer than the binary is refused.
    let db = TestDb::migrated().await;
    sqlx::query("INSERT INTO delitools_schema VALUES(2,'future',now())")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        store::migrate(&db.pool)
            .await
            .unwrap_err()
            .to_string()
            .contains("newer")
    );
    assert!(store::check(&db.pool).await.is_err());
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn the_schema_stores_no_content_columns_and_enforces_expiry_and_immutability() {
    let db = TestDb::migrated().await;
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns WHERE table_schema=$1 AND table_name='usage_outbox' ORDER BY ordinal_position",
    )
    .bind(&db.schema)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    for forbidden in [
        "text",
        "schema",
        "output",
        "content",
        "secret",
        "prompt",
        "translation",
    ] {
        assert!(
            columns.iter().all(|c| !c.contains(forbidden)),
            "{forbidden}: {columns:?}"
        );
    }
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables WHERE table_schema=$1 ORDER BY 1",
    )
    .bind(&db.schema)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(tables, vec!["delitools_schema", "usage_outbox"]);

    let outbox = Outbox::new(db.pool.clone());
    let request = Uuid::now_v7();
    insert(&outbox, TENANT, request, 1, now_micros()).await;
    // Expiry must be exactly 30 days after creation.
    let wrong = sqlx::query("UPDATE usage_outbox SET expires_at=expires_at+interval '1 day'")
        .execute(&db.pool)
        .await;
    assert!(wrong.is_err(), "retries cannot extend the expiry");
    for column in [
        "quantity=quantity+1",
        "meter_key='other'",
        "occurred_at=now()",
        "created_at=now()",
        "event_key='x'",
        "tenant_id=gen_random_uuid()",
        "delivery_key=gen_random_uuid()",
    ] {
        let result = sqlx::query(&format!("UPDATE usage_outbox SET {column}"))
            .execute(&db.pool)
            .await;
        assert!(result.is_err(), "{column} must be immutable");
    }
    sqlx::query("UPDATE usage_outbox SET delivery_attempts=delivery_attempts+1,next_attempt_at=now()+interval '1 day',last_failure='unavailable'")
        .execute(&db.pool)
        .await
        .unwrap();
    let bad_insert = sqlx::query("INSERT INTO usage_outbox(id,tenant_id,request_id,attempt,direction,event_key,delivery_key,meter_key,quantity,occurred_at,state,next_attempt_at,created_at,expires_at) VALUES(gen_random_uuid(),gen_random_uuid(),gen_random_uuid(),1,'in','k2',gen_random_uuid(),'m',1,now(),'pending',now(),now(),now()+interval '31 days')")
        .execute(&db.pool)
        .await;
    assert!(bad_insert.is_err(), "expiry other than 30 days is rejected");
    let zero = sqlx::query("INSERT INTO usage_outbox(id,tenant_id,request_id,attempt,direction,event_key,delivery_key,meter_key,quantity,occurred_at,state,next_attempt_at,created_at,expires_at) VALUES(gen_random_uuid(),gen_random_uuid(),gen_random_uuid(),1,'in','k3',gen_random_uuid(),'m',0,now(),'pending',now(),now(),now()+interval '30 days')")
        .execute(&db.pool)
        .await;
    assert!(zero.is_err(), "zero quantities are not stored");
    db.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Outbox durability, restart and duplicate delivery
// ---------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn usage_survives_restart_and_event_keys_never_duplicate() {
    let db = TestDb::migrated().await;
    let outbox = Outbox::new(db.pool.clone());
    let request = Uuid::now_v7();
    let now = now_micros();
    insert(&outbox, TENANT, request, 1, now).await;
    // Storing the same attempt again (for example a repeated call) creates no second charge.
    insert(&outbox, TENANT, request, 1, now).await;
    assert_eq!(count(&db.pool, None).await, 2);

    // A restarted process sees the same pending rows with the same stable identities.
    db.pool.close().await;
    let restarted = db.reconnect().await;
    let rows = Outbox::new(restarted.clone())
        .due(now_micros(), 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let keys: Vec<_> = rows.iter().map(|r| r.event.event_key.clone()).collect();
    assert!(
        keys.contains(&format!("{request}:1:in")) && keys.contains(&format!("{request}:1:out"))
    );
    assert!(
        rows.iter()
            .all(|r| r.event.occurred_at == rows[0].event.occurred_at)
    );
    restarted.close().await;
    let admin = db.reconnect().await;
    sqlx::query(&format!("DROP SCHEMA {} CASCADE", db.schema))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn delivery_reports_stable_events_and_removes_accepted_rows() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    let outbox = Outbox::new(db.pool.clone());
    let request = Uuid::now_v7();
    let created = now_micros();
    insert(&outbox, TENANT, request, 2, created).await;
    let stats = delivery(&h, &db.pool).cycle(created).await.unwrap();
    assert_eq!(stats.pending, 0);
    assert_eq!(count(&db.pool, None).await, 0);
    let ingested = h.delcore.lock().unwrap().ingested.clone();
    assert_eq!(ingested.len(), 2);
    let input = ingested
        .iter()
        .find(|b| b["event"]["eventKey"] == format!("{request}:2:in"))
        .unwrap();
    assert_eq!(input["event"]["tenantId"], TENANT);
    assert_eq!(input["event"]["serviceId"], SERVICE_ID);
    assert_eq!(input["event"]["eventType"], "gem_in");
    assert_eq!(
        input["event"]["attributes"]["quantity"],
        json!({"number": "100"})
    );
    assert!(Uuid::parse_str(input["requestKey"].as_str().unwrap()).is_ok());
    assert_eq!(
        input["event"]["occurredAt"]
            .as_str()
            .unwrap()
            .parse::<DateTime<Utc>>()
            .unwrap()
            .timestamp_micros(),
        created.timestamp_micros()
    );
    let output = ingested
        .iter()
        .find(|b| b["event"]["eventKey"] == format!("{request}:2:out"))
        .unwrap();
    assert_eq!(
        output["event"]["attributes"]["quantity"],
        json!({"number": "40"})
    );
    assert!(
        h.delcore
            .lock()
            .unwrap()
            .scopes
            .iter()
            .all(|s| s == "authorize usage:write")
    );
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn failed_deliveries_retry_with_identical_identity_and_bounded_backoff() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    let outbox = Outbox::new(db.pool.clone());
    let request = Uuid::now_v7();
    let t0 = now_micros();
    insert(&outbox, TENANT, request, 1, t0).await;
    let worker = delivery(&h, &db.pool);
    {
        // Delcore accepted the first write but the response was lost: the caller sees a failure.
        let mut s = h.delcore.lock().unwrap();
        s.ingest_default = Some(Ingest::Error {
            status: 503,
            code: "unavailable",
            reason: None,
        });
    }
    let stats = worker.cycle(t0).await.unwrap();
    assert_eq!(stats.pending, 2);
    assert_eq!(h.delcore.lock().unwrap().ingested.len(), 2);
    // Within the backoff window nothing is sent again.
    worker.cycle(t0 + Duration::milliseconds(10)).await.unwrap();
    assert_eq!(h.delcore.lock().unwrap().ingested.len(), 2);
    let row = sqlx::query("SELECT delivery_attempts,last_failure,next_attempt_at FROM usage_outbox ORDER BY event_key LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(row.get::<i32, _>("delivery_attempts"), 1);
    assert_eq!(row.get::<String, _>("last_failure"), "unavailable");
    // Second delivery after the first backoff (50 ms) succeeds as an identical replay.
    h.delcore.lock().unwrap().ingest_default = None;
    worker.cycle(t0 + Duration::seconds(5)).await.unwrap();
    assert_eq!(count(&db.pool, None).await, 0);
    let ingested = h.delcore.lock().unwrap().ingested.clone();
    assert_eq!(ingested.len(), 4);
    for key in ["in", "out"] {
        let sent: Vec<&Value> = ingested
            .iter()
            .filter(|b| b["event"]["eventKey"] == format!("{request}:1:{key}"))
            .collect();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            sent[0], sent[1],
            "retries resend byte-identical identity and content"
        );
    }
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn backoff_grows_and_is_capped() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    let outbox = Outbox::new(db.pool.clone());
    let t0 = now_micros();
    insert(&outbox, TENANT, Uuid::now_v7(), 1, t0).await;
    h.delcore.lock().unwrap().ingest_default = Some(Ingest::Error {
        status: 503,
        code: "unavailable",
        reason: None,
    });
    let worker = delivery(&h, &db.pool);
    let mut now = t0;
    let mut waits = Vec::new();
    for _ in 0..6 {
        worker.cycle(now).await.unwrap();
        let next: DateTime<Utc> =
            sqlx::query_scalar("SELECT next_attempt_at FROM usage_outbox LIMIT 1")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        waits.push((next - now).num_milliseconds());
        now = next;
    }
    // Configured bounds: 50 ms minimum, 400 ms maximum.
    assert_eq!(waits, vec![50, 100, 200, 400, 400, 400]);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn identity_conflicts_are_poison_and_stop_delivery_until_expiry() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    let outbox = Outbox::new(db.pool.clone());
    let t0 = now_micros();
    insert(&outbox, TENANT, Uuid::now_v7(), 1, t0).await;
    {
        let mut s = h.delcore.lock().unwrap();
        s.ingest_script.push_back(Ingest::Error {
            status: 409,
            code: "already_exists",
            reason: Some(11),
        });
        s.ingest_script.push_back(Ingest::Ok);
    }
    let worker = delivery(&h, &db.pool);
    let stats = worker.cycle(t0).await.unwrap();
    assert_eq!(stats.poisoned, 1);
    assert_eq!(stats.pending, 0);
    let reason: String =
        sqlx::query_scalar("SELECT last_failure FROM usage_outbox WHERE state='poisoned'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(reason, "identity_conflict");
    // The poisoned row is never delivered again, however long we wait.
    worker.cycle(t0 + Duration::days(1)).await.unwrap();
    assert_eq!(count(&db.pool, Some("poisoned")).await, 1);
    let metrics = h.server.metrics.encode();
    assert!(metrics.contains("delitools_outbox_poisoned 1"));
    assert!(metrics.contains("delitools_usage_deliveries_total{outcome=\"poison\"} 1"));
    // It still expires 30 days after creation.
    worker.cycle(t0 + Duration::days(30)).await.unwrap();
    assert_eq!(count(&db.pool, None).await, 0);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn other_delcore_rejections_classify_as_retry_or_poison() {
    let cases = [
        (400, "invalid_argument", Some(1), "poisoned"),
        (400, "invalid_argument", Some(9), "poisoned"),
        (412, "failed_precondition", Some(14), "poisoned"),
        (400, "invalid_argument", Some(8), "pending"),
        (403, "permission_denied", Some(3), "pending"),
        (401, "unauthenticated", Some(2), "pending"),
        (500, "internal", None, "pending"),
    ];
    for (status, code, reason, state) in cases {
        let db = TestDb::migrated().await;
        let h = Harness::start(Some(db.pool.clone())).await;
        let t0 = now_micros();
        insert(&Outbox::new(db.pool.clone()), TENANT, Uuid::now_v7(), 1, t0).await;
        h.delcore.lock().unwrap().ingest_default = Some(Ingest::Error {
            status,
            code,
            reason,
        });
        delivery(&h, &db.pool).cycle(t0).await.unwrap();
        assert_eq!(count(&db.pool, Some(state)).await, 2, "{code} {reason:?}");
        db.finish().await;
    }
}

// ---------------------------------------------------------------------------------------------
// The 30-day first-creation expiry
// ---------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn undelivered_usage_expires_exactly_thirty_days_after_first_creation() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    let t0 = now_micros();
    insert(&Outbox::new(db.pool.clone()), TENANT, Uuid::now_v7(), 1, t0).await;
    h.delcore.lock().unwrap().ingest_default = Some(Ingest::Error {
        status: 503,
        code: "unavailable",
        reason: None,
    });
    let worker = delivery(&h, &db.pool);
    let expires: DateTime<Utc> = sqlx::query_scalar("SELECT expires_at FROM usage_outbox LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(expires, t0 + Duration::days(30));
    // Many failed deliveries and long backoffs do not move the expiry.
    let mut now = t0;
    for _ in 0..8 {
        worker.cycle(now).await.unwrap();
        now += Duration::days(3);
    }
    let after: DateTime<Utc> = sqlx::query_scalar("SELECT expires_at FROM usage_outbox LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(after, expires);
    assert_eq!(count(&db.pool, None).await, 2);
    // One microsecond before the boundary the row is still deliverable and kept.
    let before = h.delcore.lock().unwrap().ingested.len();
    let boundary = t0 + Duration::days(30);
    sqlx::query("UPDATE usage_outbox SET next_attempt_at=$1")
        .bind(boundary - Duration::microseconds(1))
        .execute(&db.pool)
        .await
        .unwrap();
    worker
        .cycle(boundary - Duration::microseconds(1))
        .await
        .unwrap();
    assert_eq!(count(&db.pool, None).await, 2);
    assert_eq!(h.delcore.lock().unwrap().ingested.len(), before + 2);
    // At the boundary the rows are deleted without another delivery attempt.
    sqlx::query("UPDATE usage_outbox SET next_attempt_at=$1")
        .bind(boundary)
        .execute(&db.pool)
        .await
        .unwrap();
    worker.cycle(boundary).await.unwrap();
    assert_eq!(count(&db.pool, None).await, 0);
    assert_eq!(h.delcore.lock().unwrap().ingested.len(), before + 2);
    assert!(
        h.server
            .metrics
            .encode()
            .contains("delitools_usage_expired_total 2")
    );
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn delivery_on_the_last_day_still_succeeds() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    let t0 = now_micros();
    insert(&Outbox::new(db.pool.clone()), TENANT, Uuid::now_v7(), 1, t0).await;
    sqlx::query("UPDATE usage_outbox SET next_attempt_at=$1")
        .bind(t0)
        .execute(&db.pool)
        .await
        .unwrap();
    delivery(&h, &db.pool)
        .cycle(t0 + Duration::days(29) + Duration::hours(23))
        .await
        .unwrap();
    assert_eq!(count(&db.pool, None).await, 0);
    assert_eq!(h.delcore.lock().unwrap().ingested.len(), 2);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn rows_of_finally_deleted_tenants_are_dropped_without_delivery() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    let t0 = now_micros();
    let outbox = Outbox::new(db.pool.clone());
    insert(&outbox, TENANT, Uuid::now_v7(), 1, t0).await;
    insert(&outbox, OTHER_TENANT, Uuid::now_v7(), 1, t0).await;
    h.delcore
        .lock()
        .unwrap()
        .tenant_states
        .insert(TENANT.into(), "TENANT_STATE_DELETED".into());
    delivery(&h, &db.pool).cycle(t0).await.unwrap();
    assert_eq!(count(&db.pool, None).await, 0);
    let ingested = h.delcore.lock().unwrap().ingested.clone();
    assert_eq!(ingested.len(), 2);
    assert!(
        ingested
            .iter()
            .all(|b| b["event"]["tenantId"] == OTHER_TENANT)
    );
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn deleting_and_blocked_tenants_keep_their_usage_pending() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    let t0 = now_micros();
    insert(&Outbox::new(db.pool.clone()), TENANT, Uuid::now_v7(), 1, t0).await;
    h.delcore
        .lock()
        .unwrap()
        .tenant_states
        .insert(TENANT.into(), "TENANT_STATE_DELETING".into());
    h.delcore.lock().unwrap().ingest_default = Some(Ingest::Error {
        status: 412,
        code: "failed_precondition",
        reason: Some(5),
    });
    delivery(&h, &db.pool).cycle(t0).await.unwrap();
    assert_eq!(count(&db.pool, Some("pending")).await, 2);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn backlog_alert_metrics_follow_thresholds() {
    let db = TestDb::migrated().await;
    let h = Harness::start_with(Some(db.pool.clone()), |c| {
        c.outbox.alert_backlog = 1;
    })
    .await;
    let t0 = now_micros();
    insert(&Outbox::new(db.pool.clone()), TENANT, Uuid::now_v7(), 1, t0).await;
    h.delcore.lock().unwrap().ingest_default = Some(Ingest::Error {
        status: 503,
        code: "unavailable",
        reason: None,
    });
    delivery(&h, &db.pool)
        .cycle(t0 + Duration::seconds(30))
        .await
        .unwrap();
    let metrics = h.server.metrics.encode();
    assert!(metrics.contains("delitools_outbox_pending 2"));
    assert!(metrics.contains("delitools_outbox_alert 1"));
    assert!(metrics.contains("delitools_outbox_oldest_pending_age_seconds 30"));
    db.finish().await;
}

// ---------------------------------------------------------------------------------------------
// End to end billing through the RPCs
// ---------------------------------------------------------------------------------------------

async fn rows(db: &sqlx::PgPool) -> Vec<(String, String, i64, i32)> {
    sqlx::query("SELECT event_key,meter_key,quantity,attempt FROM usage_outbox ORDER BY event_key")
        .fetch_all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| {
            (
                r.get("event_key"),
                r.get("meter_key"),
                r.get("quantity"),
                r.get("attempt"),
            )
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn provider_totals_are_billed_without_double_counting_breakdowns() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    h.gateway.push(Reply::Ok {
        content: r#"{"translations":["Bonjour"]}"#.into(),
        usage: json!({
            "prompt_tokens": 1000, "completion_tokens": 400, "total_tokens": 1400,
            "prompt_tokens_details": {"cached_tokens": 800},
            "completion_tokens_details": {"reasoning_tokens": 300}
        }),
    });
    let wire = h.translate(USER_TOKEN, translate_body(&["Hello"])).await;
    assert!(wire.ok(), "{}", wire.body);
    let id = wire.body["requestId"].as_str().unwrap().to_owned();
    let stored = rows(&db.pool).await;
    assert_eq!(
        stored,
        vec![
            (format!("{id}:1:in"), "gem_in".into(), 1000, 1),
            (format!("{id}:1:out"), "gem_out".into(), 400, 1),
        ]
    );
    // occurred_at is the execution start, not later than the row creation.
    let (occurred, created): (DateTime<Utc>, DateTime<Utc>) =
        sqlx::query_as("SELECT occurred_at,created_at FROM usage_outbox LIMIT 1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(occurred <= created && created - occurred < Duration::seconds(5));
    // Delivery reports exactly those quantities.
    h.server.delivery().cycle(now_micros()).await.unwrap();
    let sent: Vec<Value> = h
        .delcore
        .lock()
        .unwrap()
        .ingested
        .iter()
        .map(|b| b["event"]["attributes"]["quantity"]["number"].clone())
        .collect();
    assert_eq!(sent.len(), 2);
    assert!(sent.contains(&json!("1000")) && sent.contains(&json!("400")));
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn extraction_and_translation_share_the_catalog_meters_of_their_model() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    h.gateway
        .push(Reply::ok(&extracted(json!({"name": "Ada"})), 50, 10));
    assert!(h.extract(USER_TOKEN, extract_body()).await.ok());
    let stored = rows(&db.pool).await;
    assert!(stored.iter().any(|r| r.1 == "luna_in" && r.2 == 50));
    assert!(stored.iter().any(|r| r.1 == "luna_out" && r.2 == 10));
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn failed_results_with_confirmed_usage_are_still_billed() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    // Invalid translation output.
    h.gateway.push(Reply::ok("not json at all", 120, 12));
    let wire = h.translate(USER_TOKEN, translate_body(&["Hello"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_INVALID_OUTPUT");
    // Missing extraction information.
    h.gateway
        .push(Reply::ok(r#"{"status":"missing_information"}"#, 70, 7));
    let wire = h.extract(USER_TOKEN, extract_body()).await;
    assert_eq!(wire.reason(), "ERROR_REASON_EXTRACTION_FAILED");
    // Nonconforming extraction output.
    h.gateway
        .push(Reply::ok(&extracted(json!({"name": 5})), 30, 3));
    let wire = h.extract(USER_TOKEN, extract_body()).await;
    assert_eq!(wire.reason(), "ERROR_REASON_INVALID_OUTPUT");
    let total_in: i64 = rows(&db.pool)
        .await
        .iter()
        .filter(|r| r.1.ends_with("_in"))
        .map(|r| r.2)
        .sum();
    let total_out: i64 = rows(&db.pool)
        .await
        .iter()
        .filter(|r| r.1.ends_with("_out"))
        .map(|r| r.2)
        .sum();
    assert_eq!((total_in, total_out), (120 + 70 + 30, 12 + 7 + 3));
    assert_eq!(h.gateway.calls(), 3, "no repair retries");
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn unknown_or_invalid_usage_creates_no_charge_and_zero_tokens_create_no_event() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    h.gateway.push(Reply::without_usage(r#"["a"]"#));
    assert!(
        h.translate(USER_TOKEN, translate_body(&["Hello"]))
            .await
            .ok()
    );
    h.gateway.push(Reply::Ok {
        content: r#"["a"]"#.into(),
        usage: json!({"prompt_tokens": "many", "completion_tokens": 3}),
    });
    assert!(
        h.translate(USER_TOKEN, translate_body(&["Hello"]))
            .await
            .ok()
    );
    assert_eq!(count(&db.pool, None).await, 0);
    let metrics = h.server.metrics.encode();
    assert!(metrics.contains("delitools_unknown_usage_total{reason=\"missing\"} 1"));
    assert!(metrics.contains("delitools_unknown_usage_total{reason=\"invalid\"} 1"));
    h.gateway.push(Reply::ok(r#"["a"]"#, 0, 5));
    assert!(
        h.translate(USER_TOKEN, translate_body(&["Hello"]))
            .await
            .ok()
    );
    let stored = rows(&db.pool).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].1, "gem_out");
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn the_billed_attempt_number_follows_the_provider_attempt() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    h.gateway.push(Reply::Status(503));
    h.gateway.push(Reply::Status(429));
    h.gateway.push(Reply::ok(r#"["ok"]"#, 11, 2));
    let wire = h.translate(USER_TOKEN, translate_body(&["Hello"])).await;
    assert!(wire.ok());
    let id = wire.body["requestId"].as_str().unwrap();
    let stored = rows(&db.pool).await;
    assert_eq!(
        stored.len(),
        2,
        "failed attempts without usage are not billed"
    );
    assert!(
        stored
            .iter()
            .all(|r| r.3 == 3 && r.0.starts_with(&format!("{id}:3:")))
    );
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_canceled_caller_does_not_lose_usage_that_is_being_stored() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    h.gateway.push(Reply::ok(r#"["ok"]"#, 21, 4));
    // Hold the table so the storing step blocks while the caller disconnects.
    let mut blocker = db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE usage_outbox IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pending = tokio::spawn(h.call_future("Translate", USER_TOKEN, translate_body(&["Hello"])));
    for _ in 0..100 {
        if h.gateway.calls() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    pending.abort();
    let _ = pending.await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    blocker.commit().await.unwrap();
    for _ in 0..100 {
        if count(&db.pool, None).await == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        count(&db.pool, None).await,
        2,
        "confirmed usage is stored after cancellation"
    );
    assert_eq!(h.gateway.calls(), 1);
    for _ in 0..50 {
        if h.server.in_flight(TENANT) == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(h.server.in_flight(TENANT), 0);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn usage_that_cannot_be_stored_fails_the_request_loudly() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    sqlx::query("ALTER TABLE usage_outbox RENAME TO usage_outbox_away")
        .execute(&db.pool)
        .await
        .unwrap();
    h.gateway.push(Reply::ok(r#"["ok"]"#, 21, 4));
    let wire = h.translate(USER_TOKEN, translate_body(&["Hello"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_DEPENDENCY_UNAVAILABLE");
    assert!(
        h.server
            .metrics
            .encode()
            .contains("delitools_usage_persist_failures_total 1")
    );
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_restarted_instance_delivers_usage_recorded_before_the_restart() {
    let db = TestDb::migrated().await;
    let first = Harness::start(Some(db.pool.clone())).await;
    first.gateway.push(Reply::ok(r#"["ok"]"#, 9, 3));
    first.delcore.lock().unwrap().ingest_default = Some(Ingest::Error {
        status: 503,
        code: "unavailable",
        reason: None,
    });
    assert!(
        first
            .translate(USER_TOKEN, translate_body(&["Hello"]))
            .await
            .ok()
    );
    first.server.delivery().cycle(now_micros()).await.unwrap();
    assert_eq!(count(&db.pool, Some("pending")).await, 2);
    drop(first);
    let pool = db.reconnect().await;
    let second = Harness::start(Some(pool.clone())).await;
    // The new instance's worker runs in the background loop as in production.
    let worker = tokio::spawn(second.server.delivery().run());
    for _ in 0..100 {
        if count(&pool, None).await == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    worker.abort();
    assert_eq!(count(&pool, None).await, 0);
    assert_eq!(second.delcore.lock().unwrap().ingested.len(), 2);
    pool.close().await;
    db.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Privacy
// ---------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn submitted_content_never_reaches_the_database() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    let secret_text = "TOP-SECRET-MARKER-1f8c";
    h.gateway
        .push(Reply::ok(&format!(r#"["{secret_text}-translated"]"#), 5, 5));
    assert!(
        h.translate(USER_TOKEN, translate_body(&[secret_text]))
            .await
            .ok()
    );
    h.gateway
        .push(Reply::ok(&extracted(json!({"name": secret_text})), 5, 5));
    let mut body = extract_body();
    body["text"] = json!(secret_text);
    assert!(h.extract(USER_TOKEN, body).await.ok());
    let dump: Vec<String> = sqlx::query_scalar("SELECT row_to_json(t)::text FROM usage_outbox t")
        .fetch_all(&db.pool)
        .await
        .unwrap();
    assert!(!dump.is_empty());
    for row in dump {
        assert!(
            !row.contains(secret_text) && !row.contains(USER_TOKEN) && !row.contains("gw-secret"),
            "{row}"
        );
    }
    db.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Binary: commands, configuration failure and the single-instance lock
// ---------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn binary_commands_check_configuration_and_hold_one_instance_lock() {
    use std::process::{Command, Stdio};
    let db = TestDb::new().await;
    let h = Harness::start(None).await;
    let dir = std::env::temp_dir().join(format!("delitools-bin-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let models = dir.join("models.yaml");
    std::fs::write(
        &models,
        "models:\n  - model: openai/gpt-6-luna\n    input_meter_key: luna_in\n    output_meter_key: luna_out\n",
    )
    .unwrap();
    let free_port = || {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let command = |args: &[&str], port: u16| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_delitools-api"));
        c.args(args)
            .env_clear()
            .env("DATABASE_URL", db.scoped_url())
            .env("DELCORE_URL", h.config.delcore.url.clone())
            .env("LOGTO_TOKEN_URL", h.config.delcore.token_url.clone())
            .env("DELCORE_RESOURCE", "https://delcore.test")
            .env("CLIENT_ID", "svc-client")
            .env("CLIENT_SECRET", "svc-secret")
            .env("DELITOOLS_SERVICE_ID", SERVICE_ID)
            .env("AI_GATEWAY_API_KEY", "gw-secret")
            .env("AI_GATEWAY_BASE_URL", "http://127.0.0.1:9/v1")
            .env("DELITOOLS_MODELS", &models)
            .env("LISTEN", format!("127.0.0.1:{port}"))
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        c
    };
    // Unmigrated database: check-schema and serve both refuse.
    assert!(
        !command(&["check-schema"], 0)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        !command(&["serve"], free_port())
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(command(&["migrate"], 0).output().unwrap().status.success());
    assert!(
        command(&["check-schema"], 0)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!command(&["bogus"], 0).output().unwrap().status.success());
    // Invalid configuration stops serve before it listens.
    let mut bad = command(&["serve"], free_port());
    bad.env("DELITOOLS_MAX_RETRIES", "99");
    let out = bad.output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("DELITOOLS_MAX_RETRIES"));

    // Two instances: only one owns the lock.
    let port = free_port();
    let mut first = command(&["serve"], port).spawn().unwrap();
    let client = reqwest::Client::new();
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = client
            .get(format!("http://127.0.0.1:{port}/readyz"))
            .send()
            .await
            && r.status() == 200
        {
            ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(ready, "the first instance serves and reports ready");
    let second = command(&["serve"], free_port()).output().unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("instance lock"));
    let metrics = client
        .get(format!("http://127.0.0.1:{port}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(metrics.contains("delitools_outbox_pending"));
    first.kill().unwrap();
    let _ = first.wait();
    let _ = std::fs::remove_dir_all(&dir);
    db.finish().await;
}

#[test]
fn outbox_arc_is_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Arc<Outbox>>();
    check::<Delivery>();
}
