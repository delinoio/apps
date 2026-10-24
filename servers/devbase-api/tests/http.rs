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

    // At or below the limit a malformed JSON body fails in the dispatcher's decoder, before the
    // handler. The response adapter gives that failure the typed error shape; like any
    // transport-level rejection it creates no observation.
    let small = br#"{"projectId": [[["#;
    let reply = exchange(
        fx.addr,
        &typed_head(&path, "application/json", small.len()),
        &[(Duration::ZERO, small)],
    )
    .await;
    assert!(reply.head.starts_with("HTTP/1.1 400"), "{}", reply.head);
    assert!(reply.header("x-devbase-trace-id").is_some());
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

const SAFE_DECODE_MESSAGE: &str = "request body could not be decoded";

/// Log sink shared with a scoped subscriber so a test can search everything the server logged.
#[derive(Clone, Default)]
struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn update_end_user_json(update_mask: &str) -> String {
    format!(
        r#"{{"idempotencyKey":"mask-privacy-fixture","projectId":"{PROJECT}","endUserId":"01900000-0000-7000-8000-000000000003","version":"1","updateMask":"{update_mask}"}}"#
    )
}

/// Decodes the typed error of a reply and returns `(code, message, trace, reason)`.
fn typed_error_of(reply: &Reply) -> (String, String, String, i32) {
    let trace = reply
        .header("x-devbase-trace-id")
        .expect("trace header")
        .to_owned();
    let parsed = uuid::Uuid::parse_str(&trace).unwrap();
    assert_eq!(parsed.get_version_num(), 7, "trace must be a UUID v7");
    assert_eq!(reply.header("content-type"), Some("application/json"));
    let object = &reply.body[reply.body.find('{').unwrap()..=reply.body.rfind('}').unwrap()];
    let json: serde_json::Value = serde_json::from_str(object).unwrap();
    let raw = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(json["details"][0]["value"].as_str().unwrap())
        .unwrap();
    let detail = delino_protocol::devbase::v1::ErrorDetail::decode_from_slice(&raw).unwrap();
    assert_eq!(detail.trace_id, trace, "detail must match the header");
    (
        json["code"].as_str().unwrap().to_owned(),
        json["message"].as_str().unwrap().to_owned(),
        trace,
        detail.reason.to_i32(),
    )
}

#[tokio::test]
async fn invalid_update_mask_components_get_a_fixed_typed_error_without_echo() {
    let logs = LogBuffer::default();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::TRACE)
            .finish(),
    );
    let fx = fixture(Duration::from_secs(5), Delcore::Fail).await;
    let path = format!("{PREFIX}UpdateEndUser");
    // Invalid-character branches (email-shaped and not), the underscore branch and a comma list
    // whose later component is invalid.
    let masks = [
        "alice@example.test",
        "private-customer-name@example.test",
        "private_customer_name",
        "name,private_customer_name",
        // Components with a quote or a backslash: the transport's JSON error body escapes them,
        // so the inspection must accept an escaped message.
        r#"private\"customer"#,
        r#"private\\customer"#,
    ];
    for mask in masks {
        let body = update_end_user_json(mask);
        // Without Origin and without credentials: decoding fails before authentication.
        let reply = exchange(
            fx.addr,
            &rpc_head(&path, body.len(), ""),
            &[(Duration::ZERO, body.as_bytes())],
        )
        .await;
        assert!(
            reply.head.starts_with("HTTP/1.1 400"),
            "{mask}: {}",
            reply.head
        );
        let (code, message, _, reason) = typed_error_of(&reply);
        assert_eq!(code, "invalid_argument", "{mask}");
        assert_eq!(message, SAFE_DECODE_MESSAGE, "{mask}");
        assert_eq!(reason, 1, "ERROR_REASON_INVALID_INPUT");
        for sentinel in [
            "alice",
            "example.test",
            "private",
            "customer",
            "lowerCamelCase",
            "FieldMask",
            "line ",
        ] {
            assert!(
                !reply.head.contains(sentinel) && !reply.body.contains(sentinel),
                "{mask}: {sentinel}"
            );
        }

        // With the configured Origin and an untrusted incoming trace header.
        let extra = format!("Origin: {ORIGIN}\r\nX-Devbase-Trace-Id: untrusted-client-value\r\n");
        let reply = exchange(
            fx.addr,
            &rpc_head(&path, body.len(), &extra),
            &[(Duration::ZERO, body.as_bytes())],
        )
        .await;
        assert!(
            reply.head.starts_with("HTTP/1.1 400"),
            "{mask}: {}",
            reply.head
        );
        let (code, message, trace, _) = typed_error_of(&reply);
        assert_eq!(code, "invalid_argument");
        assert_eq!(message, SAFE_DECODE_MESSAGE);
        assert_ne!(trace, "untrusted-client-value");
        assert_eq!(reply.header("access-control-allow-origin"), Some(ORIGIN));
        assert_eq!(reply.header("vary"), Some("Origin"));
        assert!(
            reply
                .header("access-control-expose-headers")
                .is_some_and(|v| v.contains("X-Devbase-Trace-Id"))
        );
    }
    // The adaptation logs a bounded category with the trace and never the replaced diagnostic.
    let logged = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(logged.contains("request_decode"), "{logged}");
    for sentinel in [
        "alice",
        "example.test",
        "private",
        "customer",
        "lowerCamelCase",
        "FieldMask",
    ] {
        assert!(
            !logged.contains(sentinel),
            "log leaked {sentinel}: {logged}"
        );
    }
    // The handler never ran, so Delcore, authentication and storage were not reached and no
    // procedure outcome was recorded.
    assert_eq!(count(&fx.server, "UpdateEndUser", "invalid_argument"), 0);
    assert_eq!(count(&fx.server, "UpdateEndUser", "unauthenticated"), 0);
    assert_eq!(count(&fx.server, "UpdateEndUser", "unavailable"), 0);
}

