// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Server, postgres, worker::Worker};
use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tracing::Instrument;

#[derive(Clone)]
struct Policy {
    timeout: Duration,
    limit: i64,
    window: Arc<Mutex<(Instant, i64)>>,
    /// Request paths (with the leading slash) of every procedure registered on the
    /// Delcore RPC router. See `rpc_procedures`.
    procedures: Arc<HashSet<String>>,
}
/// Collects the request path of every procedure the generated Delcore service
/// registered on `router`, for example `/delcore.v1.DelcoreService/ListTenants`.
/// The set comes from the router itself, so it cannot diverge from the generated
/// service definition when RPCs are added or removed.
fn rpc_procedures(router: &connectrpc::Router) -> HashSet<String> {
    router
        .methods()
        .map(|method| format!("/{method}"))
        .collect()
}
/// Decides whether the Nth rejected request of one window writes a diagnostic.
/// Logging only the 1st, 2nd, 4th, 8th, ... rejection keeps the log volume of an
/// exhausted window logarithmic in the request volume, so the limiter does not
/// turn a traffic spike into unbounded synchronous log I/O. Every rejected
/// response still carries its own correlation ID.
fn log_rejection(rejected_in_window: i64) -> bool {
    u64::try_from(rejected_in_window).is_ok_and(u64::is_power_of_two)
}
/// Selects the headers that decide how a rate rejection is rendered.
///
/// A genuine Delcore RPC (a POST to a procedure registered by the generated
/// `delcore.v1.DelcoreService`) keeps its request headers, so the connectrpc
/// renderer answers in the caller's protocol: Connect unary gets the HTTP 429 JSON
/// error, while gRPC, gRPC-Web and Connect-streaming callers get their protocol
/// framing (`grpc-status` trailers or an end-stream envelope) carrying
/// `resource_exhausted`, which their clients decode as a typed error. The complete
/// path must match a registered procedure, so an unknown method under the service
/// prefix is not an RPC. Every other request, including health, readiness,
/// metrics, webhook, lookalike and unknown paths, is rendered without headers so
/// it stays HTTP 429 with the typed Connect JSON body whatever its content type;
/// probes, webhook senders and proxies only see the HTTP status and must not
/// receive a 200 for a rejection.
fn rejection_render_headers(policy: &Policy, request: &Request) -> axum::http::HeaderMap {
    let is_rpc = request.method() == axum::http::Method::POST
        && policy.procedures.contains(request.uri().path());
    if is_rpc {
        request.headers().clone()
    } else {
        axum::http::HeaderMap::new()
    }
}
async fn bounded(State(policy): State<Policy>, mut request: Request, next: Next) -> Response {
    let (allowed, rejected_in_window) = {
        let mut state = policy.window.lock().unwrap_or_else(|e| e.into_inner());
        if state.0.elapsed() >= Duration::from_secs(60) {
            *state = (Instant::now(), 0)
        }
        state.1 = state.1.saturating_add(1);
        (
            state.1 <= policy.limit,
            state.1.saturating_sub(policy.limit.max(0)),
        )
    };
    // Generate the server correlation ID before admission so that every response,
    // including a rate rejection, carries it. Any client-supplied value is replaced.
    let correlation = server_common::id();
    request
        .headers_mut()
        .insert("X-Delcore-Correlation-Id", correlation.parse().unwrap());
    let request_headers = request.headers().clone();
    let render_headers = rejection_render_headers(&policy, &request);
    if !allowed {
        // Log only the correlation ID, category and sampled count; never headers or
        // request data. The log is sampled so that it stays bounded under load.
        if log_rejection(rejected_in_window) {
            tracing::warn!(correlation_id = %correlation, category="rate_limited", rejected_in_window, "request rate limit exceeded");
        }
        // The schema has no rate-limit reason, so use ERROR_REASON_UNSPECIFIED (0).
        // Only genuine Delcore RPCs are rendered in the caller's protocol; all
        // other requests get the HTTP 429 unary JSON body. See
        // `rejection_render_headers`.
        return crate::Fault::new(
            connectrpc::ErrorCode::ResourceExhausted,
            0,
            "request rate limit exceeded",
        )
        .with_correlation(&correlation)
        .into_http_response(&render_headers)
        .map(axum::body::Body::new);
    }
    match tokio::time::timeout(
        policy.timeout,
        next.run(request)
            .instrument(tracing::info_span!("delcore.connect", correlation_id = %correlation)),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!(correlation_id = %correlation, category="deadline_exceeded", "request deadline exceeded");
            crate::Fault::new(
                connectrpc::ErrorCode::DeadlineExceeded,
                6,
                "request deadline exceeded",
            )
            .with_correlation(&correlation)
            .into_http_response(&request_headers)
            .map(axum::body::Body::new)
        }
    }
}
pub fn router(server: Arc<Server>, worker: Arc<Worker>) -> Router {
    let rpc = connectrpc::Router::new().add_service(server.clone());
    let procedures = Arc::new(rpc_procedures(&rpc));
    let rpc = rpc.into_axum_service().with_limits(
        connectrpc::Limits::default().with_max_message_size(server.config.max_payload_bytes),
    );
    let ready = server.db.clone();
    let metrics = server.metrics.clone();
    Router::new()
        .route("/healthz", get(|| async { StatusCode::OK }))
        .route(
            "/readyz",
            get(move || {
                let db = ready.clone();
                async move {
                    match tokio::time::timeout(Duration::from_secs(1), postgres::check_schema(&db))
                        .await
                    {
                        Ok(Ok(())) => StatusCode::OK,
                        _ => StatusCode::SERVICE_UNAVAILABLE,
                    }
                }
            }),
        )
        .route(
            "/metrics",
            get(move || {
                let metrics = metrics.clone();
                async move {
                    (
                        [("content-type", "text/plain; version=0.0.4")],
                        metrics.encode(),
                    )
                }
            }),
        )
        .route("/webhooks/paddle", post(webhook).with_state(worker))
        .fallback_service(rpc)
        .layer(middleware::from_fn_with_state(
            Policy {
                timeout: server.config.timeout(),
                limit: server.config.requests_per_minute,
                window: Arc::new(Mutex::new((Instant::now(), 0))),
                procedures,
            },
            bounded,
        ))
}
async fn webhook(State(worker): State<Arc<Worker>>, request: Request) -> StatusCode {
    let signature = request
        .headers()
        .get("Paddle-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let Ok(raw) = to_bytes(request.into_body(), worker.config.max_payload_bytes).await else {
        return StatusCode::PAYLOAD_TOO_LARGE;
    };
    worker.ingest_webhook(&signature, &raw).await
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    /// Serializes the tests that emit warnings. The sampling test counts warnings
    /// through a thread-local subscriber; a callsite first hit by a concurrent test
    /// thread without a subscriber can cache a "never" interest and hide the
    /// warnings from it. Remove this lock if the tests stop sharing callsites.
    static WARNING_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    #[tokio::test]
    async fn rate_bounds_and_slow_bodies_are_enforced() {
        let _serial = WARNING_TESTS.lock().await;
        let app = Router::new()
            .route("/", post(|_: Bytes| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                Policy {
                    timeout: Duration::from_millis(30),
                    limit: 1,
                    window: Arc::new(Mutex::new((Instant::now(), 0))),
                    procedures: Arc::default(),
                },
                bounded,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        for status in [504, 429] {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            stream.write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\nConnection: close\r\n\r\n").await.unwrap();
            let mut raw = String::new();
            tokio::time::timeout(Duration::from_secs(2), stream.read_to_string(&mut raw))
                .await
                .unwrap()
                .unwrap();
            assert!(raw.starts_with(&format!("HTTP/1.1 {status}")), "{raw}");
            assert!(raw.contains("x-delcore-correlation-id:"), "{raw}");
            assert!(raw.contains("delcore.v1.ErrorDetail"), "{raw}");
            if status == 504 {
                assert!(raw.contains("deadline_exceeded"), "{raw}");
            } else {
                assert!(raw.contains("resource_exhausted"), "{raw}");
            }
        }
        task.abort();
    }
    #[tokio::test]
    async fn handler_timeouts_keep_connect_details_and_server_correlation() {
        let _serial = WARNING_TESTS.lock().await;
        use base64::Engine;
        use buffa::Message;
        let app = Router::new()
            .route(
                "/",
                post(|| async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    StatusCode::OK
                }),
            )
            .layer(middleware::from_fn_with_state(
                Policy {
                    timeout: Duration::from_millis(20),
                    limit: 10,
                    window: Arc::new(Mutex::new((Instant::now(), 0))),
                    procedures: Arc::default(),
                },
                bounded,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        for content_type in ["application/json", "application/proto"] {
            let response = reqwest::Client::new()
                .post(&url)
                .header("content-type", content_type)
                .header("X-Delcore-Correlation-Id", "untrusted-client-value")
                .body("{}")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
            let correlation = response.headers()["X-Delcore-Correlation-Id"]
                .to_str()
                .unwrap()
                .to_owned();
            assert!(server_common::canonical_id(&correlation));
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["code"], "deadline_exceeded");
            let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(body["details"][0]["value"].as_str().unwrap())
                .unwrap();
            let detail =
                delino_protocol::delcore::v1::ErrorDetail::decode_from_slice(&bytes).unwrap();
            assert_eq!(detail.correlation_id, correlation);
        }
        task.abort();
    }
    fn decode_detail(body: &serde_json::Value) -> delino_protocol::delcore::v1::ErrorDetail {
        use base64::Engine;
        use buffa::Message;
        assert_eq!(body["details"][0]["type"], "delcore.v1.ErrorDetail");
        let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(body["details"][0]["value"].as_str().unwrap())
            .unwrap();
        delino_protocol::delcore::v1::ErrorDetail::decode_from_slice(&bytes).unwrap()
    }
    #[tokio::test]
    async fn rate_rejections_keep_typed_errors_and_server_correlation() {
        let _serial = WARNING_TESTS.lock().await;
        use std::sync::atomic::{AtomicUsize, Ordering};
        for content_type in ["application/json", "application/proto"] {
            let calls = Arc::new(AtomicUsize::new(0));
            let handler_calls = calls.clone();
            let app = Router::new()
                .route(
                    "/",
                    post(move || {
                        let calls = handler_calls.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            StatusCode::OK
                        }
                    }),
                )
                .layer(middleware::from_fn_with_state(
                    Policy {
                        timeout: Duration::from_secs(30),
                        limit: 1,
                        window: Arc::new(Mutex::new((Instant::now(), 0))),
                        procedures: Arc::default(),
                    },
                    bounded,
                ));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let client = reqwest::Client::new();
            let admitted = client
                .post(&url)
                .header("content-type", content_type)
                .body("{}")
                .send()
                .await
                .unwrap();
            assert_eq!(admitted.status(), StatusCode::OK);
            let response = client
                .post(&url)
                .header("content-type", content_type)
                .header("X-Delcore-Correlation-Id", "untrusted-client-value")
                .body("{}")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(response.headers()["content-type"], "application/json");
            let correlation = response.headers()["X-Delcore-Correlation-Id"]
                .to_str()
                .unwrap()
                .to_owned();
            assert_ne!(correlation, "untrusted-client-value");
            assert!(server_common::canonical_id(&correlation));
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["code"], "resource_exhausted");
            assert!(!body["message"].as_str().unwrap().is_empty());
            let detail = decode_detail(&body);
            assert_eq!(detail.correlation_id, correlation);
            assert_eq!(detail.reason.to_i32(), 0);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "rejected call reached handler"
            );
            task.abort();
        }
    }
    #[tokio::test]
    async fn rate_rejections_stay_429_for_every_route_and_content_type() {
        let _serial = WARNING_TESTS.lock().await;
        let app = Router::new()
            .route("/healthz", get(|| async { StatusCode::OK }))
            .route("/webhooks/paddle", post(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                Policy {
                    timeout: Duration::from_secs(30),
                    limit: 1,
                    window: Arc::new(Mutex::new((Instant::now(), 0))),
                    procedures: Arc::default(),
                },
                bounded,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        // Consume the only admitted request of the window.
        assert_eq!(
            client
                .get(format!("{base}/healthz"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        for content_type in [
            None,
            Some("application/json"),
            Some("application/proto"),
            Some("application/connect+proto"),
            Some("application/connect+json"),
            Some("application/grpc"),
            Some("application/grpc+proto"),
            Some("application/grpc-web+proto"),
            Some("text/plain"),
            Some("not a media type"),
        ] {
            for (method, path) in [
                (reqwest::Method::GET, "/healthz"),
                (reqwest::Method::POST, "/webhooks/paddle"),
            ] {
                let mut request = client
                    .request(method, format!("{base}{path}"))
                    .header("X-Delcore-Correlation-Id", "untrusted-client-value");
                if let Some(content_type) = content_type {
                    request = request.header("content-type", content_type);
                }
                let response = request.send().await.unwrap();
                let label = format!("{path} {content_type:?}");
                assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS, "{label}");
                let correlation = response.headers()["X-Delcore-Correlation-Id"]
                    .to_str()
                    .unwrap()
                    .to_owned();
                assert_ne!(correlation, "untrusted-client-value", "{label}");
                assert!(server_common::canonical_id(&correlation), "{label}");
                let body: serde_json::Value = response.json().await.unwrap();
                assert_eq!(body["code"], "resource_exhausted", "{label}");
                assert_eq!(decode_detail(&body).correlation_id, correlation, "{label}");
            }
        }
        task.abort();
    }
    const RPC_PATH: &str = "/delcore.v1.DelcoreService/GetCurrentUser";
    /// A method name under the service prefix that the generated service does not register.
    const UNKNOWN_METHOD_PATH: &str = "/delcore.v1.DelcoreService/NoSuchMethod";
    /// Starts a limit-1 app with an RPC-shaped route and non-RPC routes, and
    /// consumes the only admitted request of the window.
    async fn exhausted_app() -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route(RPC_PATH, post(|| async { StatusCode::OK }))
            .route(RPC_PATH, get(|| async { StatusCode::OK }))
            .route(
                "/delcore.v1.DelcoreServiceX/Other",
                post(|| async { StatusCode::OK }),
            )
            .route("/readyz", get(|| async { StatusCode::OK }))
            .route("/metrics", get(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                Policy {
                    timeout: Duration::from_secs(30),
                    limit: 1,
                    window: Arc::new(Mutex::new((Instant::now(), 0))),
                    procedures: Arc::new(HashSet::from([RPC_PATH.to_owned()])),
                },
                bounded,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(
            reqwest::get(format!("{base}/readyz"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        (base, task)
    }
    fn correlation_of(response: &reqwest::Response, label: &str) -> String {
        let correlation = response.headers()["X-Delcore-Correlation-Id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_ne!(correlation, "untrusted-client-value", "{label}");
        assert!(server_common::canonical_id(&correlation), "{label}");
        correlation
    }
    #[tokio::test]
    async fn rpc_rate_rejections_use_the_callers_protocol_framing() {
        let _serial = WARNING_TESTS.lock().await;
        let (base, task) = exhausted_app().await;
        let client = reqwest::Client::new();
        let post_rpc = |content_type: Option<&'static str>| {
            let mut request = client
                .post(format!("{base}{RPC_PATH}"))
                .header("X-Delcore-Correlation-Id", "untrusted-client-value")
                .body("{}");
            if let Some(content_type) = content_type {
                request = request.header("content-type", content_type);
            }
            request.send()
        };
        // Connect unary, with or without a content type: HTTP 429 and a JSON body.
        for content_type in [None, Some("application/json"), Some("application/proto")] {
            let label = format!("connect unary {content_type:?}");
            let response = post_rpc(content_type).await.unwrap();
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS, "{label}");
            assert_eq!(
                response.headers()["content-type"],
                "application/json",
                "{label}"
            );
            let correlation = correlation_of(&response, &label);
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["code"], "resource_exhausted", "{label}");
            assert_eq!(decode_detail(&body).correlation_id, correlation, "{label}");
        }
        // Connect streaming: HTTP 200 with an end-stream envelope carrying the error.
        for content_type in ["application/connect+proto", "application/connect+json"] {
            let response = post_rpc(Some(content_type)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{content_type}");
            assert_eq!(response.headers()["content-type"], content_type);
            let correlation = correlation_of(&response, content_type);
            let body = response.bytes().await.unwrap();
            assert_eq!(body[0], 0x02, "{content_type}: end-stream flag");
            let end: serde_json::Value = serde_json::from_slice(&body[5..]).unwrap();
            assert_eq!(end["error"]["code"], "resource_exhausted", "{content_type}");
            assert_eq!(
                decode_detail(&end["error"]).correlation_id,
                correlation,
                "{content_type}"
            );
        }
        // gRPC: HTTP 200 trailers-only response with grpc-status RESOURCE_EXHAUSTED (8).
        for content_type in ["application/grpc", "application/grpc+proto"] {
            let response = post_rpc(Some(content_type)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{content_type}");
            // The renderer normalizes the bare gRPC type to its proto codec form.
            assert_eq!(response.headers()["content-type"], "application/grpc+proto");
            assert_eq!(response.headers()["grpc-status"], "8", "{content_type}");
            assert!(
                !response.headers()["grpc-message"].is_empty(),
                "{content_type}"
            );
            correlation_of(&response, content_type);
        }
        // gRPC-Web: HTTP 200 with a trailer frame in the body.
        for content_type in ["application/grpc-web", "application/grpc-web+proto"] {
            let response = post_rpc(Some(content_type)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{content_type}");
            assert_eq!(
                response.headers()["content-type"],
                "application/grpc-web+proto"
            );
            correlation_of(&response, content_type);
            let body = response.bytes().await.unwrap();
            assert_eq!(body[0], 0x80, "{content_type}: trailer flag");
            let trailers = std::str::from_utf8(&body[5..]).unwrap();
            assert!(trailers.contains("grpc-status: 8"), "{trailers:?}");
            assert!(trailers.contains("grpc-status-details-bin"), "{trailers:?}");
        }
        task.abort();
    }
    #[tokio::test]
    async fn non_rpc_requests_stay_429_even_with_rpc_content_types_or_lookalike_paths() {
        let _serial = WARNING_TESTS.lock().await;
        let (base, task) = exhausted_app().await;
        let client = reqwest::Client::new();
        let requests = [
            (reqwest::Method::GET, "/readyz"),
            (reqwest::Method::GET, "/metrics"),
            // A GET is not a gRPC or streaming RPC, and a service-name prefix is not the service.
            (reqwest::Method::GET, RPC_PATH),
            (reqwest::Method::POST, "/delcore.v1.DelcoreServiceX/Other"),
            (reqwest::Method::POST, "/other.v1.Service/Method"),
            (reqwest::Method::POST, "/unknown"),
            // An unknown method under the service path is not a registered procedure.
            (reqwest::Method::POST, UNKNOWN_METHOD_PATH),
        ];
        for content_type in [
            "application/grpc",
            "application/grpc+proto",
            "application/grpc-web+proto",
            "application/connect+proto",
            "application/connect+json",
        ] {
            for (method, path) in &requests {
                let label = format!("{method} {path} {content_type}");
                let response = client
                    .request(method.clone(), format!("{base}{path}"))
                    .header("content-type", content_type)
                    .header("X-Delcore-Correlation-Id", "untrusted-client-value")
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS, "{label}");
                assert!(response.headers().get("grpc-status").is_none(), "{label}");
                let correlation = correlation_of(&response, &label);
                let body: serde_json::Value = response.json().await.unwrap();
                assert_eq!(body["code"], "resource_exhausted", "{label}");
                assert_eq!(decode_detail(&body).correlation_id, correlation, "{label}");
            }
        }
        task.abort();
    }
    /// Builds the production router around a real `Server` with a one-request
    /// window, so the registered-procedure set comes from the generated service.
    /// The database pool is lazy and unreachable: the admitted `/healthz` request
    /// and every rejected request never touch it.
    async fn exhausted_production_app() -> (String, tokio::task::JoinHandle<()>) {
        use axum::{Json, routing::get};
        let oidc = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", oidc.local_addr().unwrap());
        let discovery = serde_json::json!({"issuer":issuer,"jwks_uri":format!("{issuer}/jwks")});
        let mock = Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || async { Json(discovery) }),
            )
            .route(
                "/jwks",
                get(|| async { Json(serde_json::json!({"keys":[]})) }),
            );
        tokio::spawn(async move { axum::serve(oidc, mock).await.unwrap() });
        let mut values: std::collections::HashMap<_, _> =
            dotenvy::from_read_iter(include_bytes!("../.env.example").as_slice())
                .map(|v| v.unwrap())
                .collect();
        values.insert("ISSUER".into(), issuer);
        values.insert("REQUESTS_PER_MINUTE".into(), "1".into());
        let config =
            crate::config::Config::from_getter(|key| values.get(key).cloned().unwrap_or_default())
                .unwrap();
        let auth = crate::auth::Verifier::new(&config).await.unwrap();
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@127.0.0.1:1/test")
            .unwrap();
        let server = Arc::new(Server::new(db.clone(), auth, config.clone()));
        let worker = Arc::new(Worker::new(db, config).unwrap());
        let app = router(server, worker);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(
            reqwest::get(format!("{base}/healthz"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        (base, task)
    }
    #[tokio::test]
    async fn generated_service_procedures_get_framing_and_unknown_methods_stay_429() {
        use delino_protocol::delcore::v1::{
            DELCORE_SERVICE_GET_CURRENT_USER_SPEC, DELCORE_SERVICE_INGEST_EVENT_SPEC,
        };
        let _serial = WARNING_TESTS.lock().await;
        assert_eq!(DELCORE_SERVICE_GET_CURRENT_USER_SPEC.procedure, RPC_PATH);
        let (base, task) = exhausted_production_app().await;
        let client = reqwest::Client::new();
        let post = |path: &str, content_type: &'static str| {
            client
                .post(format!("{base}{path}"))
                .header("content-type", content_type)
                .body("{}")
                .send()
        };
        // Registered procedures keep their protocol framing.
        for spec in [
            DELCORE_SERVICE_GET_CURRENT_USER_SPEC,
            DELCORE_SERVICE_INGEST_EVENT_SPEC,
        ] {
            let response = post(spec.procedure, "application/grpc").await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{}", spec.procedure);
            assert_eq!(response.headers()["grpc-status"], "8", "{}", spec.procedure);
            let response = post(spec.procedure, "application/connect+json")
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{}", spec.procedure);
            let body = response.bytes().await.unwrap();
            assert_eq!(body[0], 0x02, "{}: end-stream flag", spec.procedure);
            let response = post(spec.procedure, "application/grpc-web+proto")
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{}", spec.procedure);
            let body = response.bytes().await.unwrap();
            assert_eq!(body[0], 0x80, "{}: trailer flag", spec.procedure);
        }
        // Unknown methods, including near misses of registered ones, stay unary HTTP 429.
        for path in [
            UNKNOWN_METHOD_PATH,
            "/delcore.v1.DelcoreService/getcurrentuser",
            "/delcore.v1.DelcoreService/GetCurrentUser/",
            "/delcore.v1.DelcoreService/",
        ] {
            for content_type in [
                "application/grpc",
                "application/grpc-web+proto",
                "application/connect+proto",
                "application/connect+json",
            ] {
                let label = format!("{path} {content_type}");
                let response = post(path, content_type).await.unwrap();
                assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS, "{label}");
                assert!(response.headers().get("grpc-status").is_none(), "{label}");
                assert_eq!(
                    response.headers()["content-type"],
                    "application/json",
                    "{label}"
                );
                let correlation = correlation_of(&response, &label);
                let body: serde_json::Value = response.json().await.unwrap();
                assert_eq!(body["code"], "resource_exhausted", "{label}");
                assert_eq!(decode_detail(&body).correlation_id, correlation, "{label}");
            }
        }
        task.abort();
    }
    #[test]
    fn rejection_logging_is_bounded_per_window() {
        let logged: Vec<i64> = (1..=1_000_000).filter(|n| log_rejection(*n)).collect();
        assert_eq!(logged.len(), 20, "{logged:?}");
        assert_eq!(&logged[..5], &[1, 2, 4, 8, 16]);
        assert!(!log_rejection(0));
        assert!(!log_rejection(-1));
        assert!(!log_rejection(3));
    }
    #[tokio::test]
    async fn rate_rejection_warnings_are_sampled_while_every_response_is_rejected() {
        let _serial = WARNING_TESTS.lock().await;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tracing_subscriber::prelude::*;
        struct CountWarnings(Arc<AtomicUsize>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountWarnings {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _: tracing_subscriber::layer::Context<'_, S>,
            ) {
                if *event.metadata().level() == tracing::Level::WARN {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        let warnings = Arc::new(AtomicUsize::new(0));
        // The default test runtime is single-threaded, so the thread-local default
        // subscriber also covers the connection tasks spawned by `axum::serve`.
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(CountWarnings(warnings.clone())),
        );
        let app = Router::new()
            .route("/", post(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                Policy {
                    timeout: Duration::from_secs(30),
                    limit: 1,
                    window: Arc::new(Mutex::new((Instant::now(), 0))),
                    procedures: Arc::default(),
                },
                bounded,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        assert_eq!(
            client.post(&url).send().await.unwrap().status(),
            StatusCode::OK
        );
        let mut correlations = std::collections::HashSet::new();
        for _ in 0..9 {
            let response = client.post(&url).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            correlations.insert(
                response.headers()["X-Delcore-Correlation-Id"]
                    .to_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        // Rejections 1, 2, 4 and 8 of the nine log; every response keeps its own ID.
        assert_eq!(warnings.load(Ordering::SeqCst), 4);
        assert_eq!(correlations.len(), 9);
        task.abort();
    }
    #[tokio::test]
    async fn rate_window_resets_after_sixty_seconds() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let stale = Instant::now()
            .checked_sub(Duration::from_secs(61))
            .expect("monotonic clock older than 61 seconds");
        let app = Router::new()
            .route(
                "/",
                post(move || {
                    let calls = handler_calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(
                Policy {
                    timeout: Duration::from_secs(30),
                    limit: 1,
                    window: Arc::new(Mutex::new((stale, 1))),
                    procedures: Arc::default(),
                },
                bounded,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let response = reqwest::Client::new()
            .post(&url)
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();
    }
}
