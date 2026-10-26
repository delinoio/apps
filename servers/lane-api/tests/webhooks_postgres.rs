// SPDX-License-Identifier: FSL-1.1-ALv2
//! Component integration tests use real PostgreSQL and the real entity domain.
//! Run with LANE_TEST_DATABASE_URL pointing to a disposable PostgreSQL database.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use lane_api::{
    Principal, account_lock, entities, id, string,
    webhooks::{self, WebhookConfig},
};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::{Connection, PgConnection, Row};

struct Fixture {
    db: PgConnection,
    schema: String,
    principal: Principal,
    config: WebhookConfig,
    repository: String,
    receiver: String,
    secret: String,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("LANE_TEST_DATABASE_URL")
            .expect("set LANE_TEST_DATABASE_URL to a disposable PostgreSQL database");
        let mut db = PgConnection::connect(&url).await.unwrap();
        let schema = format!("webhooks_{}", id().replace('-', ""));
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&mut db)
            .await
            .unwrap();
        sqlx::query(&format!("SET search_path TO {schema}"))
            .execute(&mut db)
            .await
            .unwrap();
        // The auth subsystem is outside this component. Intake needs only account routing.
        sqlx::raw_sql("CREATE TABLE lane_accounts(account_id text PRIMARY KEY,tenant_id text NOT NULL,epoch text NOT NULL)").execute(&mut db).await.unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for name in ["00002_entities.sql", "00003_webhooks.sql"] {
            let path = if name == "00002_entities.sql" {
                std::env::var("LANE_TEST_ENTITIES_MIGRATION")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| root.join("migrations").join(name))
            } else {
                root.join("migrations").join(name)
            };
            let sql = std::fs::read_to_string(path).unwrap();
            sqlx::raw_sql(&sql).execute(&mut db).await.unwrap();
        }
        // Entity task creation integrates push; its real schema is needed even with no devices.
        let push = std::env::var("LANE_TEST_PUSH_MIGRATION")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| root.join("migrations/00004_push.sql"));
        if push.exists() {
            sqlx::raw_sql(&std::fs::read_to_string(push).unwrap())
                .execute(&mut db)
                .await
                .unwrap();
        }
        sqlx::raw_sql(include_str!("../migrations/00006_captures.sql"))
            .execute(&mut db)
            .await
            .unwrap();
        let principal = Principal {
            account_id: id(),
            tenant_id: id(),
            session_id: id(),
            device_id: id(),
            device_name: "Webhook test".into(),
            origin_kind: "ORIGIN_KIND_CLI".into(),
        };
        sqlx::query("INSERT INTO lane_accounts VALUES($1,$2,$3)")
            .bind(&principal.account_id)
            .bind(&principal.tenant_id)
            .bind(id())
            .execute(&mut db)
            .await
            .unwrap();
        let config = WebhookConfig::new(
            &URL_SAFE_NO_PAD.encode([8; 32]),
            "https://lane.example/hooks",
            1024 * 1024,
        )
        .unwrap();
        let repository = id();
        let receiver = id();
        let mut tx = db.begin().await.unwrap();
        account_lock(&mut tx, &principal.account_id).await.unwrap();
        entities::dispatch(&mut tx,&principal,"CreateRepository",&json!({"meta":{"requestId":id()},"id":repository,"url":"https://example.com/test/repo"}),usize::MAX).await.unwrap();
        let created=webhooks::dispatch_with_config(&mut tx,&principal,"CreateReceiver",&json!({"meta":{"requestId":id()},"id":receiver,"repositoryId":repository,"name":"test"}),usize::MAX,Some(&config)).await.unwrap();
        tx.commit().await.unwrap();
        Self {
            db,
            schema,
            principal,
            config,
            repository,
            receiver,
            secret: string(&created, "secret").into(),
        }
    }
    async fn call(&mut self, method: &str, q: &Value) -> lane_api::Result<Value> {
        let mut tx = self.db.begin().await?;
        account_lock(&mut tx, &self.principal.account_id).await?;
        let result = webhooks::dispatch_with_config(
            &mut tx,
            &self.principal,
            method,
            q,
            1024 * 1024,
            Some(&self.config),
        )
        .await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(e) => {
                tx.rollback().await?;
                Err(e)
            }
        }
    }
    async fn intake_raw(&mut self, raw: &[u8]) -> lane_api::Result<Value> {
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let signature = sign(&self.secret, raw, &timestamp);
        let mut tx = self.db.begin().await?;
        let result = webhooks::intake(
            &mut tx,
            &self.receiver,
            raw,
            &timestamp,
            &signature,
            &self.config,
        )
        .await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(e) => {
                tx.rollback().await?;
                Err(e)
            }
        }
    }
    async fn event(&mut self) -> String {
        let raw = serde_json::to_vec(
            &json!({"id":id(),"type":"test","occurredAt":"2026-10-25T00:00:00Z","payload":{"x":1}}),
        )
        .unwrap();
        string(&self.intake_raw(&raw).await.unwrap(), "id").into()
    }
    async fn claim(&mut self, event: &str) -> String {
        string(
            &self.call("ClaimEvent", &mutation(event)).await.unwrap(),
            "leaseToken",
        )
        .into()
    }
    async fn close(mut self) {
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&mut self.db)
            .await
            .unwrap();
    }
}
fn mutation(id: &str) -> Value {
    json!({"meta":{"requestId":lane_api::id()},"id":id})
}
fn sign(secret: &str, raw: &[u8], timestamp: &str) -> String {
    let key = URL_SAFE_NO_PAD.decode(secret).unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(raw);
    format!("v1={}", hex::encode(mac.finalize().into_bytes()))
}
fn task(repository: &str, title: &str) -> Value {
    json!({"meta":{"requestId":id()},"id":id(),"repository":{"id":repository},"title":title})
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn intake_canonical_replay_rotation_and_redaction() {
    let mut f = Fixture::new().await;
    let raw=br#"{"id":"sender","type":"build","occurredAt":"2026-10-25T00:00:00Z","payload":{"b":2,"a":1}}"#;
    let result = f.intake_raw(raw).await.unwrap();
    let event = string(&result, "id").to_owned();
    let equivalent=br#"{"payload":{"a":1.0,"b":2},"occurredAt":"2026-10-25T09:00:00.000000+09:00","type":"build","id":"sender"}"#;
    assert_eq!(f.intake_raw(equivalent).await.unwrap()["id"], event);
    assert!(f.intake_raw(br#"{"id":"sender","type":"build","occurredAt":"2026-10-25T00:00:00Z","payload":{"b":3}}"#).await.is_err());
    let mut rotate = mutation(&f.receiver);
    rotate["expectedRevision"] = json!("1");
    let rotated = f.call("RotateReceiverKey", &rotate).await.unwrap();
    assert!(
        f.call("RotateReceiverKey", &rotate)
            .await
            .unwrap()
            .get("secret")
            .is_none()
    );
    assert!(f.intake_raw(raw).await.is_err());
    f.secret = string(&rotated, "secret").into();
    assert_eq!(f.intake_raw(raw).await.unwrap()["id"], event);
    let receipts: Vec<Value> = sqlx::query_scalar("SELECT response FROM lane_webhook_receipts")
        .fetch_all(&mut f.db)
        .await
        .unwrap();
    for response in receipts {
        assert!(!response.to_string().contains(&f.secret));
        assert!(response.get("secret").is_none());
    }
    f.call("DeleteEvent", &mutation(&event)).await.unwrap();
    let replay = f.intake_raw(raw).await.unwrap();
    assert_eq!(replay["deleted"], true);
    assert_eq!(replay["id"], event);
    assert!(f.call("GetEvent", &json!({"id":event})).await.is_err());
    let row = sqlx::query("SELECT data,lease_hash,last_error FROM lane_webhook_events WHERE id=$1")
        .bind(event)
        .fetch_one(&mut f.db)
        .await
        .unwrap();
    assert!(row.get::<Option<Value>, _>("data").is_none());
    assert!(row.get::<Option<String>, _>("lease_hash").is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn completion_is_atomic_and_replay_survives_reopen_and_deletion() {
    let mut f = Fixture::new().await;
    let event = f.event().await;
    let token = f.claim(&event).await;
    let mut q = mutation(&event);
    q["leaseToken"] = json!(token);
    q["tasks"] = json!([task(&f.repository, "first"), task(&f.repository, "")]);
    // Deliberately commit the caller transaction after catching the domain error. The completion
    // savepoint must remove the first task, history, receipts and changes despite this misuse.
    let mut tx = f.db.begin().await.unwrap();
    account_lock(&mut tx, &f.principal.account_id)
        .await
        .unwrap();
    assert!(
        webhooks::dispatch_with_config(
            &mut tx,
            &f.principal,
            "CompleteEvent",
            &q,
            usize::MAX,
            Some(&f.config)
        )
        .await
        .is_err()
    );
    tx.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM lane_entities WHERE kind=2")
            .fetch_one(&mut f.db)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM lane_webhook_processing")
            .fetch_one(&mut f.db)
            .await
            .unwrap(),
        0
    );
    q["tasks"][1]["title"] = json!("second");
    let completed = f.call("CompleteEvent", &q).await.unwrap();
    assert_eq!(completed["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(f.call("CompleteEvent", &q).await.unwrap(), completed);
    let mut reopen = mutation(&event);
    reopen["expectedRevision"] = completed["event"]["revision"].clone();
    f.call("ReopenEvent", &reopen).await.unwrap();
    assert_eq!(f.call("CompleteEvent", &q).await.unwrap(), completed);
    let current = f.call("GetEvent", &json!({"id":event})).await.unwrap();
    assert_eq!(current["event"]["state"], "EVENT_STATE_PENDING");
    assert_eq!(current["processing"].as_array().unwrap().len(), 1);
    let mut altered = q.clone();
    altered["tasks"][0]["title"] = json!("changed");
    assert!(f.call("CompleteEvent", &altered).await.is_err());
    f.call("DeleteEvent", &mutation(&event)).await.unwrap();
    let replay = f.call("CompleteEvent", &q).await.unwrap();
    assert_eq!(replay["event"]["state"], "EVENT_STATE_DELETED");
    assert!(replay["event"].get("payload").is_none());
    assert!(replay.get("tasks").is_none());

    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM lane_entities WHERE kind=2")
            .fetch_one(&mut f.db)
            .await
            .unwrap(),
        2
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn expiry_release_renew_and_receiver_deletion() {
    let mut f = Fixture::new().await;
    let event = f.event().await;
    let token = f.claim(&event).await;
    let mut lease = mutation(&event);
    lease["leaseToken"] = json!(token);
    f.call("RenewEventLease", &lease).await.unwrap();
    lease["meta"]["requestId"] = json!(id());
    lease["errorReason"] = json!("worker stopped");
    f.call("ReleaseEventLease", &lease).await.unwrap();
    assert!(f.call("CompleteEvent", &lease).await.is_err());
    let second = f.claim(&event).await;
    assert_ne!(second, token);
    lease["leaseToken"] = json!(second);
    lease["meta"]["requestId"] = json!(id());
    sqlx::query("UPDATE lane_webhook_events SET expires_at=clock_timestamp() WHERE id=$1")
        .bind(&event)
        .execute(&mut f.db)
        .await
        .unwrap();
    assert!(f.call("RenewEventLease", &lease).await.is_err());
    assert!(f.call("CompleteEvent", &lease).await.is_err());
    let third = f.claim(&event).await;
    lease["leaseToken"] = json!(third);
    lease["meta"]["requestId"] = json!(id());
    f.call("DeleteReceiver", &mutation(&f.receiver.clone()))
        .await
        .unwrap();
    assert!(f.intake_raw(br#"{}"#).await.is_err());
    let completed = f.call("CompleteEvent", &lease).await.unwrap();
    assert_eq!(completed["tasks"], json!([]));
    assert_eq!(completed["event"]["state"], "EVENT_STATE_COMPLETED");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn shared_intake_vectors_execute_database_domain() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../protos/lane/v1/fixtures/webhooks.json"
    ))
    .unwrap();
    assert_eq!(fixtures["formatVersion"], 1);
    for case in fixtures["cases"].as_array().unwrap() {
        let input = &case["input"];
        let operation = string(input, "operation");
        if !matches!(
            operation,
            "parseIntake" | "intakeReplay" | "boundedIntake" | "intake"
        ) {
            continue;
        }
        let mut f = Fixture::new().await;
        match operation {
            "parseIntake" => {
                assert_eq!(
                    f.intake_raw(string(input, "rawBody").as_bytes())
                        .await
                        .is_ok(),
                    case["expected"]["accepted"].as_bool().unwrap(),
                    "{}",
                    case["id"]
                );
            }
            "intakeReplay" => {
                let first = f
                    .intake_raw(string(input, "firstRawBody").as_bytes())
                    .await
                    .unwrap();
                if input["deletedBeforeRetry"] == true {
                    f.call("DeleteEvent", &mutation(string(&first, "id")))
                        .await
                        .unwrap();
                }
                let replay = f.intake_raw(string(input, "retryRawBody").as_bytes()).await;
                match string(&case["expected"], "result") {
                    "CONFLICT" => assert!(replay.is_err()),
                    "DELETED" => assert_eq!(replay.unwrap()["deleted"], true),
                    "REPLAYED" => assert_eq!(replay.unwrap()["id"], first["id"]),
                    _ => panic!("unknown fixture outcome"),
                }
                let bodies: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM lane_webhook_events WHERE data IS NOT NULL",
                )
                .fetch_one(&mut f.db)
                .await
                .unwrap();
                assert_eq!(bodies, case["expected"]["retainedBodies"].as_i64().unwrap());
            }
            "boundedIntake" => {
                f.config.max_webhook_bytes = string(input, "maxWebhookBytes").parse().unwrap();
                assert!(
                    f.intake_raw(&vec![b' '; string(input, "receivedBytes").parse().unwrap()])
                        .await
                        .is_err()
                );
            }
            "intake" => {
                f.call("DeleteReceiver", &mutation(&f.receiver.clone()))
                    .await
                    .unwrap();
                assert!(f.intake_raw(b"{}").await.is_err());
            }
            _ => unreachable!(),
        }
        f.close().await;
    }
}

// PostgreSQL resolves this fixture clock before pg_catalog only in this disposable schema.
// This lets the actual domain SQL exercise exact microsecond boundaries without sleeping.
async fn set_clock(f: &mut Fixture, micros: i64) {
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS fixture_clock(value timestamptz NOT NULL); DELETE FROM fixture_clock;").execute(&mut f.db).await.unwrap();
    sqlx::query("INSERT INTO fixture_clock VALUES($1)")
        .bind(chrono::DateTime::from_timestamp_micros(micros).unwrap())
        .execute(&mut f.db)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE OR REPLACE FUNCTION clock_timestamp() RETURNS timestamptz LANGUAGE SQL AS 'SELECT value FROM fixture_clock'").execute(&mut f.db).await.unwrap();
    sqlx::query(&format!("SET search_path TO {},pg_catalog", f.schema))
        .execute(&mut f.db)
        .await
        .unwrap();
    let observed: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut f.db)
        .await
        .unwrap();
    assert_eq!(observed.timestamp_micros(), micros);
}
async fn state(f: &mut Fixture, event: &str) -> String {
    sqlx::query_scalar("SELECT state FROM lane_webhook_events WHERE id=$1")
        .bind(event)
        .fetch_one(&mut f.db)
        .await
        .unwrap()
}
async fn task_count(f: &mut Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM lane_entities WHERE kind=2")
        .fetch_one(&mut f.db)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn shared_processing_vectors_execute_database_domain() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../protos/lane/v1/fixtures/webhooks.json"
    ))
    .unwrap();
    assert_eq!(fixtures["formatVersion"], 1);
    let mut exercised = 0;
    for case in fixtures["cases"].as_array().unwrap() {
        let input = &case["input"];
        let expected = &case["expected"];
        let operation = string(input, "operation");
        if !matches!(
            operation,
            "complete"
                | "renew"
                | "completionReplay"
                | "reassignReceiver"
                | "releaseThenClaim"
                | "claim"
        ) {
            continue;
        }
        exercised += 1;
        let mut f = Fixture::new().await;
        let event = f.event().await;
        let token = f.claim(&event).await;
        let mut q = mutation(&event);
        q["leaseToken"] = json!(token);
        if let Some(now) = input["nowMicros"].as_str() {
            set_clock(&mut f, now.parse().unwrap()).await;
            let expiry = chrono::DateTime::from_timestamp_micros(
                string(input, "expiresAtMicros").parse().unwrap(),
            )
            .unwrap();
            sqlx::query("UPDATE lane_webhook_events SET expires_at=$1 WHERE id=$2")
                .bind(expiry)
                .bind(&event)
                .execute(&mut f.db)
                .await
                .unwrap();
        }
        match operation {
            "complete" | "renew" => {
                if input["tokenMatches"] == false {
                    q["leaseToken"] = json!("unknown");
                }
                if input["released"] == true {
                    f.call("ReleaseEventLease", &q).await.unwrap();
                    q["meta"]["requestId"] = json!(id());
                }
                if input["eventDeleted"] == true {
                    f.call("DeleteEvent", &mutation(&event)).await.unwrap();
                }
                if input["receiverDeleted"] == true {
                    f.call("DeleteReceiver", &mutation(&f.receiver.clone()))
                        .await
                        .unwrap();
                }
                if input["repositoryDeleted"] == true {
                    let mut tx = f.db.begin().await.unwrap();
                    account_lock(&mut tx, &f.principal.account_id)
                        .await
                        .unwrap();
                    webhooks::delete_repository(&mut tx, &f.principal.account_id, &f.repository)
                        .await
                        .unwrap();
                    tx.commit().await.unwrap();
                }
                if input["tasks"].is_array() {
                    q["tasks"] = input["tasks"].clone();
                }
                if input["failAt"].is_string() {
                    q["tasks"] =
                        json!([task(&f.repository, "first"), task(&f.repository, "second")]);
                    sqlx::raw_sql("CREATE FUNCTION fixture_fail_second_task() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind=2 AND EXISTS(SELECT 1 FROM lane_entities WHERE kind=2) THEN RAISE EXCEPTION 'injected storage failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER fixture_fail_second_task BEFORE INSERT ON lane_entities FOR EACH ROW EXECUTE FUNCTION fixture_fail_second_task();")
                        .execute(&mut f.db).await.unwrap();
                }
                let method = if operation == "renew" {
                    "RenewEventLease"
                } else {
                    "CompleteEvent"
                };
                let mut tx = f.db.begin().await.unwrap();
                account_lock(&mut tx, &f.principal.account_id)
                    .await
                    .unwrap();
                let outcome = webhooks::dispatch_with_config(
                    &mut tx,
                    &f.principal,
                    method,
                    &q,
                    usize::MAX,
                    Some(&f.config),
                )
                .await;
                // Completion catches even real SQL errors with a savepoint, leaving the caller
                // transaction usable and without partially created tasks.
                tx.commit().await.unwrap();
                let accept = expected["accepted"].as_bool().unwrap_or(false);
                assert_eq!(outcome.is_ok(), accept, "{}: {outcome:?}", case["id"]);
                if let Some(expiry) = expected["expiresAtMicros"].as_str() {
                    let actual = chrono::DateTime::parse_from_rfc3339(string(
                        &outcome.unwrap(),
                        "expiresAt",
                    ))
                    .unwrap()
                    .timestamp_micros();
                    assert_eq!(actual, expiry.parse::<i64>().unwrap());
                }
                if let Some(count) = expected["newTasks"].as_i64() {
                    assert_eq!(task_count(&mut f).await, count, "{}", case["id"]);
                }
                if let Some(count) = expected["processingRecords"].as_i64() {
                    assert_eq!(
                        sqlx::query_scalar::<_, i64>(
                            "SELECT count(*) FROM lane_webhook_processing"
                        )
                        .fetch_one(&mut f.db)
                        .await
                        .unwrap(),
                        count
                    );
                }
                if let Some(expected_state) = expected["state"].as_str() {
                    assert_eq!(
                        format!("EVENT_STATE_{}", state(&mut f, &event).await),
                        expected_state
                    );
                }
            }
            "completionReplay" => {
                let ids = input["originalTaskIds"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| vec![json!(id()), json!(id())]);
                q["tasks"] = Value::Array(
                    ids.iter()
                        .map(|id| {
                            let mut t = task(&f.repository, "completed");
                            t["id"] = id.clone();
                            t
                        })
                        .collect(),
                );
                let original = f.call("CompleteEvent", &q).await.unwrap();
                if input["currentState"] == "EVENT_STATE_PENDING" {
                    let mut reopen = mutation(&event);
                    reopen["expectedRevision"] = original["event"]["revision"].clone();
                    f.call("ReopenEvent", &reopen).await.unwrap();
                }
                if input["currentState"] == "EVENT_STATE_DELETED" {
                    f.call("DeleteEvent", &mutation(&event)).await.unwrap();
                }
                if input["identicalRequest"] == false {
                    q["tasks"][0]["title"] = json!("different");
                }
                let count = task_count(&mut f).await;
                let replay = f.call("CompleteEvent", &q).await;
                if input["identicalRequest"] == false {
                    assert_eq!(
                        replay.unwrap_err().code,
                        connectrpc::ErrorCode::AlreadyExists
                    );
                } else if expected["bodyPresent"] == false {
                    let replay = replay.unwrap();
                    assert_eq!(replay["event"]["state"], "EVENT_STATE_DELETED");
                    assert!(replay["event"].get("payload").is_none());
                    assert!(replay.get("processing").is_none());
                } else {
                    assert_eq!(
                        replay.unwrap()["processing"]["taskIds"],
                        expected["taskIds"]
                    );
                }
                assert_eq!(task_count(&mut f).await, count);
                if let Some(expected_state) = expected["state"].as_str() {
                    assert_eq!(
                        format!("EVENT_STATE_{}", state(&mut f, &event).await),
                        expected_state
                    );
                }
            }
            "releaseThenClaim" | "claim" => {
                if operation == "releaseThenClaim" {
                    f.call("ReleaseEventLease", &q).await.unwrap();
                }
                let claimed = f.call("ClaimEvent", &mutation(&event)).await.unwrap();
                assert_ne!(claimed["leaseToken"], token);
                q["meta"]["requestId"] = json!(id());
                assert!(f.call("CompleteEvent", &q).await.is_err());
                let actual = chrono::DateTime::parse_from_rfc3339(string(&claimed, "expiresAt"))
                    .unwrap()
                    .timestamp_micros();
                let expected_expiry = expected
                    .get("expiresAtMicros")
                    .unwrap_or(&expected["newExpiresAtMicros"])
                    .as_str()
                    .unwrap()
                    .parse::<i64>()
                    .unwrap();
                assert_eq!(actual, expected_expiry);
            }
            "reassignReceiver" => {
                let next = id();
                let mut tx = f.db.begin().await.unwrap();
                account_lock(&mut tx, &f.principal.account_id)
                    .await
                    .unwrap();
                entities::dispatch(&mut tx,&f.principal,"CreateRepository",&json!({"meta":{"requestId":id()},"id":next,"url":"https://example.com/test/next"}),usize::MAX).await.unwrap();
                tx.commit().await.unwrap();
                let mut save = mutation(&f.receiver);
                save["expectedRevision"] = json!("1");
                save["repositoryId"] = json!(next);
                f.call("SaveReceiver", &save).await.unwrap();
                let current = f.call("GetEvent", &json!({"id":event})).await.unwrap();
                assert_eq!(current["event"]["repositoryId"], f.repository);
                let new_event = f.event().await;
                let new = f.call("GetEvent", &json!({"id":new_event})).await.unwrap();
                assert_eq!(new["event"]["repositoryId"], next);
            }
            _ => unreachable!(),
        }
        f.close().await;
    }
    assert_eq!(exercised, 20);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn account_claimant_isolation_task_redaction_and_outer_rollback() {
    let mut f = Fixture::new().await;
    let event = f.event().await;
    let token = f.claim(&event).await;
    let mut q = mutation(&event);
    q["leaseToken"] = json!(token);
    q["tasks"] = json!([task(&f.repository, "private task body")]);
    let owner = f.principal.clone();
    f.principal.session_id = id();
    assert_eq!(f.call("CompleteEvent", &q).await.unwrap_err().reason, 18);
    f.principal.account_id = id();
    assert!(f.call("GetEvent", &json!({"id":event})).await.is_err());
    assert!(f.call("DeleteEvent", &mutation(&event)).await.is_err());
    f.principal = owner;
    let mut tx = f.db.begin().await.unwrap();
    account_lock(&mut tx, &f.principal.account_id)
        .await
        .unwrap();
    webhooks::dispatch_with_config(
        &mut tx,
        &f.principal,
        "CompleteEvent",
        &q,
        usize::MAX,
        Some(&f.config),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(state(&mut f, &event).await, "LEASED");
    assert_eq!(task_count(&mut f).await, 0);
    let completed = f.call("CompleteEvent", &q).await.unwrap();
    // Receiver IDs are client-selected; the same UUID can identify an event in another table.
    // Deleting that receiver must not redact the unrelated event's completion receipt.
    let same_id = json!({"meta":{"requestId":id()},"id":event,"repositoryId":f.repository,"name":"distinct resource"});
    f.call("CreateReceiver", &same_id).await.unwrap();
    f.call("DeleteReceiver", &mutation(&event)).await.unwrap();
    assert_eq!(f.call("CompleteEvent", &q).await.unwrap(), completed);
    let task_id = string(&q["tasks"][0], "id");
    let mut tx = f.db.begin().await.unwrap();
    account_lock(&mut tx, &f.principal.account_id)
        .await
        .unwrap();
    entities::dispatch(
        &mut tx,
        &f.principal,
        "DeleteTask",
        &mutation(task_id),
        usize::MAX,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let replay = f.call("CompleteEvent", &q).await.unwrap();
    assert_eq!(replay["tasks"][0]["result"], "RESULT_KIND_DELETED");
    assert!(replay["tasks"][0].get("version").is_none());
    assert_eq!(
        replay["processing"]["taskIds"],
        completed["processing"]["taskIds"]
    );
    let mut tx = f.db.begin().await.unwrap();
    account_lock(&mut tx, &f.principal.account_id)
        .await
        .unwrap();
    webhooks::delete_repository(&mut tx, &f.principal.account_id, &f.repository)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        webhooks::receiver_account(&mut f.db, &f.receiver)
            .await
            .is_err()
    );
    let mut tx = f.db.begin().await.unwrap();
    account_lock(&mut tx, &f.principal.account_id)
        .await
        .unwrap();
    webhooks::delete_account(&mut tx, &f.principal.account_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for table in [
        "lane_webhook_receivers",
        "lane_webhook_events",
        "lane_webhook_processing",
        "lane_webhook_receipts",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&mut f.db)
                .await
                .unwrap(),
            0
        );
    }
    f.close().await;
}