#[tokio::test]
async fn valid_update_masks_and_binary_requests_still_reach_the_handler() {
    let fx = fixture(Duration::from_secs(5), Delcore::Fail).await;
    let path = format!("{PREFIX}UpdateEndUser");
    // Decodable masks, including a well-formed but unsupported path, pass the decoder; the
    // handler then answers the missing credentials with its own typed authentication error.
    for mask in ["name", "name,email", "unsupportedField"] {
        let body = update_end_user_json(mask);
        let reply = exchange(
            fx.addr,
            &rpc_head(&path, body.len(), ""),
            &[(Duration::ZERO, body.as_bytes())],
        )
        .await;
        assert!(
            reply.head.starts_with("HTTP/1.1 401"),
            "{mask}: {}",
            reply.head
        );
        let (code, message, _, _) = typed_error_of(&reply);
        assert_eq!(code, "unauthenticated", "{mask}");
        assert_ne!(message, SAFE_DECODE_MESSAGE);
    }
    // Binary requests do not use the JSON parser: a mask that the JSON parser would refuse is
    // an ordinary value here and reaches the handler.
    let request = delino_protocol::devbase::v1::UpdateEndUserRequest {
        project_id: PROJECT.into(),
        update_mask: Some(buffa_types::google::protobuf::FieldMask {
            paths: vec!["private_customer_name".into()],
            ..Default::default()
        })
        .into(),
        ..Default::default()
    };
    let body = request.encode_to_vec();
    let reply = exchange(
        fx.addr,
        &typed_head(&path, "application/proto", body.len())
            .replace("Authorization: Bearer login-token\r\n", ""),
        &[(Duration::ZERO, body.as_slice())],
    )
    .await;
    assert!(reply.head.starts_with("HTTP/1.1 401"), "{}", reply.head);
    assert_eq!(typed_error_of(&reply).0, "unauthenticated");
    assert_eq!(count(&fx.server, "UpdateEndUser", "unauthenticated"), 4);
}

