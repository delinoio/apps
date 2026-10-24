// SPDX-License-Identifier: FSL-1.1-ALv2
//! UUID v7 identifiers must carry the RFC 4122 variant, not only version nibble 7. These tests
//! drive the real router with a disposable PostgreSQL schema and a mocked Delcore and cover the
//! resource, cursor, compact-key and login-token mutation boundaries.
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use devbase_api::{Server, config::Config, store};
use serde_json::{Value, json};
use server_common::authority::Authority;
use std::{sync::Arc, time::Duration};

struct Harness {
    http: reqwest::Client,
    base: String,
    tenant: String,
    admin: sqlx::PgPool,
    schema: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Harness {
    async fn start(user_burst: u32, management_burst: u32) -> Self {
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
        let actor = uuid::Uuid::now_v7().to_string();
        let mock_actor = actor.clone();
        let mock = Router::new()
            .route(
                "/token",
                post(|| async { Json(json!({"access_token":"service","expires_in":3600})) }),
            )
            .route(
                "/delcore.v1.DelcoreService/GetCurrentUser",
                post(move |h: HeaderMap| {
                    let actor = mock_actor.clone();
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
        // Tiny refill rates make the bucket sizes exact for the bursts the caller selects.
        let config = Config {
            projects: 100,
            users: 100,
            keys: 20,
            user_rate: 0.001,
            user_burst,
            management_rate: 0.001,
            management_burst,
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
        let router = devbase_api::http::router(server);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!(
            "http://{}/devbase.v1.DevBaseService",
            listener.local_addr().unwrap()
        );
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            http,
            base,
            tenant: uuid::Uuid::now_v7().to_string(),
            admin,
            schema,
            tasks: vec![mock_task, task],
        }
    }

    async fn call(&self, method: &str, body: Value) -> (StatusCode, Value) {
        let response = self
            .http
            .post(format!("{}/{method}", self.base))
            .bearer_auth("manager")
            .header("Connect-Protocol-Version", "1")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap())
    }

    async fn finish(self) {
        for task in &self.tasks {
            task.abort();
        }
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
    }
}

/// Fourth-group leading nibbles 0-7 and c-f give version-7 text without the RFC 4122 variant.
fn non_rfc_vectors() -> Vec<String> {
    (0..16u8)
        .filter(|n| !(8..12).contains(n))
        .map(|n| format!("01900000-0000-7000-{n:x}000-000000000001"))
        .collect()
}

impl Harness {
    async fn call_as(&self, bearer: &str, method: &str, body: Value) -> (StatusCode, Value) {
        let response = self
            .http
            .post(format!("{}/{method}", self.base))
            .bearer_auth(bearer)
            .header("Connect-Protocol-Version", "1")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap())
    }
}

fn assert_invalid(status: StatusCode, body: &Value, message: &str, label: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {body}");
    assert_eq!(body["code"], "invalid_argument", "{label}: {body}");
    assert_eq!(body["message"], message, "{label}: {body}");
    assert!(!body["details"].as_array().unwrap().is_empty(), "{label}");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn resource_cursor_and_key_ids_enforce_the_rfc_variant() {
    let fx = Harness::start(1000, 1000).await;
    let (status, created) = fx
        .call(
            "CreateProject",
            json!({"tenantId":fx.tenant,"name":"Project","idempotencyKey":"variant-project"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let project = created["project"]["id"].as_str().unwrap().to_owned();
    let mut users = vec![];
    for n in 0..2 {
        let (status, user) = fx
            .call(
                "CreateEndUser",
                json!({"projectId":project,"externalId":format!("u{n}"),"idempotencyKey":format!("variant-user-{n}")}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{user}");
        users.push(user["endUser"]["id"].as_str().unwrap().to_owned());
    }
    let (status, issued) = fx
        .call(
            "IssueApiKey",
            json!({"projectId":project,"permission":"API_KEY_PERMISSION_CRUD","idempotencyKey":"variant-key"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let secret = issued["secret"].as_str().unwrap().to_owned();
    let key_id = issued["apiKey"]["id"].as_str().unwrap().to_owned();

    for vector in non_rfc_vectors() {
        // Resource IDs keep their operation-validation boundary and typed message.
        for (method, body, message) in [
            (
                "GetEndUser",
                json!({"projectId":project,"endUserId":vector}),
                "end_user_id must be a canonical UUID v7",
            ),
            (
                "GetApiKey",
                json!({"projectId":project,"apiKeyId":vector}),
                "api_key_id must be a canonical UUID v7",
            ),
            (
                "DeleteEndUser",
                json!({"projectId":project,"endUserId":vector,"version":"1","idempotencyKey":"variant-delete"}),
                "end_user_id must be a canonical UUID v7",
            ),
            (
                "RevokeApiKey",
                json!({"projectId":project,"apiKeyId":vector,"version":"1","idempotencyKey":"variant-revoke"}),
                "api_key_id must be a canonical UUID v7",
            ),
        ] {
            let (status, reply) = fx.call(method, body).await;
            assert_invalid(status, &reply, message, &format!("{method} {vector}"));
        }
        // A malformed idempotency key still wins over the resource ID.
        let (status, reply) = fx
            .call(
                "DeleteEndUser",
                json!({"projectId":project,"endUserId":vector,"version":"1","idempotencyKey":"short"}),
            )
            .await;
        assert_invalid(status, &reply, "invalid idempotency_key", "short key");
        // Cursor IDs.
        let token = URL_SAFE_NO_PAD.encode(format!("0.{vector}"));
        for method in ["ListEndUsers", "ListApiKeys"] {
            let (status, reply) = fx
                .call(method, json!({"projectId":project,"pageToken":token}))
                .await;
            assert_invalid(
                status,
                &reply,
                "page_token is invalid",
                &format!("{method} {vector}"),
            );
        }
        let (status, reply) = fx
            .call(
                "ListProjects",
                json!({"tenantId":fx.tenant,"pageToken":token}),
            )
            .await;
        assert_invalid(status, &reply, "page_token is invalid", "ListProjects");
        // A compact key with this ID is unauthenticated before any key lookup.
        let forged = format!("dbk_{}_{}", vector.replace('-', ""), "A".repeat(43));
        let (status, reply) = fx
            .call_as(
                &forged,
                "GetEndUser",
                json!({"projectId":project,"endUserId":users[0]}),
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{reply}");
        assert_eq!(reply["code"], "unauthenticated");
        // A key request rejects a non-RFC target at once, even with a usable key.
        let (status, reply) = fx
            .call_as(
                &secret,
                "GetEndUser",
                json!({"projectId":vector,"endUserId":users[0]}),
            )
            .await;
        assert_invalid(
            status,
            &reply,
            "target must be a canonical UUID v7",
            "key target",
        );
        // A read rejects a non-RFC target at once with a login token.
        let (status, reply) = fx.call("GetProject", json!({"projectId":vector})).await;
        assert_invalid(
            status,
            &reply,
            "target must be a canonical UUID v7",
            "read target",
        );
    }

    // Valid IDs, server-issued cursors and issued keys keep their behavior.
    let (status, reply) = fx
        .call(
            "GetEndUser",
            json!({"projectId":project,"endUserId":users[0]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let (status, reply) = fx
        .call(
            "GetEndUser",
            json!({"projectId":project,"endUserId":uuid::Uuid::now_v7().to_string()}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{reply}");
    let (status, reply) = fx
        .call("GetApiKey", json!({"projectId":project,"apiKeyId":key_id}))
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let (status, page) = fx
        .call("ListEndUsers", json!({"projectId":project,"pageSize":1}))
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let next = page["nextPageToken"].as_str().expect("server cursor");
    let (status, second) = fx
        .call(
            "ListEndUsers",
            json!({"projectId":project,"pageSize":1,"pageToken":next}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["endUsers"].as_array().unwrap().len(), 1);
    // An RFC-variant cursor ID parses.
    let token = URL_SAFE_NO_PAD.encode(format!("0.{}", uuid::Uuid::now_v7()));
    let (status, reply) = fx
        .call(
            "ListEndUsers",
            json!({"projectId":project,"pageToken":token}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let (status, reply) = fx
        .call_as(
            &secret,
            "GetEndUser",
            json!({"projectId":project,"endUserId":users[1]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");

    fx.finish().await;
}

// A version-7 target without the RFC variant is malformed for a login-token mutation exactly like
// any other malformed target: it enters the deferred replay-routing path instead of an
// unconditional rejection, and rate limiting still precedes the routing conflict.
#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn login_mutations_with_a_non_rfc_target_use_the_deferred_replay_route() {
    let fx = Harness::start(3, 4).await;
    let vectors = non_rfc_vectors();
    let target = "target must be a canonical UUID v7";

    // Fresh idempotency key: no replay record, so the target is invalid_argument.
    for vector in &vectors {
        let (status, reply) = fx
            .call(
                "CreateProject",
                json!({"tenantId":vector,"name":"Project","idempotencyKey":"variant-fresh"}),
            )
            .await;
        assert_invalid(status, &reply, target, &format!("fresh tenant {vector}"));
        let (status, reply) = fx
            .call(
                "CreateEndUser",
                json!({"projectId":vector,"externalId":"u","idempotencyKey":"variant-fresh"}),
            )
            .await;
        assert_invalid(status, &reply, target, &format!("fresh project {vector}"));
    }
    // Invalid credentials are decided before the target; a malformed key before the replay lookup.
    let (status, reply) = fx
        .call_as(
            "wrong",
            "CreateProject",
            json!({"tenantId":vectors[0],"name":"Project","idempotencyKey":"variant-fresh"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{reply}");
    assert_eq!(reply["code"], "unauthenticated");
    let (status, reply) = fx
        .call(
            "CreateProject",
            json!({"tenantId":vectors[0],"name":"Project","idempotencyKey":"short"}),
        )
        .await;
    assert_invalid(status, &reply, "invalid idempotency_key", "short key");

    // Existing replay route: the recorded tenant makes the malformed target a conflict. The
    // management bucket holds four requests: the creation and three conflicts.
    let (status, created) = fx
        .call(
            "CreateProject",
            json!({"tenantId":fx.tenant,"name":"Project","idempotencyKey":"variant-create"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let project = created["project"]["id"].as_str().unwrap().to_owned();
    for (n, vector) in vectors.iter().take(3).enumerate() {
        let (status, reply) = fx
            .call(
                "CreateProject",
                json!({"tenantId":vector,"name":"Project","idempotencyKey":"variant-create"}),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{n}: {reply}");
        assert_eq!(reply["code"], "already_exists", "{n}");
    }
    // Exhausted rate capacity wins over the routing conflict.
    let (status, reply) = fx
        .call(
            "CreateProject",
            json!({"tenantId":vectors[3],"name":"Project","idempotencyKey":"variant-create"}),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{reply}");
    assert_eq!(reply["code"], "resource_exhausted");

    // The same holds for end-user mutations, charged to the recorded project bucket (three).
    let (status, user) = fx
        .call(
            "CreateEndUser",
            json!({"projectId":project,"externalId":"u1","idempotencyKey":"variant-user"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{user}");
    for vector in vectors.iter().take(2) {
        let (status, reply) = fx
            .call(
                "CreateEndUser",
                json!({"projectId":vector,"externalId":"u1","idempotencyKey":"variant-user"}),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{reply}");
        assert_eq!(reply["code"], "already_exists");
    }
    let (status, reply) = fx
        .call(
            "CreateEndUser",
            json!({"projectId":vectors[2],"externalId":"u1","idempotencyKey":"variant-user"}),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{reply}");
    assert_eq!(reply["code"], "resource_exhausted");

    fx.finish().await;
}
