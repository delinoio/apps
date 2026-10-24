// SPDX-License-Identifier: FSL-1.1-ALv2
//! A rate-limit rejection through the real DevBase HTTP router carries the typed
//! `ErrorDetail` (reason, trace ID and `retry_after`), the `X-Devbase-Trace-Id` header and a
//! `Retry-After` header that describes the same wait, for JSON and binary unary requests. The
//! pool is lazy and never connects: the first request of each bucket ends in request validation,
//! after the limiter charged it, and the second finds the bucket empty. Delcore is a local mock.
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use base64::Engine;
use buffa::Message;
use delino_protocol::devbase::v1::{ErrorDetail, ListProjectsRequest};
use devbase_api::{
    Server,
    config::{Config, MAX_RETRY_SECONDS},
};
use serde_json::{Value, json};
use server_common::authority::Authority;
use std::{sync::Arc, time::Duration};

const PATH: &str = "/devbase.v1.DevBaseService/ListProjects";

struct Fixture {
    http: reqwest::Client,
    base: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn fixture(management_rate: f64) -> Fixture {
    let mock = Router::new()
        .route(
            "/token",
            post(|| async { Json(json!({"access_token":"service","expires_in":3600})) }),
        )
        .route(
            "/delcore.v1.DelcoreService/GetCurrentUser",
            post(|| async { Json(json!({"userId":"01900000-0000-7000-8000-00000000000a"})) }),
        )
        .route(
            "/delcore.v1.DelcoreService/ListMembers",
            post(|h: HeaderMap| async move {
                if h.get("authorization").and_then(|h| h.to_str().ok()) == Some("Bearer manager") {
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
        user_rate: 1000.0,
        user_burst: 2000,
        management_rate,
        management_burst: 1,
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
    let db = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://postgres@127.0.0.1:1/unused")
        .unwrap();
    let router = devbase_api::http::router(Arc::new(Server::new(db, authority, config)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Fixture {
        http,
        base,
        tasks: vec![mock_task, task],
    }
}

#[derive(Clone, Copy, Debug)]
enum Codec {
    Json,
    Binary,
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Fixture {
    /// Sends a `ListProjects` request for `tenant` whose page size is invalid, so a request the
    /// limiter admits ends in `invalid_argument` before any database access.
    async fn list_projects(&self, codec: Codec, tenant: &str, page_size: i32) -> Reply {
        let request = self
            .http
            .post(format!("{}{PATH}", self.base))
            .bearer_auth("manager")
            .header("Connect-Protocol-Version", "1");
        let request = match codec {
            Codec::Json => request
                .header("Content-Type", "application/json")
                .body(json!({"tenantId": tenant, "pageSize": page_size}).to_string()),
            Codec::Binary => request.header("Content-Type", "application/proto").body(
                ListProjectsRequest {
                    tenant_id: tenant.into(),
                    page_size,
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
        };
        let response = request.send().await.unwrap();
        Reply {
            status: response.status(),
            headers: response.headers().clone(),
            body: response.json().await.unwrap(),
        }
    }
}

fn detail_of(reply: &Reply) -> ErrorDetail {
    assert_eq!(reply.body["details"][0]["type"], "devbase.v1.ErrorDetail");
    let raw = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(reply.body["details"][0]["value"].as_str().unwrap())
        .unwrap();
    ErrorDetail::decode_from_slice(&raw).unwrap()
}

/// Exhausts the one-request bucket of a fresh tenant and checks the rate-limit rejection.
async fn assert_rate_limited(fx: &Fixture, codec: Codec) {
    let tenant = uuid::Uuid::now_v7().to_string();
    // The first request is admitted by the limiter and rejected by validation.
    let admitted = fx.list_projects(codec, &tenant, 101).await;
    assert_eq!(admitted.status, StatusCode::BAD_REQUEST, "{codec:?}");
    assert_eq!(admitted.body["code"], "invalid_argument", "{codec:?}");

    let reply = fx.list_projects(codec, &tenant, 101).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS, "{codec:?}");
    assert_eq!(reply.body["code"], "resource_exhausted", "{codec:?}");
    let detail = detail_of(&reply);
    assert_eq!(
        detail.reason.to_i32(),
        7,
        "{codec:?}: ERROR_REASON_RATE_LIMITED"
    );
    let trace = reply.headers["x-devbase-trace-id"].to_str().unwrap();
    assert!(uuid::Uuid::parse_str(trace).is_ok(), "{codec:?}");
    assert_eq!(detail.trace_id, trace, "{codec:?}");

    let retry = detail.retry_after.as_option().expect("retry_after");
    assert!(
        (0..=MAX_RETRY_SECONDS as i64).contains(&retry.seconds),
        "{codec:?}"
    );
    assert!((0..=999_999_999).contains(&retry.nanos), "{codec:?}");
    // The header rounds the same wait up to whole seconds, with a minimum of one.
    let wait = retry.seconds as f64 + f64::from(retry.nanos) / 1e9;
    let header: u64 = reply.headers["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(header as f64, wait.ceil().max(1.0), "{codec:?}");
}

#[tokio::test]
async fn rate_limit_rejections_carry_matching_retry_information_for_both_codecs() {
    // 0.001 requests/s with burst 1 keeps the exact documented wait of about 1000 seconds.
    let fx = fixture(0.001).await;
    for codec in [Codec::Json, Codec::Binary] {
        assert_rate_limited(&fx, codec).await;
    }
    let tenant = uuid::Uuid::now_v7().to_string();
    fx.list_projects(Codec::Json, &tenant, 101).await;
    let detail = detail_of(&fx.list_projects(Codec::Json, &tenant, 101).await);
    let retry = detail.retry_after.as_option().unwrap();
    assert!((998..=1000).contains(&retry.seconds), "{}", retry.seconds);
}

#[tokio::test]
async fn the_slowest_accepted_rate_keeps_retry_after_within_the_protobuf_duration_range() {
    // The smallest rate Config::load accepts: its retry delay is at the protobuf maximum.
    let fx = fixture(1.0 / MAX_RETRY_SECONDS as f64).await;
    for codec in [Codec::Json, Codec::Binary] {
        assert_rate_limited(&fx, codec).await;
    }
}