#[tokio::test]
async fn decoding_adaptation_leaves_other_responses_and_unknown_paths_unchanged() {
    let fx = fixture(Duration::from_secs(5), Delcore::Fail).await;
    // A message above 256 KiB keeps the plain transport rejection, including with an invalid
    // mask at its start.
    let mut large = update_end_user_json("alice@example.test").into_bytes();
    large.resize((256 << 10) + 1, b' ');
    let path = format!("{PREFIX}UpdateEndUser");
    let reply = exchange(
        fx.addr,
        &rpc_head(&path, large.len(), ""),
        &[(Duration::ZERO, large.as_slice())],
    )
    .await;
    assert!(reply.head.starts_with("HTTP/1.1 429"), "{}", reply.head);
    assert!(reply.header("x-devbase-trace-id").is_none());
    assert!(reply.body.contains("resource_exhausted"));
    // An unknown procedure is not a decoding failure: it receives the fixed unmatched-procedure
    // reply (separately covered below), never the decoding message.
    let unknown = format!("{PREFIX}NoSuchProcedure");
    let body = update_end_user_json("alice@example.test");
    let reply = exchange(
        fx.addr,
        &rpc_head(&unknown, body.len(), ""),
        &[(Duration::ZERO, body.as_bytes())],
    )
    .await;
    assert!(reply.head.starts_with("HTTP/1.1 404"), "{}", reply.head);
    assert_eq!(typed_error_of(&reply).1, SAFE_UNKNOWN_MESSAGE);
}

const SAFE_UNKNOWN_MESSAGE: &str = "procedure not found";

#[tokio::test]
async fn unmatched_procedures_get_a_fixed_typed_not_found_without_echo() {
    let logs = LogBuffer::default();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::TRACE)
            .finish(),
    );
    let fx = fixture(Duration::from_secs(5), Delcore::Fail).await;
    // The exact synthetic request, other personal-looking selectors, an unrelated path, a
    // percent-encoded selector and a query sentinel.
    let paths = [
        format!("{PREFIX}alice@example.test"),
        format!("{PREFIX}bob@example.test"),
        format!("{PREFIX}alice%40example.test"),
        format!("{PREFIX}GetProject?who=carol@example.test"),
        format!("{PREFIX}NoSuchProcedure?who=carol@example.test"),
        "/private-customer-name/anything".to_owned(),
        "/devbase.v1.OtherService/GetProject".to_owned(),
    ];
    for path in &paths {
        for extra in [
            String::new(),
            format!("Origin: {ORIGIN}\r\nX-Devbase-Trace-Id: untrusted-client-value\r\n"),
        ] {
            let reply = exchange(
                fx.addr,
                &rpc_head(path, 2, &extra),
                &[(Duration::ZERO, b"{}")],
            )
            .await;
            // A known procedure with a query string is not an exact path match either, so it
            // only reaches the handler when the path itself is exact.
            if path.starts_with(&format!("{PREFIX}GetProject?")) {
                assert!(
                    reply.head.starts_with("HTTP/1.1 401"),
                    "{path}: {}",
                    reply.head
                );
                continue;
            }
            assert!(
                reply.head.starts_with("HTTP/1.1 404"),
                "{path}: {}",
                reply.head
            );
            let (code, message, trace, reason) = typed_error_of(&reply);
            assert_eq!(code, "unimplemented", "{path}");
            assert_eq!(message, SAFE_UNKNOWN_MESSAGE, "{path}");
            assert_eq!(reason, 1, "ERROR_REASON_INVALID_INPUT");
            assert_ne!(trace, "untrusted-client-value");
            for sentinel in [
                "alice",
                "bob",
                "example.test",
                "private",
                "customer",
                "carol",
                "NoSuchProcedure",
                "OtherService",
                "method not found",
                "%40",
            ] {
                assert!(
                    !reply.head.contains(sentinel) && !reply.body.contains(sentinel),
                    "{path}: {sentinel}"
                );
            }
            if extra.is_empty() {
                assert!(reply.header("access-control-allow-origin").is_none());
            } else {
                assert_eq!(reply.header("access-control-allow-origin"), Some(ORIGIN));
                assert!(
                    reply
                        .header("access-control-expose-headers")
                        .is_some_and(|v| v.contains("X-Devbase-Trace-Id"))
                );
            }
        }
    }
    // Each reply carries its own fresh trace.
    let first = exchange(
        fx.addr,
        &rpc_head(&paths[0], 2, ""),
        &[(Duration::ZERO, b"{}")],
    )
    .await;
    let second = exchange(
        fx.addr,
        &rpc_head(&paths[0], 2, ""),
        &[(Duration::ZERO, b"{}")],
    )
    .await;
    assert_ne!(typed_error_of(&first).2, typed_error_of(&second).2);

    // A GET on an unknown path takes the same transport rejection and the same replacement.
    let get = "GET /devbase.v1.DevBaseService/alice@example.test HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let reply = exchange(fx.addr, get, &[]).await;
    assert!(reply.head.starts_with("HTTP/1.1 404"), "{}", reply.head);
    assert_eq!(typed_error_of(&reply).1, SAFE_UNKNOWN_MESSAGE);

    // The adaptation logs a bounded category with the trace and never the selector.
    let logged = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(logged.contains("unknown_procedure"), "{logged}");

    // Known procedures keep their own outcomes: a typed authentication error with accounting,
    // an unsupported method on a known procedure and a rejected Origin.
    let known = format!("{PREFIX}ListProjects");
    let reply = exchange(
        fx.addr,
        &rpc_head(&known, 2, ""),
        &[(Duration::ZERO, b"{}")],
    )
    .await;
    assert!(reply.head.starts_with("HTTP/1.1 401"), "{}", reply.head);
    assert_eq!(typed_error_of(&reply).0, "unauthenticated");
    assert_eq!(count(&fx.server, "ListProjects", "unauthenticated"), 1);
    let rejected = rpc_head(&paths[0], 2, "Origin: https://evil.example\r\n");
    let reply = exchange(fx.addr, &rejected, &[(Duration::ZERO, b"{}")]).await;
    assert!(reply.head.starts_with("HTTP/1.1 403"), "{}", reply.head);

    // Unknown paths create no procedure series.
    let encoded = fx.server.metrics.encode();
    assert!(
        !encoded.contains("procedure=\"alice") && !encoded.contains("example.test"),
        "{encoded}"
    );
}

