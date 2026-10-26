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
        sqlx::raw_sql(include_str!("../migrations/00007_planning.sql"))
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
        let (receiver, secret) = (self.receiver.clone(), self.secret.clone());
        self.intake_to(&receiver, &secret, raw).await
    }
    async fn intake_to(
        &mut self,
        receiver: &str,
        secret: &str,
        raw: &[u8],
    ) -> lane_api::Result<Value> {
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let signature = sign(secret, raw, &timestamp);
        let mut tx = self.db.begin().await?;
        let result =
            webhooks::intake(&mut tx, receiver, raw, &timestamp, &signature, &self.config).await;
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
async fn completion_tasks_follow_the_planning_rules_and_roll_back_together() {
    let mut f = Fixture::new().await;
    let label = id();
    let mut tx = f.db.begin().await.unwrap();
    account_lock(&mut tx, &f.principal.account_id)
        .await
        .unwrap();
    entities::dispatch(&mut tx,&f.principal,"CreatePlanningItem",&json!({"meta":{"requestId":id()},"id":label,"item":{"label":{"repositoryId":f.repository,"name":"Triage","color":"#00ff00"}}}),usize::MAX).await.unwrap();
    tx.commit().await.unwrap();
    let event = f.event().await;
    let token = f.claim(&event).await;
    let mut q = mutation(&event);
    q["leaseToken"] = json!(token);
    let mut first = task(&f.repository, "first");
    first["labelIds"] = json!([label]);
    first["priority"] = json!("TASK_PRIORITY_HIGH");
    let mut second = task(&f.repository, "second");
    second["parentTaskId"] = first["id"].clone();
    // A later task that violates a rule rolls back the whole completion, earlier tasks included.
    let mut bad = task(&f.repository, "bad");
    bad["labelIds"] = json!([id()]);
    q["tasks"] = json!([first, second, bad]);
    let rejected = f.call("CompleteEvent", &q).await.unwrap_err();
    assert_eq!(
        lane_api::planning::violation_of(&rejected).map(|v| v.name()),
        Some("CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND")
    );
    let tasks: i64 = sqlx::query_scalar("SELECT count(*) FROM lane_entities WHERE kind=2")
        .fetch_one(&mut f.db)
        .await
        .unwrap();
    assert_eq!(tasks, 0);
    q["tasks"] = json!([q["tasks"][0], q["tasks"][1]]);
    let completed = f.call("CompleteEvent", &q).await.unwrap();
    assert_eq!(
        completed["tasks"][0]["version"]["task"]["labelIds"],
        json!([label])
    );
    assert_eq!(
        completed["tasks"][0]["version"]["task"]["priority"],
        "TASK_PRIORITY_HIGH"
    );
    assert_eq!(
        completed["tasks"][1]["version"]["task"]["parentTaskId"],
        first["id"]
    );
    assert_eq!(f.call("CompleteEvent", &q).await.unwrap(), completed);
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn create_receiver_endpoint_forms_and_legacy_receipt_replay() {
    let mut f = Fixture::new().await;
    let repository = f.repository.clone();
    let origin = |base: &str| {
        WebhookConfig::new(&URL_SAFE_NO_PAD.encode([8; 32]), base, 1024 * 1024).unwrap()
    };
    for (base, prefix) in [
        ("https://lane.example", "https://lane.example/webhooks"),
        ("https://lane.example/", "https://lane.example/webhooks"),
        (
            "https://lane.example/webhooks",
            "https://lane.example/webhooks",
        ),
        (
            "https://lane.example/webhooks/",
            "https://lane.example/webhooks",
        ),
        ("https://lane.example/hooks/", "https://lane.example/hooks"),
    ] {
        f.config = origin(base);
        let receiver = id();
        let q = json!({"meta":{"requestId":id()},"id":receiver,"repositoryId":repository,"name":"form"});
        let created = f.call("CreateReceiver", &q).await.unwrap();
        assert_eq!(
            created["endpoint"],
            format!("{prefix}/{receiver}"),
            "{base}"
        );
    }
    // A receipt issued before the endpoint builder was corrected holds the old origin-relative
    // URL. Replay returns the durable receipt unchanged and never creates another receiver or key.
    f.config = origin("https://lane.example");
    let receiver = id();
    let q =
        json!({"meta":{"requestId":id()},"id":receiver,"repositoryId":repository,"name":"legacy"});
    let created = f.call("CreateReceiver", &q).await.unwrap();
    let legacy = format!("https://lane.example/{receiver}");
    sqlx::query("UPDATE lane_webhook_receipts SET response=jsonb_set(response,'{endpoint}',to_jsonb($1::text)) WHERE account_id=$2 AND method='CreateReceiver' AND request_id=$3")
        .bind(&legacy)
        .bind(&f.principal.account_id)
        .bind(q["meta"]["requestId"].as_str().unwrap())
        .execute(&mut f.db)
        .await
        .unwrap();
    let key_query =
        "SELECT encrypted_key FROM lane_webhook_receivers WHERE account_id=$1 AND id=$2";
    let key_before: Vec<u8> = sqlx::query(key_query)
        .bind(&f.principal.account_id)
        .bind(&receiver)
        .fetch_one(&mut f.db)
        .await
        .unwrap()
        .get(0);
    let before: i64 =
        sqlx::query("SELECT count(*) FROM lane_webhook_receivers WHERE account_id=$1")
            .bind(&f.principal.account_id)
            .fetch_one(&mut f.db)
            .await
            .unwrap()
            .get(0);
    let replay = f.call("CreateReceiver", &q).await.unwrap();
    // Replay never re-discloses the signing key (existing behavior); the receipt is otherwise
    // unchanged, including the old endpoint.
    let mut expected = created.clone();
    expected["endpoint"] = json!(legacy);
    expected.as_object_mut().unwrap().remove("secret");
    assert_eq!(replay, expected);
    let after: i64 = sqlx::query("SELECT count(*) FROM lane_webhook_receivers WHERE account_id=$1")
        .bind(&f.principal.account_id)
        .fetch_one(&mut f.db)
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after);
    let key_after: Vec<u8> = sqlx::query(key_query)
        .bind(&f.principal.account_id)
        .bind(&receiver)
        .fetch_one(&mut f.db)
        .await
        .unwrap()
        .get(0);
    assert_eq!(key_before, key_after);
    f.close().await;
}

// ---------------------------------------------------------------------------------------------
// Repository alias identity: webhook writes, projections, filters, cascade and cleanup.
//
// A synchronized repository alias L maps to canonical repository C inside one account. Receivers,
// events, list filters and repository deletion must treat L and C as the same repository.
// "Historical" rows model the state written before aliases were resolved: the stored repository
// reference is the raw alias although the mapping already existed.
// ---------------------------------------------------------------------------------------------

const MARKER: &str = "synthetic-c06";
const RECEIVER_NAME: &str = "receiver-name-c06";

impl Fixture {
    async fn entity_call(&mut self, method: &str, q: &Value) -> lane_api::Result<Value> {
        let mut tx = self.db.begin().await?;
        account_lock(&mut tx, &self.principal.account_id).await?;
        let result = entities::dispatch(&mut tx, &self.principal, method, q, usize::MAX).await;
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
    async fn sync_call(&mut self, method: &str, mut q: Value) -> lane_api::Result<Value> {
        let epoch: String =
            sqlx::query_scalar("SELECT epoch FROM lane_accounts WHERE account_id=$1")
                .bind(&self.principal.account_id)
                .fetch_one(&mut self.db)
                .await?;
        q["formatVersion"] = json!(3);
        q["epoch"] = json!(epoch);
        q["deviceId"] = json!(self.principal.device_id);
        let mut tx = self.db.begin().await?;
        account_lock(&mut tx, &self.principal.account_id).await?;
        let result =
            lane_api::sync::dispatch(&mut tx, &self.principal, method, &q, usize::MAX).await;
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
    async fn create_repository(&mut self, repository: &str, url: &str) {
        self.entity_call(
            "CreateRepository",
            &json!({"meta":{"requestId":id()},"id":repository,"url":url}),
        )
        .await
        .unwrap();
    }
    /// Duplicate repository CREATE under `local` with the same normalized URL as `canonical`:
    /// the real synchronization flow records the account-local mapping local -> canonical.
    async fn alias(&mut self, canonical: &str, local: &str) {
        let mut version = self
            .entity_call("GetRepository", &json!({"id":canonical}))
            .await
            .unwrap()["result"]["version"]
            .clone();
        version["entityId"] = json!(local);
        version["versionId"] = json!(id());
        let mutation = json!({"meta":{"requestId":id()},"kind":"MUTATION_KIND_CREATE","version":version,"expectedHeadIds":[]});
        let pushed = self
            .sync_call("PushChanges", json!({"mutations":[mutation]}))
            .await
            .unwrap();
        assert!(
            pushed["repositoryMappings"]
                .as_array()
                .unwrap()
                .contains(&json!({"localId":local,"canonicalId":canonical}))
        );
    }
    async fn receiver_at(&mut self, repository: &str, name: &str) -> (String, String) {
        let receiver = id();
        let created = self
            .call(
                "CreateReceiver",
                &json!({"meta":{"requestId":id()},"id":receiver,"repositoryId":repository,"name":name}),
            )
            .await
            .unwrap();
        (receiver, string(&created, "secret").into())
    }
    async fn event_at(&mut self, receiver: &str, secret: &str, tag: &str) -> String {
        let raw = serde_json::to_vec(&json!({"id":id(),"type":"test","occurredAt":"2026-10-25T00:00:00Z","payload":{"marker":format!("{MARKER}-{tag}")}})).unwrap();
        string(&self.intake_to(receiver, secret, &raw).await.unwrap(), "id").into()
    }
    async fn delete_repository(&mut self, repository: &str) {
        self.entity_call(
            "DeleteRepository",
            &json!({"meta":{"requestId":id()},"id":repository}),
        )
        .await
        .unwrap();
    }
    async fn pool(&self) -> sqlx::PgPool {
        let url = std::env::var("LANE_TEST_DATABASE_URL").unwrap();
        let schema = self.schema.clone();
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .after_connect(move |conn, _| {
                let schema = schema.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {schema}"))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap()
    }
    async fn count(&mut self, sql: &str, binds: &[&str]) -> i64 {
        let mut query = sqlx::query_scalar::<_, i64>(sql);
        for bind in binds {
            query = query.bind(*bind);
        }
        query.fetch_one(&mut self.db).await.unwrap()
    }
    async fn stored_repository(&mut self, table: &str, id: &str) -> String {
        sqlx::query_scalar(&format!("SELECT repository_id FROM {table} WHERE id=$1"))
            .bind(id)
            .fetch_one(&mut self.db)
            .await
            .unwrap()
    }
    async fn set_stored_repository(&mut self, table: &str, id: &str, repository: &str) {
        sqlx::query(&format!("UPDATE {table} SET repository_id=$2 WHERE id=$1"))
            .bind(id)
            .bind(repository)
            .execute(&mut self.db)
            .await
            .unwrap();
    }
    async fn list_all(&mut self, method: &str, repository: Option<&str>, size: u64) -> Vec<Value> {
        let mut items = Vec::new();
        let mut cursor = String::new();
        loop {
            let mut q = json!({"page":{"size":size,"cursor":cursor}});
            if let Some(repository) = repository {
                q["repositoryId"] = json!(repository);
            }
            let page = self.call(method, &q).await.unwrap();
            items.extend(page["items"].as_array().unwrap().clone());
            cursor = string(&page, "nextCursor").into();
            if cursor.is_empty() {
                return items;
            }
        }
    }
    async fn deletion_changes(&mut self) -> i64 {
        let account = self.principal.account_id.clone();
        self.count(
            "SELECT count(*) FROM lane_changes WHERE account_id=$1 AND data->'deletion' IS NOT NULL",
            &[&account],
        )
        .await
    }
    /// Receiver and event rows as `(id, revision, deletion version)` for stability comparisons.
    async fn tombstones(&mut self) -> Vec<(String, i64, Option<String>)> {
        sqlx::query_as("SELECT id,revision,deletion_version_id FROM (SELECT id,revision,deletion_version_id FROM lane_webhook_receivers UNION ALL SELECT id,revision,deletion_version_id FROM lane_webhook_events) t ORDER BY id")
            .fetch_all(&mut self.db)
            .await
            .unwrap()
    }
}

struct Seeded {
    canonical: String,
    alias_a: String,
    alias_b: String,
    receiver: String,
    secret: String,
    pending: String,
    leased: String,
    lease_token: String,
    completed: String,
    errored: String,
}
impl Seeded {
    fn events(&self) -> [&str; 4] {
        [&self.pending, &self.leased, &self.completed, &self.errored]
    }
}
/// Two aliases of the fixture repository, a receiver created through alias A, and pending,
/// leased, completed (zero tasks) and error-retaining events. `historical` rewrites the stored
/// references to raw alias IDs, which is what the pre-fix flow persisted.
async fn seed(f: &mut Fixture, historical: bool) -> Seeded {
    let canonical = f.repository.clone();
    let (alias_a, alias_b) = (id(), id());
    f.alias(&canonical, &alias_a).await;
    f.alias(&canonical, &alias_b).await;
    let (receiver, secret) = f.receiver_at(&alias_a, RECEIVER_NAME).await;
    assert_eq!(
        f.stored_repository("lane_webhook_receivers", &receiver)
            .await,
        canonical,
        "new receivers persist the canonical repository"
    );
    let pending = f.event_at(&receiver, &secret, "pending").await;
    let leased = f.event_at(&receiver, &secret, "leased").await;
    let completed = f.event_at(&receiver, &secret, "completed").await;
    let errored = f.event_at(&receiver, &secret, "errored").await;
    let lease_token = f.claim(&leased).await;
    let token = f.claim(&completed).await;
    let mut complete = mutation(&completed);
    complete["leaseToken"] = json!(token);
    f.call("CompleteEvent", &complete).await.unwrap();
    let token = f.claim(&errored).await;
    let mut release = mutation(&errored);
    release["leaseToken"] = json!(token);
    release["errorReason"] = json!(format!("{MARKER}-error"));
    f.call("ReleaseEventLease", &release).await.unwrap();
    if historical {
        f.set_stored_repository("lane_webhook_receivers", &receiver, &alias_a)
            .await;
        for (event, alias) in [
            (&pending, &alias_a),
            (&leased, &alias_a),
            (&completed, &alias_b),
            (&errored, &alias_b),
        ] {
            f.set_stored_repository("lane_webhook_events", event, alias)
                .await;
        }
    }
    Seeded {
        canonical,
        alias_a,
        alias_b,
        receiver,
        secret,
        pending,
        leased,
        lease_token,
        completed,
        errored,
    }
}
/// Everything retained by the listed receivers and events must be erased.
async fn assert_scrubbed(f: &mut Fixture, receivers: &[&str], events: &[&str], needles: &[&str]) {
    let account = f.principal.account_id.clone();
    for receiver in receivers {
        let row = sqlx::query(
            "SELECT name,encrypted_key,deletion_version_id FROM lane_webhook_receivers WHERE id=$1",
        )
        .bind(receiver)
        .fetch_one(&mut f.db)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("name"), "", "{receiver}");
        assert!(row.get::<Option<Vec<u8>>, _>("encrypted_key").is_none());
        assert!(
            row.get::<Option<String>, _>("deletion_version_id")
                .is_some()
        );
    }
    for event in events {
        let row = sqlx::query("SELECT state,data,lease_hash,claimant,expires_at,last_error,deletion_version_id FROM lane_webhook_events WHERE id=$1")
            .bind(event)
            .fetch_one(&mut f.db)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("state"), "DELETED", "{event}");
        assert!(row.get::<Option<Value>, _>("data").is_none());
        assert!(row.get::<Option<String>, _>("lease_hash").is_none());
        assert!(row.get::<Option<String>, _>("claimant").is_none());
        assert!(
            row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("expires_at")
                .is_none()
        );
        assert!(row.get::<Option<String>, _>("last_error").is_none());
        assert!(
            row.get::<Option<String>, _>("deletion_version_id")
                .is_some()
        );
        assert_eq!(
            f.count(
                "SELECT count(*) FROM lane_webhook_processing WHERE event_id=$1",
                &[event]
            )
            .await,
            0
        );
        let rejected = f.call("GetEvent", &json!({"id":event})).await.unwrap_err();
        assert_eq!(rejected.reason, 7, "GetEvent must reject deleted events");
    }
    for needle in needles {
        let pattern = format!("%{needle}%");
        assert_eq!(
            f.count(
                "SELECT count(*) FROM lane_changes WHERE account_id=$1 AND data::text LIKE $2",
                &[&account, &pattern]
            )
            .await,
            0,
            "changes retain {needle}"
        );
        assert_eq!(
            f.count(
                "SELECT count(*) FROM lane_webhook_receipts WHERE account_id=$1 AND response::text LIKE $2",
                &[&account, &pattern]
            )
            .await,
            0,
            "receipts retain {needle}"
        );
        assert_eq!(
            f.count(
                "SELECT count(*) FROM lane_webhook_events WHERE account_id=$1 AND data::text LIKE $2",
                &[&account, &pattern]
            )
            .await,
            0
        );
    }
    // Deletion-only history: a pull from the oldest cursor shows tombstones and no content.
    let pulled = f.sync_call("PullChanges", json!({})).await.unwrap();
    let changes = pulled["changes"].as_array().unwrap();
    assert_eq!(pulled["hasMore"], false);
    let text = Value::Array(changes.clone()).to_string();
    assert!(needles.iter().all(|needle| !text.contains(needle)));
    for resource in receivers.iter().chain(events.iter()) {
        // Changes about this resource itself; surviving events may still name a deleted receiver.
        let mentions: Vec<&Value> = changes
            .iter()
            .filter(|c| {
                [
                    &c["deletion"]["id"],
                    &c["event"]["id"],
                    &c["receiver"]["id"],
                    &c["processing"]["eventId"],
                ]
                .contains(&&json!(resource))
            })
            .collect();
        assert!(!mentions.is_empty(), "{resource} keeps a tombstone change");
        assert!(
            mentions
                .iter()
                .all(|c| c["deletion"]["id"] == json!(resource)),
            "{resource} has only deletion changes: {mentions:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn alias_writes_persist_canonical_ownership_and_both_spellings_delete_the_repository() {
    for delete_with_alias in [true, false] {
        let mut f = Fixture::new().await;
        let s = seed(&mut f, false).await;
        // SaveReceiver through the second alias also persists the canonical repository.
        let (second, second_secret) = f.receiver_at(&s.canonical, "direct").await;
        let mut save = mutation(&second);
        save["expectedRevision"] = json!("1");
        save["repositoryId"] = json!(s.alias_b);
        save["name"] = json!("saved");
        let saved = f.call("SaveReceiver", &save).await.unwrap();
        assert_eq!(saved["receiver"]["repositoryId"], json!(s.canonical));
        assert_eq!(
            f.stored_repository("lane_webhook_receivers", &second).await,
            s.canonical
        );
        let event = f.event_at(&second, &second_secret, "saved").await;
        assert_eq!(
            f.stored_repository("lane_webhook_events", &event).await,
            s.canonical,
            "events inherit the canonical repository"
        );
        let fetched = f.call("GetEvent", &json!({"id":event})).await.unwrap();
        assert_eq!(fetched["event"]["repositoryId"], json!(s.canonical));
        // Every spelling selects the same live rows.
        let by_canonical = f.list_all("ListReceivers", Some(&s.canonical), 100).await;
        assert_eq!(by_canonical.len(), 3);
        for spelling in [&s.alias_a, &s.alias_b] {
            assert_eq!(
                f.list_all("ListReceivers", Some(spelling), 100).await,
                by_canonical
            );
        }
        // Delete using L or C: the same account transaction scrubs every owned resource.
        let target = if delete_with_alias {
            s.alias_a.clone()
        } else {
            s.canonical.clone()
        };
        f.delete_repository(&target).await;
        let mut events: Vec<&str> = s.events().to_vec();
        events.push(&event);
        let direct = f.receiver.clone();
        let receivers = [s.receiver.as_str(), second.as_str(), direct.as_str()];
        assert_scrubbed(&mut f, &receivers, &events, &[MARKER, RECEIVER_NAME]).await;
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn historical_alias_rows_filter_identically_across_pages() {
    let mut f = Fixture::new().await;
    let s = seed(&mut f, true).await;
    // More rows in mixed spellings, and a second live repository that must stay isolated.
    let (extra, extra_secret) = f.receiver_at(&s.canonical, "extra").await;
    f.set_stored_repository("lane_webhook_receivers", &extra, &s.alias_b)
        .await;
    let extra_event = f.event_at(&extra, &extra_secret, "extra").await;
    f.set_stored_repository("lane_webhook_events", &extra_event, &s.alias_b)
        .await;
    let other = id();
    f.create_repository(&other, "https://example.com/other/repo")
        .await;
    let (other_receiver, other_secret) = f.receiver_at(&other, "other").await;
    let other_event = f.event_at(&other_receiver, &other_secret, "other").await;

    for (method, expected_total) in [("ListReceivers", 3usize), ("ListEvents", 5)] {
        let baseline = f.list_all(method, Some(&s.canonical), 2).await;
        assert_eq!(baseline.len(), expected_total, "{method}");
        let mut seen: Vec<&str> = baseline.iter().map(|i| string(i, "id")).collect();
        let sorted = {
            let mut copy = seen.clone();
            copy.sort();
            copy.dedup();
            copy
        };
        assert_eq!(seen, sorted, "{method} pages are ordered and not repeated");
        seen.clear();
        for item in &baseline {
            assert_eq!(item["repositoryId"], json!(s.canonical), "{method}");
        }
        for spelling in [&s.alias_a, &s.alias_b] {
            for size in [1, 2, 100] {
                assert_eq!(
                    f.list_all(method, Some(spelling), size).await,
                    baseline,
                    "{method} {spelling} size {size}"
                );
            }
        }
        // Isolation from the other repository and the unfiltered view.
        let others = f.list_all(method, Some(&other), 1).await;
        assert_eq!(others.len(), 1);
        assert_eq!(others[0]["repositoryId"], json!(other));
        assert!(baseline.iter().all(|i| i["id"] != others[0]["id"]));
        assert_eq!(f.list_all(method, None, 2).await.len(), expected_total + 1);
    }
    // Cursors are scoped to the canonical query: alias and canonical share them, others reject.
    let first = f
        .call(
            "ListEvents",
            &json!({"repositoryId":s.alias_a,"page":{"size":2}}),
        )
        .await
        .unwrap();
    let cursor = string(&first, "nextCursor").to_owned();
    assert!(!cursor.is_empty());
    let via_canonical = f
        .call(
            "ListEvents",
            &json!({"repositoryId":s.canonical,"page":{"size":100,"cursor":cursor}}),
        )
        .await
        .unwrap();
    let via_alias = f
        .call(
            "ListEvents",
            &json!({"repositoryId":s.alias_b,"page":{"size":100,"cursor":cursor}}),
        )
        .await
        .unwrap();
    assert_eq!(via_canonical, via_alias);
    assert_eq!(via_canonical["items"].as_array().unwrap().len(), 3);
    assert!(
        f.call(
            "ListEvents",
            &json!({"repositoryId":other,"page":{"cursor":cursor}})
        )
        .await
        .is_err()
    );
    // State filters still compose with the repository filter.
    let leased = f
        .call(
            "ListEvents",
            &json!({"repositoryId":s.alias_b,"states":["EVENT_STATE_LEASED"]}),
        )
        .await
        .unwrap();
    assert_eq!(leased["items"].as_array().unwrap().len(), 1);
    assert_eq!(leased["items"][0]["id"], json!(s.leased));
    assert_ne!(string(&leased["items"][0], "id"), other_event);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn deleting_the_canonical_repository_scrubs_historical_alias_resources() {
    let mut f = Fixture::new().await;
    let s = seed(&mut f, true).await;
    let direct = f.receiver.clone();
    // Counterfactual before deletion: the retained alias content is live and readable.
    assert_eq!(
        f.call("GetEvent", &json!({"id":s.pending})).await.unwrap()["event"]["repositoryId"],
        json!(s.canonical)
    );
    f.delete_repository(&s.canonical).await;
    assert_scrubbed(
        &mut f,
        &[&s.receiver, &direct],
        &s.events(),
        &[MARKER, RECEIVER_NAME],
    )
    .await;
    // The old lease cannot create tasks, and no event can be claimed again.
    let mut late = mutation(&s.leased);
    late["leaseToken"] = json!(s.lease_token);
    late["tasks"] = json!([task(&s.canonical, "late task")]);
    assert!(f.call("CompleteEvent", &late).await.is_err());
    assert_eq!(task_count(&mut f).await, 0);
    assert!(f.call("ClaimEvent", &mutation(&s.pending)).await.is_err());
    assert!(f.intake_to(&s.receiver, &s.secret, br#"{}"#).await.is_err());
    f.close().await;
}

/// Reproduce the missed cascade that earlier builds performed: they selected only exact
/// repository IDs, so deleting the canonical repository left alias-owned rows behind.
async fn delete_ignoring_aliases(f: &mut Fixture, repository: &str) {
    let account = f.principal.account_id.clone();
    let mappings: Vec<(String, String)> = sqlx::query_as(
        "SELECT local_id,canonical_id FROM lane_repository_mappings WHERE account_id=$1",
    )
    .bind(&account)
    .fetch_all(&mut f.db)
    .await
    .unwrap();
    sqlx::query("DELETE FROM lane_repository_mappings WHERE account_id=$1")
        .bind(&account)
        .execute(&mut f.db)
        .await
        .unwrap();
    f.delete_repository(repository).await;
    for (local, canonical) in mappings {
        sqlx::query("INSERT INTO lane_repository_mappings(account_id,local_id,canonical_id) VALUES($1,$2,$3)")
            .bind(&account)
            .bind(local)
            .bind(canonical)
            .execute(&mut f.db)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn compatibility_cleanup_scrubs_orphans_idempotently_with_rollback_and_resume() {
    let mut f = Fixture::new().await;
    let s = seed(&mut f, true).await;
    delete_ignoring_aliases(&mut f, &s.canonical).await;
    // The missed deletion is committed: the receiver key and event content survive C's deletion.
    let key_before: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT encrypted_key FROM lane_webhook_receivers WHERE id=$1")
            .bind(&s.receiver)
            .fetch_one(&mut f.db)
            .await
            .unwrap();
    assert!(key_before.is_some());
    assert!(f.call("GetEvent", &json!({"id":s.pending})).await.is_ok());
    let changes_before = f.deletion_changes().await;
    let tombstones_before = f.tombstones().await;
    let pool = f.pool().await;

    // Fixture transaction failure on the last event: the whole account's cleanup rolls back.
    let last = s.events().iter().max().copied().unwrap().to_owned();
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION fail_event_scrub() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected cleanup failure'; END $$; \
         CREATE TRIGGER inject_failure BEFORE UPDATE ON lane_webhook_events FOR EACH ROW WHEN (NEW.id = '{last}' AND NEW.state = 'DELETED') EXECUTE FUNCTION fail_event_scrub()"
    ))
    .execute(&mut f.db)
    .await
    .unwrap();
    assert!(
        webhooks::cleanup_deleted_repository_resources(&pool)
            .await
            .is_err()
    );
    assert_eq!(f.tombstones().await, tombstones_before);
    assert_eq!(f.deletion_changes().await, changes_before);
    let key_after: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT encrypted_key FROM lane_webhook_receivers WHERE id=$1")
            .bind(&s.receiver)
            .fetch_one(&mut f.db)
            .await
            .unwrap();
    assert_eq!(key_after, key_before, "partial cleanup must roll back");

    // Retry after the fault is removed resumes and completes.
    sqlx::raw_sql("DROP TRIGGER inject_failure ON lane_webhook_events")
        .execute(&mut f.db)
        .await
        .unwrap();
    let report = webhooks::cleanup_deleted_repository_resources(&pool)
        .await
        .unwrap();
    assert_eq!(
        report,
        webhooks::CleanupReport {
            accounts: 1,
            receivers: 1,
            events: 4
        }
    );
    let direct = f.receiver.clone();
    assert_scrubbed(
        &mut f,
        &[&s.receiver, &direct],
        &s.events(),
        &[MARKER, RECEIVER_NAME],
    )
    .await;
    // Second run: stable tombstone identities, revisions and deletion-change counts.
    let tombstones = f.tombstones().await;
    let changes = f.deletion_changes().await;
    assert_eq!(changes, changes_before + 5);
    assert_eq!(
        webhooks::cleanup_deleted_repository_resources(&pool)
            .await
            .unwrap(),
        webhooks::CleanupReport::default()
    );
    assert_eq!(f.tombstones().await, tombstones);
    assert_eq!(f.deletion_changes().await, changes);
    pool.close().await;
    f.close().await;
}

/// R starts under L -> C and receives E1, moves to D and receives E2.
struct Moved {
    alias: String,
    destination: String,
    receiver: String,
    secret: String,
    first: String,
    second: String,
}
async fn moved(f: &mut Fixture, historical: bool) -> Moved {
    let canonical = f.repository.clone();
    let alias = id();
    f.alias(&canonical, &alias).await;
    let destination = id();
    f.create_repository(&destination, "https://example.com/destination/repo")
        .await;
    let (receiver, secret) = f.receiver_at(&alias, "moved-receiver").await;
    let first = f.event_at(&receiver, &secret, "first").await;
    let mut save = mutation(&receiver);
    save["expectedRevision"] = json!("1");
    save["repositoryId"] = json!(destination);
    save["name"] = json!("moved-receiver");
    f.call("SaveReceiver", &save).await.unwrap();
    let second = f.event_at(&receiver, &secret, "second").await;
    if historical {
        f.set_stored_repository("lane_webhook_events", &first, &alias)
            .await;
    }
    assert_eq!(
        f.stored_repository("lane_webhook_events", &second).await,
        destination
    );
    Moved {
        alias,
        destination,
        receiver,
        secret,
        first,
        second,
    }
}
async fn assert_live(f: &mut Fixture, receiver: Option<(&str, i64)>, event: Option<&str>) {
    if let Some((receiver, revision)) = receiver {
        let row = sqlx::query("SELECT name,encrypted_key,revision,deletion_version_id FROM lane_webhook_receivers WHERE id=$1")
            .bind(receiver)
            .fetch_one(&mut f.db)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("name"), "moved-receiver");
        assert!(row.get::<Option<Vec<u8>>, _>("encrypted_key").is_some());
        assert_eq!(row.get::<i64, _>("revision"), revision);
        assert!(
            row.get::<Option<String>, _>("deletion_version_id")
                .is_none()
        );
    }
    if let Some(event) = event {
        let fetched = f.call("GetEvent", &json!({"id":event})).await.unwrap();
        assert!(fetched["event"]["payload"]["marker"].is_string());
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn receiver_reassignment_keeps_original_event_ownership_through_cascade_and_cleanup() {
    for historical in [false, true] {
        // Deleting C erases E1 only; R (now in D) and E2 survive, and new events still arrive.
        let mut f = Fixture::new().await;
        let m = moved(&mut f, historical).await;
        let canonical = f.repository.clone();
        f.delete_repository(&canonical).await;
        let direct = f.receiver.clone();
        let first_marker = format!("{MARKER}-first");
        assert_scrubbed(&mut f, &[&direct], &[&m.first], &[&first_marker]).await;
        assert_live(&mut f, Some((&m.receiver, 2)), Some(&m.second)).await;
        f.event_at(&m.receiver, &m.secret, "third").await;
        let pool = f.pool().await;
        let tombstones = f.tombstones().await;
        assert_eq!(
            webhooks::cleanup_deleted_repository_resources(&pool)
                .await
                .unwrap(),
            webhooks::CleanupReport::default()
        );
        assert_eq!(f.tombstones().await, tombstones);
        assert_live(&mut f, Some((&m.receiver, 2)), Some(&m.second)).await;
        pool.close().await;
        f.close().await;

        // Deleting D while C is live erases R and E2 only; E1 (originally C) survives.
        let mut f = Fixture::new().await;
        let m = moved(&mut f, historical).await;
        f.delete_repository(&m.destination).await;
        let receivers = [m.receiver.as_str()];
        let second_marker = format!("{MARKER}-second");
        assert_scrubbed(
            &mut f,
            &receivers,
            &[&m.second],
            &[&second_marker, "moved-receiver"],
        )
        .await;
        assert_live(&mut f, None, Some(&m.first)).await;
        let pool = f.pool().await;
        let tombstones = f.tombstones().await;
        assert_eq!(
            webhooks::cleanup_deleted_repository_resources(&pool)
                .await
                .unwrap(),
            webhooks::CleanupReport::default()
        );
        assert_eq!(f.tombstones().await, tombstones);
        assert_live(&mut f, None, Some(&m.first)).await;
        // E1 is deleted by its own repository later, through the alias spelling.
        f.delete_repository(&m.alias).await;
        let direct = f.receiver.clone();
        assert_scrubbed(
            &mut f,
            &[&m.receiver, &direct],
            &[&m.first, &m.second],
            &[MARKER, "moved-receiver"],
        )
        .await;
        pool.close().await;
        f.close().await;
    }
}

/// Everything account-visible about a webhook account, for exact before/after comparison.
async fn account_snapshot(f: &mut Fixture) -> Value {
    let account = f.principal.account_id.clone();
    let receivers: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(t) FROM lane_webhook_receivers t WHERE account_id=$1 ORDER BY id",
    )
    .bind(&account)
    .fetch_all(&mut f.db)
    .await
    .unwrap();
    let events: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(t) FROM lane_webhook_events t WHERE account_id=$1 ORDER BY id",
    )
    .bind(&account)
    .fetch_all(&mut f.db)
    .await
    .unwrap();
    let processing: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(t) FROM lane_webhook_processing t WHERE account_id=$1 ORDER BY id",
    )
    .bind(&account)
    .fetch_all(&mut f.db)
    .await
    .unwrap();
    let receipts: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(t) FROM lane_webhook_receipts t WHERE account_id=$1 ORDER BY method,request_id")
        .bind(&account)
        .fetch_all(&mut f.db)
        .await
        .unwrap();
    let changes: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(t) FROM lane_changes t WHERE account_id=$1 ORDER BY sequence",
    )
    .bind(&account)
    .fetch_all(&mut f.db)
    .await
    .unwrap();
    json!({"receivers":receivers,"events":events,"processing":processing,"receipts":receipts,"changes":changes})
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn cascade_and_cleanup_are_account_isolated_and_keep_receiver_only_deletion_semantics() {
    let mut f = Fixture::new().await;
    let first_principal = f.principal.clone();
    // Account B uses the same repository and alias spellings, including a historical alias row.
    let canonical = f.repository.clone();
    let alias = id();
    f.alias(&canonical, &alias).await;
    let second_principal = Principal {
        account_id: id(),
        tenant_id: id(),
        session_id: id(),
        device_id: id(),
        device_name: "Second account".into(),
        origin_kind: "ORIGIN_KIND_CLI".into(),
    };
    sqlx::query("INSERT INTO lane_accounts VALUES($1,$2,$3)")
        .bind(&second_principal.account_id)
        .bind(&second_principal.tenant_id)
        .bind(id())
        .execute(&mut f.db)
        .await
        .unwrap();
    f.principal = second_principal.clone();
    f.create_repository(&canonical, "https://example.com/test/repo")
        .await;
    f.alias(&canonical, &alias).await;
    let (b_receiver, b_secret) = f.receiver_at(&alias, "second-account").await;
    let b_event = f.event_at(&b_receiver, &b_secret, "second-account").await;
    f.set_stored_repository("lane_webhook_receivers", &b_receiver, &alias)
        .await;
    f.set_stored_repository("lane_webhook_events", &b_event, &alias)
        .await;
    let b_token = f.claim(&b_event).await;
    let before = account_snapshot(&mut f).await;
    f.principal = first_principal;

    // Account A: alias-owned content, a live second repository with a leased event whose
    // receiver is deleted alone, and a receiver/event ID collision.
    let (a_receiver, a_secret) = f.receiver_at(&alias, "first-account").await;
    let a_event = f.event_at(&a_receiver, &a_secret, "first-account").await;
    f.set_stored_repository("lane_webhook_receivers", &a_receiver, &alias)
        .await;
    f.set_stored_repository("lane_webhook_events", &a_event, &alias)
        .await;
    let keep = id();
    f.create_repository(&keep, "https://example.com/keep/repo")
        .await;
    let (keep_receiver, keep_secret) = f.receiver_at(&keep, "keep-receiver").await;
    let keep_event = f.event_at(&keep_receiver, &keep_secret, "keep").await;
    let keep_token = f.claim(&keep_event).await;
    f.call("DeleteReceiver", &mutation(&keep_receiver))
        .await
        .unwrap();
    assert_eq!(state(&mut f, &keep_event).await, "LEASED");
    let mut complete = mutation(&keep_event);
    complete["leaseToken"] = json!(keep_token);
    assert!(f.call("CompleteEvent", &complete).await.is_ok());
    // Collision: an event shares its ID with a receiver, and each owns receipts. The event and
    // its claim receipt are seeded directly because intake always generates fresh event IDs.
    let collision = format!("{MARKER}-collision");
    sqlx::query("INSERT INTO lane_webhook_events(account_id,id,receiver_id,repository_id,sender_event_id,fingerprint,data,state) VALUES($1,$2,$3,$4,'collision','fingerprint',$5,'PENDING')")
        .bind(&f.principal.account_id)
        .bind(&a_receiver)
        .bind(&a_receiver)
        .bind(&alias)
        .bind(json!({"eventType":"test","payload":{"marker":collision}}))
        .execute(&mut f.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO lane_webhook_receipts(account_id,method,request_id,resource_id,fingerprint,response) VALUES($1,'ClaimEvent',$2,$3,'fingerprint',$4)")
        .bind(&f.principal.account_id)
        .bind(id())
        .bind(&a_receiver)
        .bind(json!({"event":{"id":a_receiver,"state":"EVENT_STATE_LEASED","payload":{"marker":collision}}}))
        .execute(&mut f.db)
        .await
        .unwrap();

    f.delete_repository(&canonical).await;
    let direct = f.receiver.clone();
    let a_markers = [
        format!("{MARKER}-first-account"),
        format!("{MARKER}-collision"),
    ];
    assert_scrubbed(
        &mut f,
        &[&a_receiver, &direct],
        &[&a_event, &a_receiver],
        &[&a_markers[0], &a_markers[1], "first-account"],
    )
    .await;
    // Receipts keep separate redaction: the receiver's creation receipt is a receiver tombstone,
    // the colliding event's claim receipt an event tombstone.
    let created: Value = sqlx::query_scalar("SELECT response FROM lane_webhook_receipts WHERE method='CreateReceiver' AND resource_id=$1")
        .bind(&a_receiver)
        .fetch_one(&mut f.db)
        .await
        .unwrap();
    assert_eq!(
        created,
        json!({"receiver":{"id":a_receiver,"deleted":true}})
    );
    let claimed: Value = sqlx::query_scalar(
        "SELECT response FROM lane_webhook_receipts WHERE method='ClaimEvent' AND resource_id=$1",
    )
    .bind(&a_receiver)
    .fetch_one(&mut f.db)
    .await
    .unwrap();
    assert_eq!(claimed["event"]["state"], "EVENT_STATE_DELETED");
    assert!(claimed.get("deletion").is_none());

    let pool = f.pool().await;
    webhooks::cleanup_deleted_repository_resources(&pool)
        .await
        .unwrap();
    // Receiver-only deletion in A kept its event and lease; B is untouched by delete and cleanup.
    assert_eq!(state(&mut f, &keep_event).await, "COMPLETED");
    f.principal = second_principal;
    assert_eq!(account_snapshot(&mut f).await, before);
    let mut late = mutation(&b_event);
    late["leaseToken"] = json!(b_token);
    assert!(f.call("CompleteEvent", &late).await.is_ok());
    pool.close().await;
    f.close().await;
}

/// Immutable identity material that alias resolution must never rewrite.
async fn identities(f: &mut Fixture) -> Value {
    let versions: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(t) FROM lane_version_identities t ORDER BY version_id")
            .fetch_all(&mut f.db)
            .await
            .unwrap();
    let events: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('id',id,'receiver',receiver_id,'sender',sender_event_id,'fingerprint',fingerprint) FROM lane_webhook_events ORDER BY id",
    )
    .fetch_all(&mut f.db)
    .await
    .unwrap();
    let receipts: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('method',method,'request',request_id,'resource',resource_id,'fingerprint',fingerprint) FROM lane_webhook_receipts ORDER BY method,request_id",
    )
    .fetch_all(&mut f.db)
    .await
    .unwrap();
    json!({"versions":versions,"events":events,"receipts":receipts})
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL via LANE_TEST_DATABASE_URL"]
async fn alias_resolution_preserves_versions_fingerprints_and_replay_identities() {
    let mut f = Fixture::new().await;
    let s = seed(&mut f, true).await;
    let identities_before = identities(&mut f).await;
    let versions_before: Vec<Value> =
        sqlx::query_scalar("SELECT envelope FROM lane_versions ORDER BY version_id")
            .fetch_all(&mut f.db)
            .await
            .unwrap();
    // Reads and writes through every spelling leave immutable bodies and identities unchanged.
    f.list_all("ListReceivers", Some(&s.alias_a), 1).await;
    f.list_all("ListEvents", Some(&s.canonical), 1).await;
    f.call("GetEvent", &json!({"id":s.pending})).await.unwrap();
    let mut create = mutation(&id());
    create["repositoryId"] = json!(s.alias_b);
    create["name"] = json!("replayed");
    let created = f.call("CreateReceiver", &create).await.unwrap();
    let mut replay = created.clone();
    replay.as_object_mut().unwrap().remove("secret");
    assert_eq!(f.call("CreateReceiver", &create).await.unwrap(), replay);
    let mut changed = create.clone();
    changed["name"] = json!("changed content");
    assert_eq!(
        f.call("CreateReceiver", &changed).await.unwrap_err().code,
        connectrpc::ErrorCode::AlreadyExists
    );
    // Sender deduplication: identical content replays, changed content under one sender conflicts.
    let raw = serde_json::to_vec(&json!({"id":"sender-c06","type":"test","occurredAt":"2026-10-25T00:00:00Z","payload":{"marker":format!("{MARKER}-sender")}})).unwrap();
    let first = f.intake_to(&s.receiver, &s.secret, &raw).await.unwrap();
    assert_eq!(
        f.intake_to(&s.receiver, &s.secret, &raw).await.unwrap()["id"],
        first["id"]
    );
    let different = serde_json::to_vec(&json!({"id":"sender-c06","type":"test","occurredAt":"2026-10-25T00:00:00Z","payload":{"marker":"other"}})).unwrap();
    assert!(
        f.intake_to(&s.receiver, &s.secret, &different)
            .await
            .is_err()
    );
    let versions_after: Vec<Value> =
        sqlx::query_scalar("SELECT envelope FROM lane_versions ORDER BY version_id")
            .fetch_all(&mut f.db)
            .await
            .unwrap();
    assert_eq!(versions_after, versions_before);
    let identities_written = identities(&mut f).await;
    for key in ["versions", "events", "receipts"] {
        let before = identities_before[key].as_array().unwrap();
        let after = identities_written[key].as_array().unwrap();
        assert!(
            before.iter().all(|row| after.contains(row)),
            "{key} identities of existing rows are unchanged"
        );
    }

    // Deletion keeps minimal identities and fingerprints, and replays reveal no erased content.
    let mut completion = mutation(&s.leased);
    completion["leaseToken"] = json!(s.lease_token);
    f.call("CompleteEvent", &completion).await.unwrap();
    let identities_complete = identities(&mut f).await;
    f.delete_repository(&s.alias_b).await;
    let pool = f.pool().await;
    webhooks::cleanup_deleted_repository_resources(&pool)
        .await
        .unwrap();
    let identities_deleted = identities(&mut f).await;
    for key in ["events", "receipts"] {
        let before = identities_complete[key].as_array().unwrap();
        let after = identities_deleted[key].as_array().unwrap();
        assert!(
            before.iter().all(|row| after.contains(row)),
            "{key} fingerprints and replay identities survive deletion"
        );
    }
    let version_ids: Vec<&Value> = identities_complete["versions"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    assert!(
        version_ids.iter().all(|row| identities_deleted["versions"]
            .as_array()
            .unwrap()
            .contains(row)),
        "version identities survive repository deletion"
    );
    let deleted_replay = f.intake_to(&s.receiver, &s.secret, &raw).await.unwrap_err();
    // The receiver is deleted, so intake is rejected before replay and reveals nothing.
    assert!(!format!("{deleted_replay:?}").contains(MARKER));
    let replay = f.call("CompleteEvent", &completion).await.unwrap();
    assert_eq!(replay["event"]["state"], "EVENT_STATE_DELETED");
    assert!(replay.get("processing").is_none() && replay.get("tasks").is_none());
    assert!(!replay.to_string().contains(MARKER));
    assert_eq!(
        f.call("CreateReceiver", &create).await.unwrap()["receiver"]["deleted"],
        json!(true)
    );
    pool.close().await;
    f.close().await;
}
