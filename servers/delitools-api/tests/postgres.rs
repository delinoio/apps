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
        fence: tokio_util::sync::CancellationToken::new(),
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
async fn unschedulable_retry_deadline_fails_the_cycle_and_keeps_the_row_pending() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    let outbox = Outbox::new(db.pool.clone());
    let t0 = now_micros();
    insert(&outbox, TENANT, Uuid::now_v7(), 1, t0).await;
    // Startup validation rejects these bounds; bypass it to prove the worker's own defense.
    let mut worker = delivery(&h, &db.pool);
    worker.settings.backoff_min = std::time::Duration::from_secs(10_000_000_000_000);
    worker.settings.backoff_max = worker.settings.backoff_min;
    h.delcore.lock().unwrap().ingest_default = Some(Ingest::Error {
        status: 503,
        code: "unavailable",
        reason: None,
    });
    let before: Vec<(String, i32, String)> = sqlx::query_as(
        "SELECT state,delivery_attempts,expires_at::text FROM usage_outbox ORDER BY event_key",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert!(matches!(
        worker.cycle(t0).await,
        Err(delitools_api::outbox::CycleError::Schedule(_))
    ));
    let after: Vec<(String, i32, String)> = sqlx::query_as(
        "SELECT state,delivery_attempts,expires_at::text FROM usage_outbox ORDER BY event_key",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        before, after,
        "the row is neither deleted, poisoned nor re-expired"
    );
    assert_eq!(count(&db.pool, Some("pending")).await, 2);
    // The next cycle with representable bounds runs normally.
    h.delcore.lock().unwrap().ingest_default = None;
    delivery(&h, &db.pool)
        .cycle(t0 + Duration::seconds(1))
        .await
        .unwrap();
    assert_eq!(count(&db.pool, None).await, 0);
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
async fn invalid_acknowledgments_keep_usage_pending_until_a_valid_one_arrives() {
    let id = VALID_EVENT_ID;
    let invalid = [
        json!({}),
        json!({"eventId": 42, "revision": "broken"}),
        json!({"eventId": "", "revision": 1}),
        json!({"eventId": null, "revision": 1}),
        json!({"eventId": "e", "revision": 1}),
        json!({"eventId": id}),
        json!({"eventId": id, "revision": null}),
        json!({"eventId": id, "revision": 0}),
        json!({"eventId": id, "revision": -1}),
        json!({"eventId": id, "revision": "one"}),
        json!({"eventId": id, "revision": 2}),
    ];
    const IDENTITY: &str = "SELECT event_key, delivery_key, meter_key, quantity, occurred_at, \
         created_at, expires_at FROM usage_outbox ORDER BY event_key";
    for body in invalid {
        let db = TestDb::migrated().await;
        let h = Harness::start(Some(db.pool.clone())).await;
        let outbox = Outbox::new(db.pool.clone());
        let request = Uuid::now_v7();
        let t0 = now_micros();
        insert(&outbox, TENANT, request, 1, t0).await;
        let before = sqlx::query(IDENTITY).fetch_all(&db.pool).await.unwrap();
        h.delcore.lock().unwrap().ingest_default = Some(Ingest::Raw {
            status: 200,
            body: body.clone(),
        });
        let worker = delivery(&h, &db.pool);
        let stats = worker.cycle(t0).await.unwrap();
        assert_eq!(stats.poisoned, 0, "{body}");
        assert_eq!(count(&db.pool, Some("pending")).await, 2, "{body}");
        assert_eq!(count(&db.pool, Some("poisoned")).await, 0, "{body}");
        let attempts = sqlx::query(
            "SELECT delivery_attempts, last_failure, next_attempt_at FROM usage_outbox",
        )
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(attempts.len(), 2);
        for row in &attempts {
            assert_eq!(row.get::<i32, _>("delivery_attempts"), 1, "{body}");
            assert_eq!(
                row.get::<String, _>("last_failure"),
                "malformed_response",
                "{body}"
            );
            // Configured minimum backoff is 50 ms.
            assert_eq!(
                (row.get::<DateTime<Utc>, _>("next_attempt_at") - t0).num_milliseconds(),
                50,
                "{body}"
            );
        }
        let after = sqlx::query(IDENTITY).fetch_all(&db.pool).await.unwrap();
        assert_eq!(before.len(), after.len());
        for (b, a) in before.iter().zip(&after) {
            assert_eq!(
                b.get::<String, _>("event_key"),
                a.get::<String, _>("event_key")
            );
            assert_eq!(
                b.get::<Uuid, _>("delivery_key"),
                a.get::<Uuid, _>("delivery_key")
            );
            assert_eq!(
                b.get::<String, _>("meter_key"),
                a.get::<String, _>("meter_key")
            );
            assert_eq!(b.get::<i64, _>("quantity"), a.get::<i64, _>("quantity"));
            for column in ["occurred_at", "created_at", "expires_at"] {
                assert_eq!(
                    b.get::<DateTime<Utc>, _>(column),
                    a.get::<DateTime<Utc>, _>(column),
                    "{column} {body}"
                );
            }
        }
        // The next due cycle resends identical identity and deletes only on a valid acknowledgment.
        h.delcore.lock().unwrap().ingest_default = Some(Ingest::Raw {
            status: 200,
            body: json!({"event_id": id, "revision": "1"}),
        });
        worker.cycle(t0 + Duration::seconds(5)).await.unwrap();
        assert_eq!(count(&db.pool, None).await, 0, "{body}");
        let ingested = h.delcore.lock().unwrap().ingested.clone();
        assert_eq!(ingested.len(), 4, "{body}");
        for key in ["in", "out"] {
            let sent: Vec<&Value> = ingested
                .iter()
                .filter(|b| b["event"]["eventKey"] == format!("{request}:1:{key}"))
                .collect();
            assert_eq!(sent.len(), 2);
            assert_eq!(sent[0], sent[1]);
        }
        db.finish().await;
    }
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

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn unrelated_error_details_never_poison_usage() {
    // Field 1 varint 1, 9, 11 and 14 collide with the permanent Delcore reasons.
    for value in ["CAE", "CAk", "CAs", "CA4"] {
        let db = TestDb::migrated().await;
        let h = Harness::start(Some(db.pool.clone())).await;
        let outbox = Outbox::new(db.pool.clone());
        let request = Uuid::now_v7();
        let t0 = now_micros();
        insert(&outbox, TENANT, request, 1, t0).await;
        let before = sqlx::query(
            "SELECT event_key, delivery_key, meter_key, quantity, occurred_at, created_at, expires_at \
             FROM usage_outbox ORDER BY event_key",
        )
        .fetch_all(&db.pool)
        .await
        .unwrap();
        h.delcore.lock().unwrap().ingest_default = Some(Ingest::Raw {
            status: 503,
            body: json!({"code":"unavailable","details":[{"type":"other.v1.Failure","value":value}]}),
        });
        let worker = delivery(&h, &db.pool);
        let stats = worker.cycle(t0).await.unwrap();
        assert_eq!(stats.poisoned, 0, "{value}");
        assert_eq!(count(&db.pool, Some("pending")).await, 2, "{value}");
        assert_eq!(count(&db.pool, Some("poisoned")).await, 0, "{value}");
        let rows = sqlx::query(
            "SELECT delivery_attempts, last_failure, next_attempt_at FROM usage_outbox",
        )
        .fetch_all(&db.pool)
        .await
        .unwrap();
        for row in &rows {
            assert_eq!(row.get::<i32, _>("delivery_attempts"), 1);
            assert_eq!(row.get::<String, _>("last_failure"), "unavailable");
            // Configured minimum backoff is 50 ms.
            assert_eq!(
                (row.get::<DateTime<Utc>, _>("next_attempt_at") - t0).num_milliseconds(),
                50
            );
        }
        let after = sqlx::query(
            "SELECT event_key, delivery_key, meter_key, quantity, occurred_at, created_at, expires_at \
             FROM usage_outbox ORDER BY event_key",
        )
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(before.len(), after.len());
        for (b, a) in before.iter().zip(&after) {
            assert_eq!(
                b.get::<String, _>("event_key"),
                a.get::<String, _>("event_key")
            );
            assert_eq!(
                b.get::<Uuid, _>("delivery_key"),
                a.get::<Uuid, _>("delivery_key")
            );
            assert_eq!(
                b.get::<String, _>("meter_key"),
                a.get::<String, _>("meter_key")
            );
            assert_eq!(b.get::<i64, _>("quantity"), a.get::<i64, _>("quantity"));
            for column in ["occurred_at", "created_at", "expires_at"] {
                assert_eq!(
                    b.get::<DateTime<Utc>, _>(column),
                    a.get::<DateTime<Utc>, _>(column),
                    "{column}"
                );
            }
        }
        // The next due attempt delivers once Delcore accepts the events.
        h.delcore.lock().unwrap().ingest_default = None;
        worker.cycle(t0 + Duration::seconds(5)).await.unwrap();
        assert_eq!(count(&db.pool, None).await, 0, "{value}");
        db.finish().await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_typed_permanent_reason_after_an_unrelated_detail_still_poisons() {
    let db = TestDb::migrated().await;
    let h = Harness::start(Some(db.pool.clone())).await;
    let t0 = now_micros();
    insert(&Outbox::new(db.pool.clone()), TENANT, Uuid::now_v7(), 1, t0).await;
    // `CAI` (reason 2, retryable) is unrelated and skipped; the typed detail wins.
    // `CAs` is field 1 = 11, `EVENT_IDENTITY_CONFLICT`, under the exact Delcore type.
    let typed = "CAs";
    h.delcore.lock().unwrap().ingest_default = Some(Ingest::Raw {
        status: 409,
        body: json!({"code":"already_exists","details":[
            {"type":"other.v1.Failure","value":"CAI"},
            {"type":"delcore.v1.ErrorDetail","value":typed},
        ]}),
    });
    let worker = delivery(&h, &db.pool);
    let stats = worker.cycle(t0).await.unwrap();
    assert_eq!(stats.pending, 0);
    assert_eq!(count(&db.pool, Some("poisoned")).await, 2);
    let reason: String =
        sqlx::query_scalar("SELECT DISTINCT last_failure FROM usage_outbox WHERE state='poisoned'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(reason, "identity_conflict");
    // Poisoned rows are excluded from later due selection.
    let calls = h.delcore.lock().unwrap().ingested.len();
    worker.cycle(t0 + Duration::days(1)).await.unwrap();
    assert_eq!(h.delcore.lock().unwrap().ingested.len(), calls);
    db.finish().await;
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
    let _serial = SERVE.lock().await;
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

// ---------------------------------------------------------------------------------------------
// Instance fence: lock loss versus ordinary shutdown
// ---------------------------------------------------------------------------------------------

async fn eventually<F, Fut>(what: &str, seconds: u64, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while std::time::Instant::now() < limit {
        if check().await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn confirmed_usage_is_stored_after_the_fence_within_the_bound() {
    let db = TestDb::migrated().await;
    let h = Harness::start_with(Some(db.pool.clone()), |c| {
        c.dependency_timeout = std::time::Duration::from_secs(3)
    })
    .await;
    h.gateway.push(Reply::ok(r#"["ok"]"#, 21, 4));
    let mut blocker = db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE usage_outbox IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pending = tokio::spawn(h.call_future("Translate", USER_TOKEN, translate_body(&["Hello"])));
    eventually("the confirmed provider response", 5, || async {
        h.gateway.calls() == 1
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    h.server.fence();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    blocker.commit().await.unwrap();
    let response = pending.await.unwrap().unwrap();
    assert_eq!(
        response.status(),
        200,
        "confirmed usage finalizes after the fence"
    );
    assert_eq!(
        count(&db.pool, Some("pending")).await,
        2,
        "both directions are stored for the replacement to deliver"
    );
    assert_eq!(h.gateway.calls(), 1);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn usage_finalization_after_the_fence_is_bounded() {
    let db = TestDb::migrated().await;
    let h = Harness::start_with(Some(db.pool.clone()), |c| {
        c.dependency_timeout = std::time::Duration::from_millis(400)
    })
    .await;
    h.gateway.push(Reply::ok(r#"["ok"]"#, 21, 4));
    h.gateway.push(Reply::ok(r#"["never"]"#, 1, 1));
    let mut blocker = db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE usage_outbox IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pending = tokio::spawn(h.call_future("Translate", USER_TOKEN, translate_body(&["Hello"])));
    eventually("the confirmed provider response", 5, || async {
        h.gateway.calls() == 1
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let fenced = std::time::Instant::now();
    h.server.fence();
    let response = tokio::time::timeout(std::time::Duration::from_secs(3), pending)
        .await
        .expect("the blocked finalization ends within the bound")
        .unwrap()
        .unwrap();
    assert!(fenced.elapsed() < std::time::Duration::from_secs(2));
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "unavailable");
    let metrics = h.server.metrics.encode();
    assert!(
        metrics.contains("delitools_usage_persist_failures_total 1"),
        "{metrics}"
    );
    assert_eq!(h.gateway.calls(), 1, "no provider work restarts");
    blocker.rollback().await.unwrap();
    assert_eq!(count(&db.pool, None).await, 0);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn the_fence_stops_delivery_without_the_transport_and_the_replacement_delivers() {
    let db = TestDb::migrated().await;
    let a = Harness::start(Some(db.pool.clone())).await;
    let barrier = IngestBarrier::new();
    a.delcore.lock().unwrap().ingest_barrier = Some(barrier.clone());
    let outbox = Outbox::new(db.pool.clone());
    let request = Uuid::now_v7();
    insert(&outbox, TENANT, request, 1, now_micros()).await;
    let keys: Vec<(String, String)> = sqlx::query(
        "SELECT event_key,delivery_key::text AS delivery_key FROM usage_outbox ORDER BY event_key",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|r| (r.get("event_key"), r.get("delivery_key")))
    .collect();
    let worker = tokio::spawn(a.server.delivery().run());
    eventually("the first ingest to be in flight", 5, || async {
        barrier.entered.load(std::sync::atomic::Ordering::SeqCst) == 1
    })
    .await;
    a.server.fence();
    // The worker ends by itself: no abort and no transport are involved.
    tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .expect("the fenced worker stops")
        .unwrap();
    eventually("the ingest connection to drop", 2, || async {
        barrier.dropped.load(std::sync::atomic::Ordering::SeqCst) == 1
    })
    .await;
    barrier.open();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        barrier.entered.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "no further IngestEvent starts after the fence"
    );
    assert!(a.delcore.lock().unwrap().ingested.is_empty());
    assert_eq!(count(&db.pool, Some("pending")).await, 2);
    // A fenced cycle does nothing even when called directly.
    a.server.delivery().cycle(now_micros()).await.unwrap();
    assert!(a.delcore.lock().unwrap().ingested.is_empty());

    // The replacement delivers the retained rows with their original identities.
    let b = Harness::start(Some(db.pool.clone())).await;
    b.server.delivery().cycle(now_micros()).await.unwrap();
    assert_eq!(count(&db.pool, None).await, 0);
    let delivered: Vec<(String, String)> = b
        .delcore
        .lock()
        .unwrap()
        .ingested
        .iter()
        .map(|body| {
            (
                body["event"]["eventKey"].as_str().unwrap().to_owned(),
                body["requestKey"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut delivered = delivered;
    delivered.sort();
    assert_eq!(delivered, keys);
    assert!(keys[0].0.starts_with(&request.to_string()));
    db.finish().await;
}

/// Binary tests share one database-wide advisory lock, so they run one at a time.
static SERVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A running `delitools-api serve` process with captured logs.
struct Instance {
    child: std::process::Child,
    port: u16,
    logs: Arc<std::sync::Mutex<String>>,
}

impl Instance {
    fn spawn(db: &TestDb, mocks: &Harness, dir: &std::path::Path, extra: &[(&str, &str)]) -> Self {
        use std::{
            io::Read,
            process::{Command, Stdio},
        };
        let models = dir.join("models.yaml");
        std::fs::write(
            &models,
            "models:\n  - model: google/gemini-translate\n    input_meter_key: gem_in\n    output_meter_key: gem_out\n  - model: openai/gpt-6-luna\n    input_meter_key: luna_in\n    output_meter_key: luna_out\n",
        )
        .unwrap();
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let mut command = Command::new(env!("CARGO_BIN_EXE_delitools-api"));
        command
            .arg("serve")
            .env_clear()
            .env("DATABASE_URL", db.scoped_url())
            .env("DELCORE_URL", mocks.config.delcore.url.clone())
            .env("LOGTO_TOKEN_URL", mocks.config.delcore.token_url.clone())
            .env("DELCORE_RESOURCE", "https://delcore.test")
            .env("CLIENT_ID", "svc-client")
            .env("CLIENT_SECRET", "svc-secret")
            .env("DELITOOLS_SERVICE_ID", SERVICE_ID)
            .env("AI_GATEWAY_API_KEY", "gw-secret")
            .env("AI_GATEWAY_BASE_URL", mocks.config.gateway.base_url.clone())
            .env("DELITOOLS_MODELS", &models)
            .env("LISTEN", format!("127.0.0.1:{port}"))
            .env("DELITOOLS_RETRY_BASE_DELAY", "10ms")
            .env("DELITOOLS_RETRY_MAX_DELAY", "20ms")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in extra {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
        let logs = Arc::new(std::sync::Mutex::new(String::new()));
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        for mut stream in [
            Box::new(stdout) as Box<dyn Read + Send>,
            Box::new(stderr) as Box<dyn Read + Send>,
        ] {
            let logs = logs.clone();
            std::thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                while let Ok(n) = stream.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    logs.lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&buffer[..n]));
                }
            });
        }
        Self { child, port, logs }
    }

    fn logged(&self, needle: &str) -> bool {
        self.logs.lock().unwrap().contains(needle)
    }

    async fn ready(&self) {
        let client = reqwest::Client::new();
        let limit = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < limit {
            if matches!(
                client
                    .get(format!("http://127.0.0.1:{}/readyz", self.port))
                    .send()
                    .await,
                Ok(r) if r.status() == 200
            ) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!(
            "the instance did not become ready:\n{}",
            self.logs.lock().unwrap()
        );
    }

    fn translate(&self) -> tokio::task::JoinHandle<Value> {
        let url = format!(
            "http://127.0.0.1:{}/delitools.v1.DelitoolsService/Translate",
            self.port
        );
        tokio::spawn(async move {
            reqwest::Client::new()
                .post(url)
                .header("Content-Type", "application/json")
                .header("Connect-Protocol-Version", "1")
                .bearer_auth(USER_TOKEN)
                .json(&translate_body(&["Hello"]))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap()
        })
    }

    /// Waits for the process to exit.
    async fn exit(&mut self, seconds: u64) -> std::process::ExitStatus {
        let limit = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                std::time::Instant::now() < limit,
                "the process did not exit"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Terminates only the backend that holds the instance advisory lock; returns its PID.
async fn terminate_lock_backend(db: &TestDb) -> i32 {
    let pid: i32 = sqlx::query_scalar(
        "SELECT l.pid FROM pg_locks l JOIN pg_database d ON d.oid=l.database
         WHERE l.locktype='advisory' AND l.granted AND l.objid=$1 AND d.datname=current_database()",
    )
    .bind(store::INSTANCE_LOCK as i32)
    .fetch_one(&db.pool)
    .await
    .expect("one granted instance lock");
    let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(pid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(terminated);
    pid
}

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("delitools-fence-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn binary_lock_loss_cancels_the_held_provider_call_and_hands_over() {
    let _serial = SERVE.lock().await;
    let db = TestDb::migrated().await;
    let (a_mocks, b_mocks) = (Harness::start(None).await, Harness::start(None).await);
    let dir = scratch_dir();
    // The execution timeout is far above every assertion bound: only the fence can end the call.
    let mut a = Instance::spawn(
        &db,
        &a_mocks,
        &dir,
        &[("DELITOOLS_EXECUTION_TIMEOUT", "30s")],
    );
    a.ready().await;
    a_mocks.gateway.push(Reply::Hang);
    let request = a.translate();
    eventually("the held Gateway request", 10, || async {
        a_mocks.gateway.calls() == 1
    })
    .await;

    terminate_lock_backend(&db).await;
    if !a.logged("instance lock session lost") {
        // Only the lock session ended: A's pool still reaches the database.
        let ready = reqwest::get(format!("http://127.0.0.1:{}/readyz", a.port))
            .await
            .unwrap();
        assert_eq!(ready.status(), 200);
    }
    // The replacement acquires the released lock and serves while A still drains.
    let b = Instance::spawn(&db, &b_mocks, &dir, &[]);
    b.ready().await;
    eventually("A to detect the loss", 15, || async {
        a.logged("instance lock session lost")
    })
    .await;
    let detected = std::time::Instant::now();
    eventually("the provider connection to drop", 3, || async {
        a_mocks.gateway.dropped() == 1
    })
    .await;
    assert!(
        detected.elapsed() < std::time::Duration::from_secs(1),
        "the provider call ends promptly, not at the execution timeout"
    );
    let body = tokio::time::timeout(std::time::Duration::from_secs(2), request)
        .await
        .expect("the held request ends")
        .unwrap();
    assert_eq!(body["code"], "canceled");
    let status = a.exit(10).await;
    assert_eq!(status.code(), Some(1), "lock loss keeps the nonzero exit");
    assert_eq!(a_mocks.gateway.calls(), 1);
    assert_eq!(a_mocks.delcore.lock().unwrap().ingested.len(), 0);
    // The replacement keeps serving.
    b_mocks.gateway.push(Reply::without_usage(r#"["salut"]"#));
    let served = b.translate().await.unwrap();
    assert_eq!(served["translations"][0], "salut");
    drop(b);
    let _ = std::fs::remove_dir_all(&dir);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn binary_lock_loss_during_retry_backoff_starts_no_attempt() {
    let _serial = SERVE.lock().await;
    let db = TestDb::migrated().await;
    let mocks = Harness::start(None).await;
    let dir = scratch_dir();
    let mut a = Instance::spawn(
        &db,
        &mocks,
        &dir,
        &[
            ("DELITOOLS_EXECUTION_TIMEOUT", "60s"),
            ("DELITOOLS_RETRY_BASE_DELAY", "30s"),
            ("DELITOOLS_RETRY_MAX_DELAY", "30s"),
        ],
    );
    a.ready().await;
    // A transient failure leaves a retry with authorization still allowed and budget remaining.
    mocks.gateway.push(Reply::Status(503));
    mocks.gateway.push(Reply::ok(r#"["never"]"#, 5, 5));
    let request = a.translate();
    eventually("the first attempt", 10, || async {
        mocks.gateway.calls() == 1
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let authorizations = mocks.delcore.lock().unwrap().authorize_calls.len();
    terminate_lock_backend(&db).await;
    eventually("A to detect the loss", 15, || async {
        a.logged("instance lock session lost")
    })
    .await;
    let body = tokio::time::timeout(std::time::Duration::from_secs(2), request)
        .await
        .expect("the backoff ends at the fence")
        .unwrap();
    assert_eq!(body["code"], "canceled");
    assert_eq!(mocks.gateway.calls(), 1, "no attempt starts after the loss");
    assert_eq!(
        mocks.delcore.lock().unwrap().authorize_calls.len(),
        authorizations,
        "no retry authorization runs after the loss"
    );
    assert_eq!(a.exit(10).await.code(), Some(1));
    assert_eq!(count(&db.pool, None).await, 0, "no usage is estimated");
    let _ = std::fs::remove_dir_all(&dir);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn binary_lock_loss_stops_delivery_and_the_replacement_delivers() {
    let _serial = SERVE.lock().await;
    let db = TestDb::migrated().await;
    let (a_mocks, b_mocks) = (Harness::start(None).await, Harness::start(None).await);
    let barrier = IngestBarrier::new();
    a_mocks.delcore.lock().unwrap().ingest_barrier = Some(barrier.clone());
    let request = Uuid::now_v7();
    insert(
        &Outbox::new(db.pool.clone()),
        TENANT,
        request,
        1,
        now_micros(),
    )
    .await;
    let dir = scratch_dir();
    let interval = [("DELITOOLS_OUTBOX_INTERVAL", "200ms")];
    // A's Delcore timeout exceeds the monitor period, so only the fence can end the held ingest.
    let mut a = Instance::spawn(
        &db,
        &a_mocks,
        &dir,
        &[
            ("DELITOOLS_OUTBOX_INTERVAL", "200ms"),
            ("DEPENDENCY_TIMEOUT", "30s"),
            ("DELITOOLS_EXECUTION_TIMEOUT", "60s"),
        ],
    );
    a.ready().await;
    // A held request keeps A's transport drain open, so delivery must stop without waiting for it.
    a_mocks.gateway.push(Reply::Hang);
    let held = a.translate();
    eventually("A's ingest to be in flight", 15, || async {
        barrier.entered.load(std::sync::atomic::Ordering::SeqCst) == 1
            && a_mocks.gateway.calls() == 1
    })
    .await;
    terminate_lock_backend(&db).await;
    let b = Instance::spawn(&db, &b_mocks, &dir, &interval);
    b.ready().await;
    eventually("A to detect the loss", 15, || async {
        a.logged("instance lock session lost")
    })
    .await;
    eventually("A's ingest connection to drop", 2, || async {
        barrier.dropped.load(std::sync::atomic::Ordering::SeqCst) == 1
    })
    .await;
    barrier.open();
    // B delivers both rows; A, still draining or exiting, dispatches nothing more.
    eventually("the replacement to deliver", 15, || async {
        count(&db.pool, None).await == 0
    })
    .await;
    let held = tokio::time::timeout(std::time::Duration::from_secs(2), held)
        .await
        .expect("the held request ends")
        .unwrap();
    assert_eq!(held["code"], "canceled");
    assert_eq!(a.exit(10).await.code(), Some(1));
    assert_eq!(barrier.entered.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(a_mocks.delcore.lock().unwrap().ingested.is_empty());
    let delivered = b_mocks.delcore.lock().unwrap().ingested.len();
    assert_eq!(delivered, 2, "each retained event is delivered once by B");
    let _ = std::fs::remove_dir_all(&dir);
    drop(b);
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn binary_sigterm_with_an_intact_lock_keeps_the_bounded_drain() {
    let _serial = SERVE.lock().await;
    let db = TestDb::migrated().await;
    let (a_mocks, b_mocks) = (Harness::start(None).await, Harness::start(None).await);
    let dir = scratch_dir();
    // Delivery stays idle after its immediate first cycle so the stored rows remain visible.
    let mut a = Instance::spawn(
        &db,
        &a_mocks,
        &dir,
        &[
            ("DELITOOLS_EXECUTION_TIMEOUT", "30s"),
            ("DELITOOLS_OUTBOX_INTERVAL", "1h"),
        ],
    );
    a.ready().await;
    a_mocks.gateway.push(Reply::Delay(
        1500,
        Box::new(Reply::ok(r#"["bonjour"]"#, 21, 4)),
    ));
    let request = a.translate();
    eventually("the slow Gateway request", 10, || async {
        a_mocks.gateway.calls() == 1
    })
    .await;
    let signaled = std::process::Command::new("kill")
        .args(["-TERM", &a.child.id().to_string()])
        .status()
        .unwrap();
    assert!(signaled.success());
    // While A drains it still owns the lock: a second instance cannot start.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        a.child.try_wait().unwrap().is_none(),
        "A drains the request"
    );
    let blocked = Instance::spawn(&db, &b_mocks, &dir, &[]);
    let mut blocked = blocked;
    assert!(!blocked.exit(10).await.success());
    assert!(blocked.logged("instance lock"));
    // The slow request completes normally and its confirmed usage is stored.
    let body = tokio::time::timeout(std::time::Duration::from_secs(5), request)
        .await
        .expect("the drained request completes")
        .unwrap();
    assert_eq!(body["translations"][0], "bonjour");
    assert_eq!(a_mocks.gateway.dropped(), 0);
    let status = a.exit(10).await;
    assert!(status.success(), "an ordinary shutdown exits cleanly");
    assert!(!a.logged("instance lock session lost"));
    assert_eq!(count(&db.pool, Some("pending")).await, 2);
    // After A exits the lock is free.
    let b = Instance::spawn(&db, &b_mocks, &dir, &[]);
    b.ready().await;
    drop(b);
    let _ = std::fs::remove_dir_all(&dir);
    db.finish().await;
}

#[test]
fn outbox_arc_is_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Arc<Outbox>>();
    check::<Delivery>();
}
