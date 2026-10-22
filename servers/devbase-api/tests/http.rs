// SPDX-License-Identifier: FSL-1.1-ALv2
//! Request-metric accounting through the real DevBase HTTP router. These tests need neither a
//! database nor credentials: the pool is lazy and never connects, and Delcore is a local mock.
use axum::{Router, http::StatusCode, routing::post};
use base64::Engine;
use buffa::Message;
use devbase_api::{Op, Server, config::Config};
use server_common::authority::Authority;
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PREFIX: &str = "/devbase.v1.DevBaseService/";
const ORIGIN: &str = "https://console.example";
const TENANT: &str = "01900000-0000-7000-8000-000000000001";
const PROJECT: &str = "01900000-0000-7000-8000-000000000002";

struct Fixture {
    server: Arc<Server>,
    addr: std::net::SocketAddr,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

enum Delcore {
    /// Every Delcore request stays pending far beyond the request timeout.
    Hang,
    /// Every Delcore request fails immediately with a server error.
    Fail,
}

async fn fixture(request_timeout: Duration, delcore: Delcore) -> Fixture {
    let mock = match delcore {
        Delcore::Hang => Router::new().fallback(post(|| async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            StatusCode::OK
        })),
        Delcore::Fail => {
            Router::new().fallback(post(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mock_task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let config = Config {
        projects: 100,
        users: 100,
        keys: 20,
        user_rate: 10000.0,
        user_burst: 20000,
        management_rate: 10000.0,
        management_burst: 20000,
        dependency_timeout: Duration::from_secs(5),
        request_timeout,
        cleanup_interval: Duration::from_secs(300),
        origin: ORIGIN.into(),
        listen: "127.0.0.1:0".into(),
    };
    let authority = Authority::new(
        reqwest::Client::new(),
        base.clone(),
        format!("{base}/token"),
        "resource".into(),
        "client".into(),
        "fixture".into(),
    );
    // The lazy pool never connects: every scenario here ends before database access.
    let db = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://postgres@127.0.0.1:1/unused")
        .unwrap();
    let server = Arc::new(Server::new(db, authority, config));
    let router = devbase_api::http::router(server.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Fixture {
        server,
        addr,
        tasks: vec![mock_task, task],
    }
}

struct Reply {
    head: String,
    body: String,
}
impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        let prefix = format!("{name}: ");
        self.head
            .lines()
            .find_map(|line| line.strip_prefix(prefix.as_str()))
    }
}

/// Sends a raw HTTP/1.1 request whose body is delivered in delayed chunks and reads the reply.
async fn exchange(addr: std::net::SocketAddr, head: &str, chunks: &[(Duration, &[u8])]) -> Reply {
    let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
    socket.write_all(head.as_bytes()).await.unwrap();
    for (delay, chunk) in chunks {
        tokio::time::sleep(*delay).await;
        socket.write_all(chunk).await.unwrap();
    }
    let mut raw = String::new();
    tokio::time::timeout(Duration::from_secs(5), socket.read_to_string(&mut raw))
        .await
        .expect("reply within the socket bound")
        .unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    Reply {
        head: lowercase_header_names(head),
        body: body.into(),
    }
}
/// Lower-cases header names only, so lookups are case-insensitive but values stay intact.
fn lowercase_header_names(head: &str) -> String {
    head.lines()
        .map(|line| match line.split_once(": ") {
            Some((name, value)) => format!("{}: {value}", name.to_ascii_lowercase()),
            None => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

fn rpc_head(path: &str, length: usize, extra: &str) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnect-Protocol-Version: 1\r\nContent-Length: {length}\r\nConnection: close\r\n{extra}\r\n"
    )
}

fn count(server: &Server, procedure: &str, code: &str) -> u64 {
    server
        .metrics
        .requests
        .with_label_values(&[procedure, code])
        .get()
}
fn samples(server: &Server, procedure: &str) -> (u64, f64) {
    let histogram = server.metrics.duration.with_label_values(&[procedure]);
    (histogram.get_sample_count(), histogram.get_sample_sum())
}

fn assert_timeout_reply(reply: &Reply) {
    assert!(reply.head.starts_with("HTTP/1.1 504"), "{}", reply.head);
    let trace = reply.header("x-devbase-trace-id").expect("trace header");
    assert!(uuid::Uuid::parse_str(trace).is_ok());
    let body: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
    assert_eq!(body["code"], "deadline_exceeded");
    let raw = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(body["details"][0]["value"].as_str().unwrap())
        .unwrap();
    let detail = delino_protocol::devbase::v1::ErrorDetail::decode_from_slice(&raw).unwrap();
    assert_eq!(detail.trace_id, trace);
}

#[tokio::test]
async fn timeout_while_reading_the_body_is_counted_once() {
    let fx = fixture(Duration::from_millis(100), Delcore::Fail).await;
    let path = format!("{PREFIX}GetProject");
    // One byte is announced and never sent, so body reading stays pending until the deadline.
    let reply = exchange(fx.addr, &rpc_head(&path, 1, ""), &[]).await;
    assert_timeout_reply(&reply);
    assert_eq!(count(&fx.server, "GetProject", "deadline_exceeded"), 1);
    let (n, sum) = samples(&fx.server, "GetProject");
    assert_eq!(n, 1);
    assert!(sum > 0.0, "elapsed time must be positive, got {sum}");
    // A tolerance below the timeout would be flaky; the sample must at least cover the wait.
    assert!(sum >= 0.09, "sample must span the whole wait, got {sum}");
    // Nothing else is recorded, neither now nor after the cancelled work was dropped.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count(&fx.server, "GetProject", "deadline_exceeded"), 1);
    assert_eq!(count(&fx.server, "GetProject", "unavailable"), 0);
    assert_eq!(samples(&fx.server, "GetProject").0, 1);
}

#[tokio::test]
async fn timeout_while_the_handler_is_pending_is_counted_once() {
    // The body arrives after 150 ms, so the handler's own timeout starts about 150 ms after the
    // outer one and the outer deadline fires first with a wide margin.
    let fx = fixture(Duration::from_millis(400), Delcore::Hang).await;
    let path = format!("{PREFIX}ListProjects");
    let body = format!(r#"{{"tenantId":"{TENANT}"}}"#);
    let head = rpc_head(&path, body.len(), "Authorization: Bearer login-token\r\n");
    let reply = exchange(
        fx.addr,
        &head,
        &[(Duration::from_millis(150), body.as_bytes())],
    )
    .await;
    assert_timeout_reply(&reply);
    assert_eq!(count(&fx.server, "ListProjects", "deadline_exceeded"), 1);
    let (n, sum) = samples(&fx.server, "ListProjects");
    assert_eq!(n, 1);
    assert!(sum >= 0.39, "sample must include body reading, got {sum}");
    // Wait beyond the handler's own deadline: the dropped handler must add nothing.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(count(&fx.server, "ListProjects", "deadline_exceeded"), 1);
    assert_eq!(count(&fx.server, "ListProjects", "unavailable"), 0);
    assert_eq!(samples(&fx.server, "ListProjects").0, 1);
}

#[tokio::test]
async fn handler_errors_keep_their_single_observation_without_timeout_counts() {
    let fx = fixture(Duration::from_secs(5), Delcore::Fail).await;
    let path = format!("{PREFIX}ListProjects");
    let body = format!(r#"{{"tenantId":"{TENANT}"}}"#);

    // Missing credentials: a handler-produced authentication error.
    let reply = exchange(
        fx.addr,
        &rpc_head(&path, body.len(), ""),
        &[(Duration::ZERO, body.as_bytes())],
    )
    .await;
    assert!(reply.head.starts_with("HTTP/1.1 401"), "{}", reply.head);
    assert_eq!(count(&fx.server, "ListProjects", "unauthenticated"), 1);
    assert_eq!(samples(&fx.server, "ListProjects").0, 1);

    // A failing Delcore yields a handler-generated unavailable result, not a timeout.
    let reply = exchange(
        fx.addr,
        &rpc_head(&path, body.len(), "Authorization: Bearer login-token\r\n"),
        &[(Duration::ZERO, body.as_bytes())],
    )
    .await;
    assert!(reply.head.starts_with("HTTP/1.1 503"), "{}", reply.head);
    assert_eq!(count(&fx.server, "ListProjects", "unavailable"), 1);
    assert_eq!(samples(&fx.server, "ListProjects").0, 2);
    assert_eq!(count(&fx.server, "ListProjects", "deadline_exceeded"), 0);
}

#[tokio::test]
async fn timeout_after_the_handler_finished_records_the_returned_timeout_once() {
    // The handler answers a request without credentials at once and reports its own outcome.
    // A layer between the outer timeout and the service then keeps the finished response past the
    // outer deadline, which stands for slow serialization or scheduling after the handler. The
    // client receives the 504, so the one recorded observation is the timeout, not the earlier
    // handler outcome, and its latency spans the whole request.
    let fx = fixture(Duration::from_millis(200), Delcore::Fail).await;
    let slow = devbase_api::http::rpc_router(fx.server.clone()).layer(axum::middleware::from_fn(
        |request: axum::extract::Request, next: axum::middleware::Next| async move {
            let response = next.run(request).await;
            tokio::time::sleep(Duration::from_millis(600)).await;
            response
        },
    ));
    let app = Router::new()
        .fallback_service(slow)
        .layer(axum::middleware::from_fn_with_state(
            server_common::http::Policy {
                origin: ORIGIN.into(),
                timeout: Duration::from_millis(200),
                service: server_common::telemetry::Service::DevBase,
                metrics: fx.server.metrics.clone(),
                procedure: Some(|path| Op::from_procedure_path(path).map(Op::name)),
            },
            server_common::http::middleware,
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let path = format!("{PREFIX}ListProjects");
    let body = format!(r#"{{"tenantId":"{TENANT}"}}"#);
    let reply = exchange(
        addr,
        &rpc_head(&path, body.len(), ""),
        &[(Duration::ZERO, body.as_bytes())],
    )
    .await;
    task.abort();
    assert_timeout_reply(&reply);
    assert_eq!(count(&fx.server, "ListProjects", "deadline_exceeded"), 1);
    assert_eq!(count(&fx.server, "ListProjects", "unauthenticated"), 0);
    let (n, sum) = samples(&fx.server, "ListProjects");
    assert_eq!(n, 1);
    assert!(sum >= 0.19, "sample must span the whole request, got {sum}");
}

#[tokio::test]
async fn client_declared_deadline_is_counted_once_as_a_timeout() {
    // The client's Connect-Timeout-Ms is far shorter than the server's whole-request timeout and
    // expires while authentication waits for Delcore. The transport cancels the handler and
    // answers 504 itself, so the handler never reports; the 504 must still be counted once.
    let fx = fixture(Duration::from_secs(5), Delcore::Hang).await;
    let path = format!("{PREFIX}ListProjects");
    let body = format!(r#"{{"tenantId":"{TENANT}"}}"#);
    let head = rpc_head(
        &path,
        body.len(),
        "Authorization: Bearer login-token\r\nConnect-Timeout-Ms: 150\r\n",
    );
    let reply = exchange(fx.addr, &head, &[(Duration::ZERO, body.as_bytes())]).await;
    assert!(reply.head.starts_with("HTTP/1.1 504"), "{}", reply.head);
    assert_eq!(count(&fx.server, "ListProjects", "deadline_exceeded"), 1);
    let (n, sum) = samples(&fx.server, "ListProjects");
    assert_eq!(n, 1);
    assert!(
        sum >= 0.1,
        "sample must span the client deadline, got {sum}"
    );
    // The cancelled handler adds nothing afterwards.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(count(&fx.server, "ListProjects", "deadline_exceeded"), 1);
    assert_eq!(count(&fx.server, "ListProjects", "unavailable"), 0);
    assert_eq!(samples(&fx.server, "ListProjects").0, 1);
}

/// Raw request head for a unary POST with an explicit content type.
fn typed_head(path: &str, content_type: &str, length: usize) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: {content_type}\r\nConnect-Protocol-Version: 1\r\nAuthorization: Bearer login-token\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
    )
}

#[tokio::test]
async fn malformed_oversized_messages_get_the_typed_size_rejection_before_decoding() {
    // Bodies between 64 KiB and the 256 KiB transport cap whose content cannot be decoded. The
    // generated dispatcher would reject them while decoding, with a bare error and no trace
    // header; the first-position interceptor rejects them by size before any decode.
    let fx = fixture(Duration::from_secs(5), Delcore::Fail).await;
    let path = format!("{PREFIX}GetProject");
    let limit = 64 << 10;
    let mut malformed_json = br#"{"projectId": [[[["#.to_vec();
    malformed_json.resize(limit + 1, b'x');
    let malformed_proto = vec![0xff_u8; 256 << 10];
    let cases: [(&str, Vec<u8>); 3] = [
        ("application/json", malformed_json),
        ("application/proto", vec![0xff_u8; limit + 1]),
        ("application/proto", malformed_proto),
    ];
    for (n, (content_type, body)) in cases.iter().enumerate() {
        let reply = exchange(
            fx.addr,
            &typed_head(&path, content_type, body.len()),
            &[(Duration::ZERO, body)],
        )
        .await;
        assert!(
            reply.head.starts_with("HTTP/1.1 400"),
            "{n}: {}",
            reply.head
        );
        let trace = reply.header("x-devbase-trace-id").expect("trace header");
        assert!(uuid::Uuid::parse_str(trace).is_ok());
        // The reply is chunked: keep the JSON object between the chunk framing lines.
        let object = &reply.body[reply.body.find('{').unwrap()..=reply.body.rfind('}').unwrap()];
        let json: serde_json::Value = serde_json::from_str(object).unwrap();
        assert_eq!(json["code"], "invalid_argument");
        let raw = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(json["details"][0]["value"].as_str().unwrap())
            .unwrap();
        let detail = delino_protocol::devbase::v1::ErrorDetail::decode_from_slice(&raw).unwrap();
        assert_eq!(detail.trace_id, trace);
        assert_eq!(detail.reason.to_i32(), 1, "ERROR_REASON_INVALID_INPUT");
    }
    // Each rejection is observed once with its code and latency.
    assert_eq!(count(&fx.server, "GetProject", "invalid_argument"), 3);
    assert_eq!(samples(&fx.server, "GetProject").0, 3);
    assert_eq!(count(&fx.server, "GetProject", "deadline_exceeded"), 0);

    // At or below the limit a malformed body keeps the dispatcher's own decode error: no DevBase
    // trace header, no observation.
    let small = br#"{"projectId": [[["#;
    let reply = exchange(
        fx.addr,
        &typed_head(&path, "application/json", small.len()),
        &[(Duration::ZERO, small)],
    )
    .await;
    assert!(reply.head.starts_with("HTTP/1.1 400"), "{}", reply.head);
    assert!(reply.header("x-devbase-trace-id").is_none());
    assert_eq!(count(&fx.server, "GetProject", "invalid_argument"), 3);
}

#[tokio::test]
async fn oversized_messages_are_rejected_before_authentication_and_storage() {
    // The pool never connects, so any authentication or storage access would surface as
    // `unauthenticated` or `unavailable` instead of the typed size rejection.
    let fx = fixture(Duration::from_secs(5), Delcore::Fail).await;
    let path = format!("{PREFIX}GetProject");
    let prefix = format!(r#"{{"projectId":"{PROJECT}""#);
    // One byte above the 64 KiB application limit and well below the 256 KiB transport limit.
    let padding = " ".repeat((64 << 10) + 1 - prefix.len() - 1);
    let body = format!("{prefix}{padding}}}");
    assert_eq!(body.len(), (64 << 10) + 1);
    let well_formed_key = format!(
        "dbk_{}_{}",
        "0190000000007000800000000000000a",
        "A".repeat(43)
    );
    for authorization in [
        String::new(),
        "Authorization: Bearer login-token\r\n".to_owned(),
        format!("Authorization: Bearer {well_formed_key}\r\n"),
    ] {
        let reply = exchange(
            fx.addr,
            &rpc_head(&path, body.len(), &authorization),
            &[(Duration::ZERO, body.as_bytes())],
        )
        .await;
        assert!(
            reply.head.starts_with("HTTP/1.1 400"),
            "{authorization:?}: {}",
            reply.head
        );
        // The reply may use chunked framing, so take the JSON object out of the raw body.
        let json = &reply.body[reply.body.find('{').unwrap()..=reply.body.rfind('}').unwrap()];
        let body: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(body["code"], "invalid_argument", "{authorization:?}");
        assert_eq!(body["message"], "request exceeds 64 KiB");
    }
    assert_eq!(count(&fx.server, "GetProject", "invalid_argument"), 3);
    assert_eq!(count(&fx.server, "GetProject", "unauthenticated"), 0);
    assert_eq!(count(&fx.server, "GetProject", "unavailable"), 0);
}

#[tokio::test]
async fn unrouted_requests_create_no_procedure_series() {
    let fx = fixture(Duration::from_millis(100), Delcore::Fail).await;
    let unknown = [
        format!("{PREFIX}Unknown"),
        format!("{PREFIX}getproject"),
        format!("{PREFIX}GetProject/extra"),
        format!("{PREFIX}GetProject%2f"),
        "/devbase.v1.OtherService/GetProject".to_owned(),
        format!("/{PROJECT}"),
        "/".to_owned(),
    ];
    for path in &unknown {
        // The unknown path times out in body reading, which must still create no series.
        let reply = exchange(fx.addr, &rpc_head(path, 1, ""), &[]).await;
        assert!(
            reply.head.starts_with("HTTP/1.1 504") || reply.head.starts_with("HTTP/1.1 404"),
            "{path}: {}",
            reply.head
        );
    }
    for path in ["/healthz", "/readyz", "/metrics"] {
        let head = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        exchange(fx.addr, &head, &[]).await;
    }
    // A configured-origin preflight and a rejected origin on a known procedure.
    let known = format!("{PREFIX}GetProject");
    let preflight = format!(
        "OPTIONS {known} HTTP/1.1\r\nHost: localhost\r\nOrigin: {ORIGIN}\r\nAccess-Control-Request-Method: POST\r\nConnection: close\r\n\r\n"
    );
    let reply = exchange(fx.addr, &preflight, &[]).await;
    assert!(reply.head.starts_with("HTTP/1.1 204"), "{}", reply.head);
    let rejected = rpc_head(&known, 1, "Origin: https://evil.example\r\n");
    let reply = exchange(fx.addr, &rejected, &[]).await;
    assert!(reply.head.starts_with("HTTP/1.1 403"), "{}", reply.head);

    let encoded = fx.server.metrics.encode();
    assert!(
        !encoded.contains("devbase_requests_total{"),
        "unexpected request series:\n{encoded}"
    );
    assert!(
        !encoded.contains("devbase_request_duration_seconds_count{"),
        "unexpected duration series:\n{encoded}"
    );
}

/// Asserts the typed client-deadline reply and returns its trace.
fn assert_typed_deadline(reply: &Reply, untrusted: Option<&str>) -> String {
    assert_timeout_reply(reply);
    let trace = reply.header("x-devbase-trace-id").unwrap();
    let parsed = uuid::Uuid::parse_str(trace).unwrap();
    assert_eq!(parsed.get_version_num(), 7, "trace must be a UUID v7");
    if let Some(untrusted) = untrusted {
        assert_ne!(trace, untrusted);
    }
    assert_eq!(reply.header("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
    assert!(!body["message"].as_str().unwrap().is_empty());
    let raw = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(body["details"][0]["value"].as_str().unwrap())
        .unwrap();
    let detail = delino_protocol::devbase::v1::ErrorDetail::decode_from_slice(&raw).unwrap();
    assert_eq!(
        detail.reason,
        delino_protocol::devbase::v1::ErrorReason::ERROR_REASON_DEPENDENCY_UNAVAILABLE
    );
    trace.to_owned()
}

#[tokio::test]
async fn client_deadline_while_the_handler_is_pending_is_typed() {
    // The global timeout is far longer than the client deadline and Delcore never answers, so
    // only the client's Connect-Timeout-Ms can end the request.
    let fx = fixture(Duration::from_secs(10), Delcore::Hang).await;
    let path = format!("{PREFIX}ListProjects");
    let body = format!(r#"{{"tenantId":"{TENANT}"}}"#);
    let extra = format!(
        "Authorization: Bearer manager\r\nConnect-Timeout-Ms: 50\r\nX-Devbase-Trace-Id: untrusted-client-value\r\nOrigin: {ORIGIN}\r\n"
    );
    let started = std::time::Instant::now();
    let reply = exchange(
        fx.addr,
        &rpc_head(&path, body.len(), &extra),
        &[(Duration::ZERO, body.as_bytes())],
    )
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the shorter client deadline must apply"
    );
    let trace = assert_typed_deadline(&reply, Some("untrusted-client-value"));
    assert!(!trace.is_empty());
    // Allowed-origin responses keep exact CORS metadata and expose the populated trace header.
    assert_eq!(reply.header("access-control-allow-origin"), Some(ORIGIN));
    assert_eq!(reply.header("vary"), Some("Origin"));
    assert!(
        reply
            .header("access-control-expose-headers")
            .unwrap()
            .contains("X-Devbase-Trace-Id")
    );
    // The adapted response is the single observation of the cancelled request.
    assert_eq!(count(&fx.server, "ListProjects", "deadline_exceeded"), 1);
    assert_eq!(samples(&fx.server, "ListProjects").0, 1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(count(&fx.server, "ListProjects", "deadline_exceeded"), 1);
    assert_eq!(samples(&fx.server, "ListProjects").0, 1);
}

#[tokio::test]
async fn client_deadline_is_typed_for_protobuf_requests() {
    let fx = fixture(Duration::from_secs(10), Delcore::Hang).await;
    let path = format!("{PREFIX}ListProjects");
    let request = delino_protocol::devbase::v1::ListProjectsRequest {
        tenant_id: TENANT.into(),
        ..Default::default()
    };
    let body = request.encode_to_vec();
    let head = rpc_head(
        &path,
        body.len(),
        "Authorization: Bearer manager\r\nConnect-Timeout-Ms: 50\r\nX-Devbase-Trace-Id: untrusted-client-value\r\n",
    )
    .replace(
        "Content-Type: application/json",
        "Content-Type: application/proto",
    );
    let reply = exchange(fx.addr, &head, &[(Duration::ZERO, body.as_slice())]).await;
    assert_typed_deadline(&reply, Some("untrusted-client-value"));
    assert_eq!(count(&fx.server, "ListProjects", "deadline_exceeded"), 1);
}

#[tokio::test]
async fn client_deadline_covers_a_partially_delivered_body() {
    let fx = fixture(Duration::from_secs(10), Delcore::Hang).await;
    let path = format!("{PREFIX}ListProjects");
    let body = format!(r#"{{"tenantId":"{TENANT}"}}"#);
    let (first, _rest) = body.as_bytes().split_at(5);
    // The announced body never completes, so the deadline must expire during body receipt.
    let head = rpc_head(
        &path,
        body.len(),
        "Authorization: Bearer manager\r\nConnect-Timeout-Ms: 50\r\n",
    );
    let started = std::time::Instant::now();
    let reply = exchange(fx.addr, &head, &[(Duration::ZERO, first)]).await;
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_typed_deadline(&reply, None);
    assert_eq!(count(&fx.server, "ListProjects", "deadline_exceeded"), 1);
}

#[tokio::test]
async fn deadline_adaptation_leaves_other_responses_unchanged() {
    let fx = fixture(Duration::from_secs(10), Delcore::Fail).await;
    let path = format!("{PREFIX}ListProjects");
    let body = format!(r#"{{"tenantId":"{TENANT}"}}"#);

    // A generous client deadline with a failing dependency keeps its handler-typed 503.
    let head = rpc_head(
        &path,
        body.len(),
        "Authorization: Bearer manager\r\nConnect-Timeout-Ms: 5000\r\n",
    );
    let reply = exchange(fx.addr, &head, &[(Duration::ZERO, body.as_bytes())]).await;
    assert!(reply.head.starts_with("HTTP/1.1 503"), "{}", reply.head);
    assert!(reply.header("x-devbase-trace-id").is_some());
    assert_eq!(count(&fx.server, "ListProjects", "deadline_exceeded"), 0);

    // An immediate authentication error keeps its typed 401.
    let reply = exchange(
        fx.addr,
        &rpc_head(&path, body.len(), ""),
        &[(Duration::ZERO, body.as_bytes())],
    )
    .await;
    assert!(reply.head.starts_with("HTTP/1.1 401"), "{}", reply.head);
    assert!(reply.header("x-devbase-trace-id").is_some());

    // A message above 256 KiB keeps the plain transport rejection without detail or trace.
    let large = vec![b'x'; (256 << 10) + 1];
    let head = rpc_head(
        &path,
        large.len(),
        "Authorization: Bearer manager\r\nConnect-Timeout-Ms: 5000\r\n",
    );
    let reply = exchange(fx.addr, &head, &[(Duration::ZERO, large.as_slice())]).await;
    assert!(!reply.head.starts_with("HTTP/1.1 504"), "{}", reply.head);
    assert!(reply.header("x-devbase-trace-id").is_none());
    assert!(reply.head.starts_with("HTTP/1.1 429"), "{}", reply.head);
    // The body may be chunk-framed, so parse only the JSON object it contains.
    let (start, end) = (
        reply.body.find('{').unwrap(),
        reply.body.rfind('}').unwrap(),
    );
    let body: serde_json::Value = serde_json::from_str(&reply.body[start..=end]).unwrap();
    assert_eq!(body["code"], "resource_exhausted");
    assert!(body.get("details").is_none());
}

#[test]
fn procedure_allowlist_matches_operation_names() {
    use Op::*;
    let all = [
        CreateProject,
        GetProject,
        ListProjects,
        UpdateProject,
        DeleteProject,
        IssueApiKey,
        GetApiKey,
        ListApiKeys,
        RevokeApiKey,
        CreateEndUser,
        GetEndUser,
        LookupEndUser,
        ListEndUsers,
        UpdateEndUser,
        DeleteEndUser,
    ];
    // Keeps the list above exhaustive: adding a variant breaks this match.
    for op in all {
        match op {
            CreateProject | GetProject | ListProjects | UpdateProject | DeleteProject
            | IssueApiKey | GetApiKey | ListApiKeys | RevokeApiKey | CreateEndUser | GetEndUser
            | LookupEndUser | ListEndUsers | UpdateEndUser | DeleteEndUser => {}
        }
        let path = format!("{PREFIX}{}", op.name());
        assert_eq!(Op::from_procedure_path(&path), Some(op), "{path}");
    }
    for path in [
        "",
        "/",
        PREFIX,
        "/devbase.v1.DevBaseService/GetProject/",
        "/devbase.v1.DevBaseService/getproject",
        "/devbase.v1.DevBaseService/ GetProject",
        "devbase.v1.DevBaseService/GetProject",
        "/devbase.v1.DevBaseService/GetProject?x=1",
        "/other/devbase.v1.DevBaseService/GetProject",
    ] {
        assert_eq!(Op::from_procedure_path(path), None, "{path:?}");
    }
}
