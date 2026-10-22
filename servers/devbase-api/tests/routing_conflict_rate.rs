// SPDX-License-Identifier: FSL-1.1-ALv2
//! A routing conflict (a login-token mutation with a malformed target that matches the caller's
//! existing replay key) is returned only after rate limiting, so repeating it consumes the same
//! verified tenant or project bucket as a well-formed conflict. These tests drive the real router
//! with a disposable PostgreSQL schema and a mocked Delcore.
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::post,
};
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
        // Tiny refill rates make the bucket sizes exact: the management bucket holds 4 requests
        // (project creation and three conflicts) and the user bucket holds 3.
        let config = Config {
            projects: 100,
            users: 100,
            keys: 20,
            user_rate: 0.001,
            user_burst: 3,
            management_rate: 0.001,
            management_burst: 4,
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
        let server = Arc::new(Server::new(db, authority, config));
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

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn routing_conflicts_consume_the_recorded_tenant_and_project_buckets() {
    let fx = Harness::start().await;

    // Management bucket, keyed by the verified recorded tenant: the first request creates the
    // project and records the replay key; the malformed-target conflicts then drain the bucket.
    let (status, created) = fx
        .call(
            "CreateProject",
            json!({"tenantId":fx.tenant,"name":"Project","idempotencyKey":"conflict-create"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let project = created["project"]["id"].as_str().unwrap().to_owned();
    let conflict =
        json!({"tenantId":"not-a-uuid","name":"Project","idempotencyKey":"conflict-create"});
    for n in 0..3 {
        let (status, body) = fx.call("CreateProject", conflict.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{n}: {body}");
        assert_eq!(body["code"], "already_exists", "{n}");
    }
    // The fifth management request finds the bucket empty: the conflict no longer bypasses the
    // limiter, exactly as the conflict with a well-formed target does not.
    let (status, body) = fx.call("CreateProject", conflict.clone()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["code"], "resource_exhausted");
    let well_formed = json!({"tenantId":uuid::Uuid::now_v7().to_string(),"name":"Project","idempotencyKey":"conflict-create"});
    // A well-formed conflict for another tenant is limited by that tenant's own bucket.
    let (status, body) = fx.call("CreateProject", well_formed).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // User bucket, keyed by the verified recorded project, not by the malformed target text.
    let (status, user) = fx
        .call(
            "CreateEndUser",
            json!({"projectId":project,"externalId":"u1","idempotencyKey":"conflict-user"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{user}");
    let conflict =
        json!({"projectId":"not-a-uuid","externalId":"u1","idempotencyKey":"conflict-user"});
    let (status, body) = fx.call("CreateEndUser", conflict.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "already_exists");
    // Varying the malformed text must not select a fresh bucket.
    let other =
        json!({"projectId":"another-text","externalId":"u1","idempotencyKey":"conflict-user"});
    let (status, body) = fx.call("CreateEndUser", other).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = fx.call("CreateEndUser", conflict).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["code"], "resource_exhausted");

    fx.finish().await;
}