#[tokio::test]
async fn unmatched_procedure_keeps_earlier_deadline_and_body_rejections() {
    let fx = fixture(Duration::from_millis(300), Delcore::Fail).await;
    let path = format!("{PREFIX}alice@example.test");
    // A body that stays pending is cut by the whole-request deadline before the transport can
    // report the routing failure, so the existing typed timeout is preserved.
    let reply = exchange(fx.addr, &rpc_head(&path, 10, ""), &[]).await;
    assert!(reply.head.starts_with("HTTP/1.1 504"), "{}", reply.head);
    // The interceptor-level message limit applies only to registered methods, so a body between
    // 256 KiB and the transport's body bound is still drained and then reported as unmatched.
    let mut medium = vec![b' '; (256 << 10) + 1];
    medium[0] = b'{';
    let reply = exchange(
        fx.addr,
        &rpc_head(&path, medium.len(), ""),
        &[(Duration::ZERO, medium.as_slice())],
    )
    .await;
    assert!(reply.head.starts_with("HTTP/1.1 404"), "{}", reply.head);
    assert_eq!(typed_error_of(&reply).1, SAFE_UNKNOWN_MESSAGE);
    // A body above the transport's own bound is rejected while it is read, before routing. That
    // earlier plain rejection is preserved rather than turned into the unmatched reply.
    let mut huge = vec![b' '; (4 << 20) + 1];
    huge[0] = b'{';
    let reply = exchange(
        fx.addr,
        &rpc_head(&path, huge.len(), ""),
        &[(Duration::ZERO, huge.as_slice())],
    )
    .await;
    assert!(reply.head.starts_with("HTTP/1.1 429"), "{}", reply.head);
    assert!(reply.header("x-devbase-trace-id").is_none());
    assert!(reply.body.contains("resource_exhausted"), "{}", reply.body);
    assert!(!reply.body.contains("procedure not found"));
}

/// Fourth-group leading nibbles 0-7 and c-f give version-7 text without the RFC 4122 variant.
fn non_rfc_vectors() -> Vec<String> {
    (0..16u8)
        .filter(|n| !(8..12).contains(n))
        .map(|n| format!("01900000-0000-7000-{n:x}000-000000000001"))
        .collect()
}

