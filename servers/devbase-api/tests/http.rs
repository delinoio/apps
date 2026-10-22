// SPDX-License-Identifier: FSL-1.1-ALv2
//! Request accounting through the real router. These tests need no database: the pool is
//! lazy, and every scenario ends before a query runs.
use axum::{Json, Router, http::StatusCode, routing::post};
use delino_protocol::devbase::v1::*;
use devbase_api::{Op, Server, config::Config};
use serde_json::{Value, json};
use server_common::authority::Authority;
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

const ORIGIN: &str = "https://console.example";
const TIMEOUT: Duration = Duration::from_millis(300);
const ERROR_CODE: &str = "deadline_exceeded";

struct Harness {
    server: Arc<Server>,
    addr: std::net::SocketAddr,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.tasks.iter().for_each(JoinHandle::abort);
    }
}

/// Starts DevBase over a lazy pool. `identify` answers the mocked Delcore `GetCurrentUser`.
async fn harness(identify: StatusCode, hold: bool) -> Harness {
    let mock = Router::new()
        .route(
            "/token",
            post(|| async { Json(json!({"access_token":"service","expires_in":3600})) }),
        )
        .route(
            "/delcore.v1.DelcoreService/GetCurrentUser",
            post(move || async move {
                if hold {
                    std::future::pending::<()>().await;
                }
                (identify, Json(json!({"code":"unavailable"})))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mut tasks = vec![tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap()
    })];
    let db = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://postgres@127.0.0.1:1/unused")
        .unwrap();
    let authority = Authority::new(
        reqwest::Client::new(),
        base.clone(),
        format!("{base}/token"),
        "resource".into(),
        "client".into(),
        "fixture".into(),
    );
    let config = Config {
        projects: 100,
        users: 100,
        keys: 20,
        user_rate: 10000.0,
        user_burst: 20000,
        management_rate: 10000.0,
        management_burst: 20000,
        dependency_timeout: TIMEOUT,
        request_timeout: TIMEOUT,
        cleanup_interval: Duration::from_secs(300),
        origin: ORIGIN.into(),
        listen: "127.0.0.1:0".into(),
    };
    let server = Arc::new(Server::new(db, authority, config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = devbase_api::http::router(server.clone());
    tasks.push(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap()
    }));
    Harness {
        server,
        addr,
        tasks,
    }
}

fn path(op: Op) -> String {
    format!("/devbase.v1.DevBaseService/{}", op.name())
}

/// A tenant-scoped request body, so authentication reaches Delcore without a database query.
fn tenant_body() -> String {
    json!({"tenantId": uuid::Uuid::now_v7().to_string()}).to_string()
}

/// Sends one Connect JSON request in two parts and returns the raw response. The pause
/// between the parts keeps the body incomplete for `pause`.
async fn raw_post(h: &Harness, path: &str, body: &str, pause: Option<Duration>) -> String {
    let mut socket = TcpStream::connect(h.addr).await.unwrap();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nOrigin: {ORIGIN}\r\nAuthorization: Bearer manager\r\nContent-Type: application/json\r\nConnect-Protocol-Version: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(head.as_bytes()).await.unwrap();
    if let Some(pause) = pause {
        let (first, rest) = body.split_at(1);
        socket.write_all(first.as_bytes()).await.unwrap();
        tokio::time::sleep(pause).await;
        socket.write_all(rest.as_bytes()).await.unwrap();
    } else {
        socket.write_all(body.as_bytes()).await.unwrap();
    }
    let mut raw = String::new();
    tokio::time::timeout(Duration::from_secs(5), socket.read_to_string(&mut raw))
        .await
        .unwrap()
        .unwrap();
    raw
}

/// Sends headers that promise a body, then never sends it.
async fn stalled_post(h: &Harness, path: &str) -> String {
    let mut socket = TcpStream::connect(h.addr).await.unwrap();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nOrigin: {ORIGIN}\r\nContent-Type: application/json\r\nContent-Length: 1\r\nConnection: close\r\n\r\n"
    );
    socket.write_all(head.as_bytes()).await.unwrap();
    let mut raw = String::new();
    tokio::time::timeout(Duration::from_secs(5), socket.read_to_string(&mut raw))
        .await
        .unwrap()
        .unwrap();
    raw
}

fn requests(h: &Harness, procedure: &str, code: &str) -> u64 {
    h.server
        .metrics
        .requests
        .with_label_values(&[procedure, code])
        .get()
}

fn samples(h: &Harness, procedure: &str) -> (u64, f64) {
    let histogram = h.server.metrics.duration.with_label_values(&[procedure]);
    (histogram.get_sample_count(), histogram.get_sample_sum())
}

fn series(h: &Harness) -> usize {
    h.server
        .metrics
        .encode()
        .lines()
        .filter(|line| line.starts_with("devbase_requests_total{"))
        .count()
}

fn assert_gateway_timeout(raw: &str) {
    let (headers, body) = raw.split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with("HTTP/1.1 504"), "{headers}");
    let trace = headers
        .lines()
        .find_map(|line| line.strip_prefix("x-devbase-trace-id: "))
        .unwrap();
    let body: Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["code"], ERROR_CODE);
    assert!(uuid::Uuid::parse_str(trace).is_ok());
}

