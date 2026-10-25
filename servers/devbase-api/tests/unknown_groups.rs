// SPDX-License-Identifier: FSL-1.1-ALv2
//! Unknown protobuf fields in binary mutations. The generated decoder keeps a balanced group, so the
//! replay fingerprint must hash it, and the fingerprint must not preempt the documented replay
//! precedence. The harness covers canonical `0x01`-prefixed replay fingerprints.
//! These tests drive the real router with a disposable PostgreSQL schema
//! and a mocked Delcore.
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use buffa::Message;
use delino_protocol::devbase::v1::{
    CreateEndUserRequest, CreateEndUserResponse, CreateProjectRequest, UpdateEndUserRequest,
};
use devbase_api::{Server, config::Config, store};
use serde_json::{Value, json};
use server_common::authority::Authority;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::{sync::Arc, time::Duration};

struct Reply {
    status: StatusCode,
    body: Vec<u8>,
}
impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "JSON reply expected ({e}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }
}

struct Harness {
    http: reqwest::Client,
    base: String,
    server: Arc<Server>,
    project: String,
    tenant: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    db: sqlx::PgPool,
    admin: sqlx::PgPool,
    schema: String,
}
impl Harness {
    async fn start() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("devbase_{}", uuid::Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = schema.clone();
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .after_connect(move |conn, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {search}"))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        store::migrate(&db).await.unwrap();
        let tenant = uuid::Uuid::now_v7().to_string();
        let actor = uuid::Uuid::now_v7().to_string();
        let mock = Router::new()
            .route(
                "/token",
                post(|| async { Json(json!({"access_token":"service","expires_in":3600})) }),
            )
            .route(
                "/delcore.v1.DelcoreService/GetCurrentUser",
                post(move |h: HeaderMap| {
                    let actor = actor.clone();
                    async move {
                        if h.get("authorization").and_then(|h| h.to_str().ok())
                            != Some("Bearer manager")
                        {
                            return (
                                StatusCode::UNAUTHORIZED,
                                Json(json!({"code":"unauthenticated"})),
                            );
                        }
                        (StatusCode::OK, Json(json!({"userId":actor})))
                    }
                }),
            )
            .route(
                "/delcore.v1.DelcoreService/ListMembers",
                post(|h: HeaderMap| async move {
                    if h.get("authorization").and_then(|h| h.to_str().ok())
                        == Some("Bearer manager")
                    {
                        (StatusCode::OK, Json(json!({})))
                    } else {
                        (
                            StatusCode::FORBIDDEN,
                            Json(json!({"code":"permission_denied"})),
                        )
                    }
                }),
            )
            .route(
                "/delcore.v1.DelcoreService/GetTenantStates",
                post(|Json(q): Json<Value>| async move {
                    let entries: Vec<_> = q["tenantIds"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|id| json!({"tenantId":id,"state":"TENANT_STATE_ACTIVE"}))
                        .collect();
                    Json(json!({"states":entries}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let delcore = format!("http://{}", listener.local_addr().unwrap());
        let mock_task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
        let config = Config {
            projects: 100,
            users: 100,
            keys: 20,
            user_rate: 10000.0,
            user_burst: 20000,
            management_rate: 10000.0,
            management_burst: 20000,
            dependency_timeout: Duration::from_secs(3),
            request_timeout: Duration::from_secs(10),
            cleanup_interval: Duration::from_secs(300),
            origin: "https://console.example".into(),
            listen: "127.0.0.1:0".into(),
        };
        let http = reqwest::Client::new();
        let authority = Authority::new(
            http.clone(),
            delcore.clone(),
            format!("{delcore}/token"),
            "resource".into(),
            "client".into(),
            "fixture".into(),
        );
        let server = Arc::new(Server::new(db.clone(), authority, config));
        let router = devbase_api::http::router(server.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut harness = Self {
            http,
            base: format!("http://{addr}/devbase.v1.DevBaseService"),
            server,
            project: String::new(),
            tenant: tenant.clone(),
            tasks: vec![mock_task, task],
            db,
            admin,
            schema,
        };
        let created = harness
            .send(
                "CreateProject",
                "application/json",
                &[],
                serde_json::to_vec(
                    &json!({"tenantId":tenant,"name":"Project","idempotencyKey":"group-project"}),
                )
                .unwrap(),
            )
            .await;
        assert_eq!(created.status, StatusCode::OK, "{}", created.json());
        harness.project = created.json()["project"]["id"].as_str().unwrap().into();
        harness
    }

    /// Posts `body` with a Content-Length as the manager.
    async fn send(
        &self,
        method: &str,
        content_type: &str,
        extra: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Reply {
        let mut request = self
            .http
            .post(format!("{}/{method}", self.base))
            .bearer_auth("manager")
            .header("Content-Type", content_type)
            .header("Connect-Protocol-Version", "1");
        for (name, value) in extra {
            request = request.header(*name, *value);
        }
        let response = request.body(body).send().await.unwrap();
        Reply {
            status: response.status(),
            body: response.bytes().await.unwrap().to_vec(),
        }
    }

    /// A mutation as binary Connect with the unknown bytes appended to the encoded request.
    async fn binary(&self, method: &str, request: &impl Message, extra: &[u8]) -> Reply {
        let mut body = request.encode_to_vec();
        body.extend_from_slice(extra);
        self.send(method, "application/proto", &[], body).await
    }

    async fn call(&self, method: &str, body: Value) -> Reply {
        self.send(
            method,
            "application/json",
            &[],
            serde_json::to_vec(&body).unwrap(),
        )
        .await
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.db).await.unwrap()
    }

    async fn finish(self) {
        for task in &self.tasks {
            task.abort();
        }
        drop(self.server);
        self.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

// Field 100 empty group, field 100 wrapping field 101, and the same shapes with other content.
const EMPTY: [u8; 4] = [0xa3, 0x06, 0xa4, 0x06];
const NESTED: [u8; 8] = [0xa3, 0x06, 0xab, 0x06, 0xac, 0x06, 0xa4, 0x06];
const WITH_VARINT: [u8; 6] = [0xa3, 0x06, 0x08, 0x01, 0xa4, 0x06];

fn user(project: &str, key: &str, external: &str) -> CreateEndUserRequest {
    serde_json::from_value(json!({
        "projectId": project,
        "externalId": external,
        "idempotencyKey": key
    }))
    .unwrap()
}

fn error_code(reply: &Reply) -> String {
    reply.json()["code"].as_str().unwrap().to_owned()
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn unknown_groups_are_fingerprinted_and_replay_precedence_is_kept() {
    let h = Harness::start().await;
    let pid = h.project.clone();

    // A balanced group passes: one user, one replay record, and the same bytes replay the stored
    // response without another mutation.
    let first = h
        .binary("CreateEndUser", &user(&pid, "group-k1", "ext-1"), &EMPTY)
        .await;
    assert_eq!(
        first.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&first.body)
    );
    assert_eq!(h.count("SELECT count(*) FROM end_users").await, 1);
    let retry = h
        .binary("CreateEndUser", &user(&pid, "group-k1", "ext-1"), &EMPTY)
        .await;
    assert_eq!(retry.status, StatusCode::OK);
    // The reply starts with a per-request trace field; the stored end user must be identical.
    let end_user = |reply: &Reply| {
        format!(
            "{:?}",
            CreateEndUserResponse::decode_from_slice(&reply.body)
                .unwrap()
                .end_user
        )
    };
    assert_eq!(end_user(&retry), end_user(&first));
    assert_eq!(h.count("SELECT count(*) FROM end_users").await, 1);
    assert_eq!(
        h.count("SELECT count(*) FROM replays WHERE idempotency_key='group-k1'")
            .await,
        1
    );

    // The group is part of the fingerprint: changed or removed group content conflicts.
    for (what, extra) in [
        ("nested group", &NESTED[..]),
        ("group content", &WITH_VARINT[..]),
        ("no group", &[][..]),
    ] {
        let reply = h
            .binary("CreateEndUser", &user(&pid, "group-k1", "ext-1"), extra)
            .await;
        assert_eq!(error_code(&reply), "already_exists", "{what}");
    }
    assert_eq!(h.count("SELECT count(*) FROM end_users").await, 1);

    // A group inside the known metadata Struct is retained as opaque data.
    let mut metadata_request = user(&pid, "group-k2", "ext-2");
    let mut bytes = metadata_request.encode_to_vec();
    bytes.extend_from_slice(&[0x32, EMPTY.len() as u8]);
    bytes.extend_from_slice(&EMPTY);
    let reply = h
        .send("CreateEndUser", "application/proto", &[], bytes.clone())
        .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    let reply = h
        .send("CreateEndUser", "application/proto", &[], bytes)
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(h.count("SELECT count(*) FROM end_users").await, 2);
    // Without the group the same key is a different request.
    metadata_request.metadata = Default::default();
    let mut plain = metadata_request.encode_to_vec();
    plain.extend_from_slice(&[0x32, 0x00]);
    let reply = h
        .send("CreateEndUser", "application/proto", &[], plain)
        .await;
    assert_eq!(error_code(&reply), "already_exists");

    // Operation, tenant and project conflicts win over the payload, with a group present.
    let other = h
        .call(
            "CreateProject",
            json!({"tenantId":h.tenant,"name":"Other","idempotencyKey":"group-other"}),
        )
        .await;
    let other_pid = other.json()["project"]["id"].as_str().unwrap().to_owned();
    let reply = h
        .binary("CreateEndUser", &user(&other_pid, "group-k1", ""), &EMPTY)
        .await;
    assert_eq!(error_code(&reply), "already_exists", "project conflict");
    let project_request: CreateProjectRequest =
        serde_json::from_value(json!({"tenantId":h.tenant,"name":"","idempotencyKey":"group-k1"}))
            .unwrap();
    let reply = h.binary("CreateProject", &project_request, &EMPTY).await;
    assert_eq!(error_code(&reply), "already_exists", "operation conflict");

    // Invalid payloads keep `invalid_argument`, with and without a matching record, and add
    // nothing.
    let replays = h.count("SELECT count(*) FROM replays").await;
    let reply = h
        .binary("CreateEndUser", &user(&pid, "group-k1", ""), &EMPTY)
        .await;
    assert_eq!(error_code(&reply), "invalid_argument", "matching record");
    let reply = h
        .binary("CreateEndUser", &user(&pid, "group-fresh", ""), &EMPTY)
        .await;
    assert_eq!(error_code(&reply), "invalid_argument", "no record");
    assert_eq!(h.count("SELECT count(*) FROM replays").await, replays);

    // A scrubbed record answers `not_found` whatever the payload, including a group-bearing
    // request that would otherwise be invalid, and stays scrubbed.
    let created = h
        .call(
            "CreateEndUser",
            json!({"projectId":pid,"externalId":"ext-scrub","idempotencyKey":"group-scrub"}),
        )
        .await;
    let uid = created.json()["endUser"]["id"].as_str().unwrap().to_owned();
    let deleted = h
        .call(
            "DeleteEndUser",
            json!({"projectId":pid,"endUserId":uid,"version":"1","idempotencyKey":"group-del"}),
        )
        .await;
    assert_eq!(
        deleted.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&deleted.body)
    );
    let users = h.count("SELECT count(*) FROM end_users").await;
    let replays = h.count("SELECT count(*) FROM replays").await;
    for (what, external) in [("valid", "ext-scrub"), ("invalid", "")] {
        let reply = h
            .binary(
                "CreateEndUser",
                &user(&pid, "group-scrub", external),
                &EMPTY,
            )
            .await;
        assert_eq!(error_code(&reply), "not_found", "scrubbed {what}");
    }
    assert_eq!(h.count("SELECT count(*) FROM end_users").await, users);
    assert_eq!(h.count("SELECT count(*) FROM replays").await, replays);
    let row =
        sqlx::query("SELECT scrubbed, response FROM replays WHERE idempotency_key='group-scrub'")
            .fetch_one(&h.db)
            .await
            .unwrap();
    assert!(row.get::<bool, _>("scrubbed"));
    assert!(row.get::<Option<Vec<u8>>, _>("response").is_none());
    // A different project still conflicts for the scrubbed key.
    let reply = h
        .binary(
            "CreateEndUser",
            &user(&other_pid, "group-scrub", ""),
            &EMPTY,
        )
        .await;
    assert_eq!(
        error_code(&reply),
        "already_exists",
        "scrubbed project conflict"
    );

    // Malformed groups are rejected by the decoder before any mutation.
    let users = h.count("SELECT count(*) FROM end_users").await;
    let replays = h.count("SELECT count(*) FROM replays").await;
    for (what, extra) in [
        ("unterminated", &[0xa3u8, 0x06][..]),
        ("mismatched", &[0xa3, 0x06, 0xac, 0x06][..]),
        ("top-level end", &[0xa4, 0x06][..]),
    ] {
        let reply = h
            .binary("CreateEndUser", &user(&pid, "group-bad", "ext-bad"), extra)
            .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{what}");
        assert_eq!(error_code(&reply), "invalid_argument", "{what}");
    }
    assert_eq!(h.count("SELECT count(*) FROM end_users").await, users);
    assert_eq!(h.count("SELECT count(*) FROM replays").await, replays);

    h.finish().await;
}

// A metadata Struct with the known entry {"a":true}, an unknown varint field 100 and two opaque
// occurrences of unknown length-delimited field 100.
const ENTRY_A: [u8; 9] = [0x0a, 0x07, 0x0a, 0x01, 0x61, 0x12, 0x02, 0x20, 0x01];
const UNKNOWN_VARINT: [u8; 3] = [0xa0, 0x06, 0x01];
const OPAQUE_B: [u8; 6] = [0xa2, 0x06, 0x03, 0x0a, 0x01, 0x62];
const OPAQUE_C: [u8; 6] = [0xa2, 0x06, 0x03, 0x0a, 0x01, 0x63];

fn field(number: u8, payload: &[u8]) -> Vec<u8> {
    assert!(number < 16 && payload.len() < 128);
    [&[number << 3 | 2, payload.len() as u8][..], payload].concat()
}
fn sha(operation: &str, encoded: &[u8]) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(operation);
    hash.update([0]);
    hash.update(encoded);
    hash.finalize().to_vec()
}
/// Binary CreateEndUser bytes (the key included) with `metadata` as field 6.
fn user_with_metadata(project: &str, key: &str, external: &str, metadata: &[u8]) -> Vec<u8> {
    [
        user(project, key, external).encode_to_vec(),
        field(6, metadata),
    ]
    .concat()
}
/// The hashed bytes of the same request: no key and the given canonical metadata order.
fn user_hash_input(project: &str, external: &str, metadata: &[u8]) -> Vec<u8> {
    [
        user(project, "", external).encode_to_vec(),
        field(6, metadata),
    ]
    .concat()
}

/// Binary UpdateEndUser bytes whose masked metadata is `metadata`, in field-number order.
fn update_bytes(project: &str, user: &str, key: &str, metadata: &[u8]) -> Vec<u8> {
    let head: UpdateEndUserRequest =
        serde_json::from_value(json!({"projectId":project,"endUserId":user,"idempotencyKey":key}))
            .unwrap();
    let tail: UpdateEndUserRequest =
        serde_json::from_value(json!({"updateMask":"metadata","version":"1"})).unwrap();
    [
        head.encode_to_vec(),
        field(7, metadata),
        tail.encode_to_vec(),
    ]
    .concat()
}

#[derive(Debug, PartialEq)]
struct ReplayRow {
    fingerprint: Vec<u8>,
    response: Option<Vec<u8>>,
    scrubbed: bool,
    tenant_id: uuid::Uuid,
    project_id: Option<uuid::Uuid>,
    target_user_id: Option<uuid::Uuid>,
    created_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
}
impl Harness {
    async fn replay(&self, key: &str) -> ReplayRow {
        let row = sqlx::query("SELECT fingerprint,response,scrubbed,tenant_id,project_id,target_user_id,created_at,expires_at FROM replays WHERE idempotency_key=$1")
            .bind(key)
            .fetch_one(&self.db)
            .await
            .unwrap();
        ReplayRow {
            fingerprint: row.get("fingerprint"),
            response: row.get("response"),
            scrubbed: row.get("scrubbed"),
            tenant_id: row.get("tenant_id"),
            project_id: row.get("project_id"),
            target_user_id: row.get("target_user_id"),
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
        }
    }
    async fn set_fingerprint(&self, key: &str, fingerprint: &[u8]) {
        sqlx::query("UPDATE replays SET fingerprint=$2 WHERE idempotency_key=$1")
            .bind(key)
            .bind(fingerprint)
            .execute(&self.db)
            .await
            .unwrap();
    }
    async fn proto(&self, method: &str, body: Vec<u8>) -> Reply {
        self.send(method, "application/proto", &[], body).await
    }
    /// Counters that a replay must leave alone.
    async fn state(&self) -> (i64, i64, i64) {
        (
            self.count("SELECT count(*) FROM end_users").await,
            self.count("SELECT count(*) FROM replays").await,
            self.count("SELECT count(*) FROM audit_events").await,
        )
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn replay_fingerprints_require_canonical_v1() {
    let h = Harness::start().await;
    let pid = h.project.clone();
    let go_metadata = [&ENTRY_A[..], &UNKNOWN_VARINT[..]].concat();

    // A new record stores 0x01 followed by the Go-compatible digest.
    let created = h
        .proto(
            "CreateEndUser",
            user_with_metadata(&pid, "format-new", "ext-new", &go_metadata),
        )
        .await;
    assert_eq!(
        created.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&created.body)
    );
    let go_digest = sha(
        "CreateEndUser",
        &user_hash_input(&pid, "ext-new", &go_metadata),
    );
    let new_row = h.replay("format-new").await;
    assert_eq!(new_row.fingerprint, [&[1u8][..], &go_digest].concat());
    // The new row compares only the corrected digest: the retry replays, other input conflicts.
    let before = h.state().await;
    let retry = h
        .proto(
            "CreateEndUser",
            user_with_metadata(&pid, "format-new", "ext-new", &go_metadata),
        )
        .await;
    assert_eq!(retry.status, StatusCode::OK);
    let changed = [&ENTRY_A[..], &[0xa0, 0x06, 0x02][..]].concat();
    for (what, request) in [
        (
            "unknown value",
            user_with_metadata(&pid, "format-new", "ext-new", &changed),
        ),
        (
            "known input",
            user_with_metadata(&pid, "format-new", "ext-other", &go_metadata),
        ),
    ] {
        let reply = h.proto("CreateEndUser", request).await;
        assert_eq!(error_code(&reply), "already_exists", "{what}");
    }
    assert_eq!(h.state().await, before);
    assert_eq!(h.replay("format-new").await, new_row);

    // Opaque unknown-field order remains significant.
    let bc = [&ENTRY_A[..], &OPAQUE_B, &OPAQUE_C].concat();
    let cb = [&ENTRY_A[..], &OPAQUE_C, &OPAQUE_B].concat();
    let first = h
        .proto(
            "CreateEndUser",
            user_with_metadata(&pid, "format-bc", "ext-bc", &bc),
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let bc_digest = sha("CreateEndUser", &user_hash_input(&pid, "ext-bc", &bc));
    let bc_row = h.replay("format-bc").await;
    assert_eq!(bc_row.fingerprint, [&[1u8][..], &bc_digest].concat());
    let reply = h
        .proto(
            "CreateEndUser",
            user_with_metadata(&pid, "format-bc", "ext-bc", &cb),
        )
        .await;
    assert_eq!(error_code(&reply), "already_exists", "CB against new BC");
    let reply = h
        .proto(
            "CreateEndUser",
            user_with_metadata(&pid, "format-bc", "ext-bc", &bc),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK, "BC against new BC");
    assert_eq!(h.replay("format-bc").await, bc_row);
    // Unsupported stored formats fail safely as unavailable, with nothing changed.
    let request = user_with_metadata(&pid, "format-new", "ext-new", &go_metadata);
    let supported = h.replay("format-new").await.fingerprint;
    for (what, stored) in [
        ("short", supported[..31].to_vec()),
        ("unversioned", supported[1..].to_vec()),
        ("long", [&supported[..], &[0][..]].concat()),
        ("empty", Vec::new()),
        ("marker", [&[2u8][..], &supported[1..]].concat()),
    ] {
        h.set_fingerprint("format-new", &stored).await;
        let before = h.state().await;
        let reply = h.proto("CreateEndUser", request.clone()).await;
        assert_eq!(error_code(&reply), "unavailable", "{what}");
        assert!(
            stored.is_empty()
                || !String::from_utf8_lossy(&reply.body).contains(&hex::encode(&stored)),
            "{what}"
        );
        assert_eq!(h.state().await, before, "{what}");
        // The checks that come first are unchanged: an invalid payload and a routing conflict win.
        let invalid = user_with_metadata(&pid, "format-new", "", &go_metadata);
        assert_eq!(
            error_code(&h.proto("CreateEndUser", invalid).await),
            "invalid_argument",
            "{what} payload"
        );
        let json = h
            .call(
                "CreateProject",
                json!({"tenantId":h.tenant,"name":"","idempotencyKey":"format-new"}),
            )
            .await;
        assert_eq!(error_code(&json), "already_exists", "{what} routing");
    }
    // An expired record is no record, so a bad stored format is irrelevant and a new canonical
    // row replaces it.
    sqlx::query(
        "UPDATE replays SET expires_at=now()-interval '1 second' WHERE idempotency_key='format-new'",
    )
    .execute(&h.db)
    .await
    .unwrap();
    let reply = h
        .proto(
            "CreateEndUser",
            user_with_metadata(&pid, "format-new", "ext-after-expiry", &go_metadata),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(h.replay("format-new").await.fingerprint.len(), 33);

    // A scrubbed record has an empty fingerprint and answers not_found, never unavailable.
    let created = h
        .call(
            "CreateEndUser",
            json!({"projectId":pid,"externalId":"ext-format-scrub","idempotencyKey":"format-scrub"}),
        )
        .await;
    let uid = created.json()["endUser"]["id"].as_str().unwrap().to_owned();
    let deleted = h
        .call(
            "DeleteEndUser",
            json!({"projectId":pid,"endUserId":uid,"version":"1","idempotencyKey":"format-delete"}),
        )
        .await;
    assert_eq!(deleted.status, StatusCode::OK);
    let scrubbed = h.replay("format-scrub").await;
    assert!(scrubbed.scrubbed && scrubbed.fingerprint.is_empty());
    let reply = h
        .call(
            "CreateEndUser",
            json!({"projectId":pid,"externalId":"ext-format-scrub","idempotencyKey":"format-scrub"}),
        )
        .await;
    assert_eq!(error_code(&reply), "not_found");

    // A delete record replays without changing its stored fingerprint.
    let delete =
        json!({"projectId":pid,"endUserId":uid,"version":"1","idempotencyKey":"format-delete"});
    let delete_row = h.replay("format-delete").await;
    assert_eq!(delete_row.fingerprint.len(), 33);
    let before = h.state().await;
    assert_eq!(
        h.call("DeleteEndUser", delete.clone()).await.status,
        StatusCode::OK
    );
    assert_eq!(h.state().await, before);
    assert_eq!(h.replay("format-delete").await, delete_row);

    // Updates with masked metadata use the same canonical fingerprint.
    for (n, external) in ["ext-upd"].into_iter().enumerate() {
        let created = h
            .call(
                "CreateEndUser",
                json!({"projectId":pid,"externalId":external,"idempotencyKey":format!("format-upd-create-{n}")}),
            )
            .await;
        let uid = created.json()["endUser"]["id"].as_str().unwrap().to_owned();
        let key = format!("format-upd-{n}");
        let bytes = update_bytes(&pid, &uid, &key, &go_metadata);
        let reply = h.proto("UpdateEndUser", bytes.clone()).await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let stored = h.replay(&key).await;
        assert_eq!(stored.fingerprint.len(), 33);
        let digest = sha("UpdateEndUser", &update_bytes(&pid, &uid, "", &go_metadata));
        assert_eq!(stored.fingerprint[1..], digest[..]);
        let before = h.state().await;
        assert_eq!(h.proto("UpdateEndUser", bytes).await.status, StatusCode::OK);
        assert_eq!(h.state().await, before);
        assert_eq!(h.replay(&key).await, stored);
    }

    h.finish().await;
}