/// Sends one JSON RPC with an optional `Authorization` header value.
async fn rpc(fx: &Fixture, method: &str, authorization: Option<&str>, body: &str) -> Reply {
    let extra = authorization
        .map(|value| format!("Authorization: {value}\r\n"))
        .unwrap_or_default();
    exchange(
        fx.addr,
        &rpc_head(&format!("{PREFIX}{method}"), body.len(), &extra),
        &[(Duration::ZERO, body.as_bytes())],
    )
    .await
}

/// Parses the typed error of a reply: status line prefix, code, message, reason and trace match.
fn assert_typed_error(reply: &Reply, status: &str, code: &str, message: &str, reason: i32) {
    assert!(reply.head.starts_with(status), "{}", reply.head);
    let trace = reply.header("x-devbase-trace-id").expect("trace header");
    assert!(uuid::Uuid::parse_str(trace).is_ok());
    let object = &reply.body[reply.body.find('{').unwrap()..=reply.body.rfind('}').unwrap()];
    let json: serde_json::Value = serde_json::from_str(object).unwrap();
    assert_eq!(json["code"], code, "{object}");
    assert_eq!(json["message"], message, "{object}");
    let raw = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(json["details"][0]["value"].as_str().unwrap())
        .unwrap();
    let detail = delino_protocol::devbase::v1::ErrorDetail::decode_from_slice(&raw).unwrap();
    assert_eq!(detail.trace_id, trace);
    assert_eq!(detail.reason.to_i32(), reason);
}

#[tokio::test]
async fn non_rfc_variant_targets_of_reads_and_key_requests_are_rejected_at_once() {
    // The pool is lazy and unreachable: a rejection that reached credential or database work
    // would end as `unavailable` or `unauthenticated` instead of the typed target error.
    let fx = fixture(Duration::from_secs(5), Delcore::Fail).await;
    let valid_key = format!("dbk_{}_{}", uuid::Uuid::now_v7().simple(), "A".repeat(43));
    for vector in non_rfc_vectors() {
        let project = format!(r#"{{"projectId":"{vector}"}}"#);
        let tenant = format!(r#"{{"tenantId":"{vector}"}}"#);
        for (method, body, bearer) in [
            ("GetProject", &project, "Bearer synthetic-token"),
            ("ListApiKeys", &project, "Bearer synthetic-token"),
            ("ListProjects", &tenant, "Bearer synthetic-token"),
            // API-key requests reject the target before the compact key is even parsed.
            ("GetProject", &project, valid_key.as_str()),
        ] {
            let bearer = if bearer.starts_with("dbk_") {
                format!("Bearer {bearer}")
            } else {
                bearer.to_owned()
            };
            let reply = rpc(&fx, method, Some(&bearer), body).await;
            assert_typed_error(
                &reply,
                "HTTP/1.1 400",
                "invalid_argument",
                "target must be a canonical UUID v7",
                1,
            );
        }
        // The earlier structural Bearer check still wins over the target.
        let reply = rpc(&fx, "GetProject", Some("Bearer"), &project).await;
        assert!(reply.head.starts_with("HTTP/1.1 401"), "{}", reply.head);
        let reply = rpc(&fx, "GetProject", None, &project).await;
        assert!(reply.head.starts_with("HTTP/1.1 401"), "{}", reply.head);
    }
}

#[tokio::test]
async fn compact_keys_with_a_non_rfc_variant_id_are_unauthenticated_before_key_lookup() {
    let fx = fixture(Duration::from_secs(5), Delcore::Fail).await;
    let secret = "A".repeat(43);
    let body = format!(r#"{{"projectId":"{PROJECT}"}}"#);
    for vector in non_rfc_vectors() {
        let key = format!("dbk_{}_{secret}", vector.replace('-', ""));
        let reply = rpc(&fx, "GetProject", Some(&format!("Bearer {key}")), &body).await;
        // A lookup would reach the unreachable pool and end as `unavailable`.
        assert_typed_error(
            &reply,
            "HTTP/1.1 401",
            "unauthenticated",
            "authentication failed",
            2,
        );
    }
}