#[test]
fn procedure_paths_match_the_service_allowlist() {
    let specs = [
        DEV_BASE_SERVICE_CREATE_PROJECT_SPEC,
        DEV_BASE_SERVICE_GET_PROJECT_SPEC,
        DEV_BASE_SERVICE_LIST_PROJECTS_SPEC,
        DEV_BASE_SERVICE_UPDATE_PROJECT_SPEC,
        DEV_BASE_SERVICE_DELETE_PROJECT_SPEC,
        DEV_BASE_SERVICE_ISSUE_API_KEY_SPEC,
        DEV_BASE_SERVICE_GET_API_KEY_SPEC,
        DEV_BASE_SERVICE_LIST_API_KEYS_SPEC,
        DEV_BASE_SERVICE_REVOKE_API_KEY_SPEC,
        DEV_BASE_SERVICE_CREATE_END_USER_SPEC,
        DEV_BASE_SERVICE_GET_END_USER_SPEC,
        DEV_BASE_SERVICE_LOOKUP_END_USER_SPEC,
        DEV_BASE_SERVICE_LIST_END_USERS_SPEC,
        DEV_BASE_SERVICE_UPDATE_END_USER_SPEC,
        DEV_BASE_SERVICE_DELETE_END_USER_SPEC,
    ];
    assert_eq!(specs.len(), Op::ALL.len());
    for spec in specs {
        let op = Op::from_procedure_path(spec.procedure).unwrap();
        assert_eq!(spec.procedure, path(op));
    }
}

#[test]
fn near_matches_and_other_paths_have_no_procedure() {
    for path in [
        "",
        "/",
        "/healthz",
        "/metrics",
        "/devbase.v1.DevBaseService",
        "/devbase.v1.DevBaseService/",
        "/devbase.v1.DevBaseService/getproject",
        "/devbase.v1.DevBaseService/GetProject/",
        "/devbase.v1.DevBaseService/GetProject/extra",
        "/devbase.v1.DevBaseService/Unknown",
        "/delcore.v1.DelcoreService/GetProject",
        "devbase.v1.DevBaseService/GetProject",
    ] {
        assert!(Op::from_procedure_path(path).is_none(), "{path}");
    }
}

#[tokio::test]
async fn timeout_during_body_read_counts_once() {
    let h = harness(StatusCode::OK, false).await;
    let raw = stalled_post(&h, &path(Op::GetProject)).await;
    assert_gateway_timeout(&raw);
    assert_eq!(requests(&h, "GetProject", ERROR_CODE), 1);
    let (count, sum) = samples(&h, "GetProject");
    assert_eq!(count, 1);
    assert!(sum >= TIMEOUT.as_secs_f64() * 0.9, "{sum}");
    assert_eq!(series(&h), 1);
    tokio::time::sleep(TIMEOUT * 2).await;
    assert_eq!(requests(&h, "GetProject", ERROR_CODE), 1);
    assert_eq!(samples(&h, "GetProject").0, 1);
    assert_eq!(series(&h), 1);
}

#[tokio::test]
async fn timeout_during_handler_counts_once() {
    let h = harness(StatusCode::OK, true).await;
    // The handler starts after the pause, so the outer deadline expires first with a margin.
    let pause = TIMEOUT / 3;
    let raw = raw_post(&h, &path(Op::ListProjects), &tenant_body(), Some(pause)).await;
    assert_gateway_timeout(&raw);
    assert_eq!(requests(&h, "ListProjects", ERROR_CODE), 1);
    let (count, sum) = samples(&h, "ListProjects");
    assert_eq!(count, 1);
    assert!(sum >= TIMEOUT.as_secs_f64() * 0.9, "{sum}");
    // The cancelled handler must not add a delayed duplicate or an unavailable result.
    tokio::time::sleep(TIMEOUT * 2).await;
    assert_eq!(series(&h), 1);
    assert_eq!(requests(&h, "ListProjects", ERROR_CODE), 1);
    assert_eq!(samples(&h, "ListProjects").0, 1);
}

#[tokio::test]
async fn handler_errors_keep_one_observation() {
    let h = harness(StatusCode::SERVICE_UNAVAILABLE, false).await;
    let raw = raw_post(&h, &path(Op::ListProjects), &tenant_body(), None).await;
    assert!(raw.starts_with("HTTP/1.1 503"), "{raw}");
    assert_eq!(series(&h), 1);
    assert_eq!(requests(&h, "ListProjects", "unavailable"), 1);
    assert_eq!(samples(&h, "ListProjects").0, 1);
}

#[tokio::test]
async fn unrelated_requests_create_no_procedure_series() {
    let h = harness(StatusCode::OK, false).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", h.addr);
    for route in ["/healthz", "/metrics"] {
        let response = client.get(format!("{base}{route}")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let preflight = client
        .request(
            reqwest::Method::OPTIONS,
            format!("{base}{}", path(Op::GetProject)),
        )
        .header("Origin", ORIGIN)
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::NO_CONTENT);

    let rejected = client
        .post(format!("{base}{}", path(Op::GetProject)))
        .header("Origin", "https://other.example")
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    let unknown = stalled_post(&h, "/devbase.v1.DevBaseService/Unknown").await;
    assert!(!unknown.starts_with("HTTP/1.1 200"), "{unknown}");
    let other = stalled_post(&h, "/other/user-supplied-identifier").await;
    assert!(!other.starts_with("HTTP/1.1 200"), "{other}");
    assert_eq!(series(&h), 0);
    assert_eq!(h.server.metrics.encode().matches("Unknown").count(), 0);
    assert_eq!(
        h.server.metrics.encode().matches("user-supplied").count(),
        0
    );
}
