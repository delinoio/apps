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
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tracing::Instrument;

#[derive(Clone)]
struct Policy {
    timeout: Duration,
    limit: i64,
    window: Arc<Mutex<(Instant, i64)>>,
}
async fn bounded(State(policy): State<Policy>, mut request: Request, next: Next) -> Response {
    let allowed = {
        let mut state = policy.window.lock().unwrap_or_else(|e| e.into_inner());
        if state.0.elapsed() >= Duration::from_secs(60) {
            *state = (Instant::now(), 0)
        }
        state.1 = state.1.saturating_add(1);
        state.1 <= policy.limit
    };
    // Generate the server correlation ID before admission so that every response,
    // including a rate rejection, carries it. Any client-supplied value is replaced.
    let correlation = server_common::id();
    request
        .headers_mut()
        .insert("X-Delcore-Correlation-Id", correlation.parse().unwrap());
    let request_headers = request.headers().clone();
    if !allowed {
        // Log only the correlation ID and category; never headers or request data.
        tracing::warn!(correlation_id = %correlation, category="rate_limited", "request rate limit exceeded");
        // The schema has no rate-limit reason, so use ERROR_REASON_UNSPECIFIED (0).
        // Render with empty request headers so the unary Connect representation is
        // always used: for a gRPC or Connect-streaming content type the request
        // headers would select an HTTP 200 trailers-only or end-stream rendering,
        // which would hide the rejection from health probes, webhook callers and
        // proxies. Every rejection must stay HTTP 429 with the typed JSON body.
        return crate::Fault::new(
            connectrpc::ErrorCode::ResourceExhausted,
            0,
            "request rate limit exceeded",
        )
        .with_correlation(&correlation)
        .into_http_response(&axum::http::HeaderMap::new())
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
    let rpc = connectrpc::Router::new()
        .add_service(server.clone())
        .into_axum_service()
        .with_limits(
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
    #[tokio::test]
    async fn rate_bounds_and_slow_bodies_are_enforced() {
        let app = Router::new()
            .route("/", post(|_: Bytes| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                Policy {
                    timeout: Duration::from_millis(30),
                    limit: 1,
                    window: Arc::new(Mutex::new((Instant::now(), 0))),
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
        let app = Router::new()
            .route("/healthz", get(|| async { StatusCode::OK }))
            .route("/webhooks/paddle", post(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                Policy {
                    timeout: Duration::from_secs(30),
                    limit: 1,
                    window: Arc::new(Mutex::new((Instant::now(), 0))),
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
