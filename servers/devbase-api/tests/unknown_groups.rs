// SPDX-License-Identifier: FSL-1.1-ALv2
//! Unknown protobuf groups in binary mutations. The generated decoder keeps a balanced group, so the
//! replay fingerprint must hash it, and the fingerprint must not preempt the documented replay
//! precedence. These tests drive the real router with a disposable PostgreSQL schema and a mocked
//! Delcore.
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use buffa::Message;
use delino_protocol::devbase::v1::{
    CreateEndUserRequest, CreateEndUserResponse, CreateProjectRequest,
};
use devbase_api::{Server, config::Config, store};
use serde_json::{Value, json};
use server_common::authority::Authority;
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
