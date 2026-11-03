// SPDX-License-Identifier: FSL-1.1-ALv2
//! HTTP routing, Origin and CORS policy, health and metrics endpoints.
use crate::{CORRELATION_HEADER, MAX_BODY_BYTES, Server, call::RequestMeta};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use delino_protocol::anydash::v1::*;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::time::Instant;

/// Bodies above this bound are refused without decoding; bodies between 1 MiB and this bound get
/// a typed `PAYLOAD_TOO_LARGE` error from the handler.
const HARD_BODY_LIMIT: usize = 4 * MAX_BODY_BYTES;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

pub fn router(server: Arc<Server>) -> Router {
    let rpc = connectrpc::Router::new()
        .add_service::<_, DeviceServiceRegisterMarker>(server.clone())
        .add_service::<_, CollectorServiceRegisterMarker>(server.clone())
        .add_service::<_, ConnectionServiceRegisterMarker>(server.clone())
        .add_service::<_, DashboardServiceRegisterMarker>(server.clone())
        .add_service::<_, WidgetDataServiceRegisterMarker>(server.clone())
        .add_service::<_, AccountServiceRegisterMarker>(server.clone())
        .into_axum_service()
        .with_limits(
            connectrpc::Limits::default()
                .with_max_message_size(HARD_BODY_LIMIT)
                .with_max_request_body_size(HARD_BODY_LIMIT),
        );
    Router::new()
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(ready))
        .route("/metrics", get(metrics))
        .with_state(server.clone())
        .fallback_service(rpc)
        .layer(middleware::from_fn_with_state(server, policy))
}

async fn ready(State(s): State<Arc<Server>>) -> StatusCode {
    match tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query("SELECT 1").execute(&s.db),
    )
    .await
    {
        Ok(Ok(_)) => StatusCode::OK,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"anydash-metrics").expect("HMAC key");
    mac.update(a);
    let left = mac.finalize().into_bytes();
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"anydash-metrics").expect("HMAC key");
    mac.update(b);
    mac.verify_slice(&left).is_ok()
}

async fn metrics(State(s): State<Arc<Server>>, headers: HeaderMap) -> Response {
    let supplied = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    let expected = format!("Bearer {}", s.config.metrics_token);
    if !constant_time_eq(supplied.as_bytes(), expected.as_bytes()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // A database outage must not hide the metrics that explain it: the request counters and the
    // timeout counter live in memory. Serve the registry with the last gauge values and count the
    // failure instead of failing the scrape.
    if let Err(e) = crate::worker::refresh_gauges(&s).await {
        s.metrics.gauge_refresh_failures.inc();
        tracing::warn!(error = %e, "database-backed gauges could not be refreshed for a scrape");
    }
    (
        [("content-type", "text/plain; version=0.0.4")],
        s.metrics.encode(),
    )
        .into_response()
}

fn client_ip(config: &crate::config::Config, req: &Request) -> String {
    if let Some(name) = &config.client_ip_header
        && let Some(value) = req.headers().get(name).and_then(|v| v.to_str().ok())
        && let Ok(ip) = value
            .split(',')
            .next()
            .unwrap_or_default()
            .trim()
            .parse::<std::net::IpAddr>()
    {
        return ip.to_string();
    }
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_default()
}

/// Why a request body could not be buffered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyFailure {
    /// The body did not arrive completely before the request deadline.
    DeadlineExceeded,
    /// The body exceeded `HARD_BODY_LIMIT` or could not be read.
    Rejected,
}

/// Buffers a request body of at most `HARD_BODY_LIMIT` bytes that must arrive before `deadline`.
async fn buffer_body(body: Body, deadline: Instant) -> Result<Bytes, BodyFailure> {
    match tokio::time::timeout_at(deadline, to_bytes(body, HARD_BODY_LIMIT)).await {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(_)) => Err(BodyFailure::Rejected),
        Err(_) => Err(BodyFailure::DeadlineExceeded),
    }
}

async fn policy(State(s): State<Arc<Server>>, req: Request, next: Next) -> Response {
    let correlation = crate::new_id().to_string();
    let origin = req.headers().get("origin").cloned();
    let allowed = origin
        .as_ref()
        .is_none_or(|o| o.as_bytes() == s.config.web_origin.as_bytes());
    let mut response = if !allowed {
        tracing::warn!(correlation_id = %correlation, method = %req.method(), "request rejected by Origin policy");
        StatusCode::FORBIDDEN.into_response()
    } else if req.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else {
        let ip = client_ip(&s.config, &req);
        // One absolute deadline covers body buffering and the handler. The transport's idle timer
        // restarts on every received byte, so it cannot bound a body that arrives slowly.
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let (mut parts, body) = req.into_parts();
        match buffer_body(body, deadline).await {
            Ok(bytes) => {
                parts.extensions.insert(RequestMeta {
                    correlation: correlation.clone(),
                    client_ip: ip,
                    body_bytes: bytes.len(),
                });
                let req = Request::from_parts(parts, Body::from(bytes));
                match tokio::time::timeout_at(deadline, next.run(req)).await {
                    Ok(response) => response,
                    Err(_) => {
                        // The dropped handler future cannot record this request, so count it here.
                        s.metrics.timeouts.inc();
                        tracing::warn!(correlation_id = %correlation, "request timed out");
                        StatusCode::GATEWAY_TIMEOUT.into_response()
                    }
                }
            }
            Err(BodyFailure::DeadlineExceeded) => {
                tracing::warn!(correlation_id = %correlation, "request body not received before the request deadline");
                StatusCode::REQUEST_TIMEOUT.into_response()
            }
            Err(BodyFailure::Rejected) => StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        }
    };
    let h = response.headers_mut();
    if !h.contains_key(CORRELATION_HEADER) {
        h.insert(
            CORRELATION_HEADER,
            correlation.parse().expect("UUID header value"),
        );
    }
    h.insert("cache-control", HeaderValue::from_static("no-store"));
    if allowed && origin.is_some() {
        // Bearer tokens carry credentials, so CORS never allows cookies or arbitrary origins.
        h.insert(
            "access-control-allow-origin",
            s.config.web_origin.parse().expect("validated origin"),
        );
        h.insert("vary", HeaderValue::from_static("Origin"));
        h.insert(
            "access-control-allow-methods",
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        h.insert(
            "access-control-allow-headers",
            HeaderValue::from_static(
                "authorization, content-type, connect-protocol-version, connect-timeout-ms, \
                 x-anydash-client-kind, x-anydash-client-version, x-grpc-web, x-user-agent, grpc-timeout",
            ),
        );
        h.insert(
            "access-control-expose-headers",
            HeaderValue::from_static(
                "x-anydash-correlation-id, retry-after, grpc-status, grpc-message, grpc-status-details-bin",
            ),
        );
        h.insert("access-control-max-age", HeaderValue::from_static("600"));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    /// A body that sends one byte every 30 seconds and never ends.
    struct Trickle(Pin<Box<tokio::time::Sleep>>);

    impl axum::body::HttpBody for Trickle {
        type Data = Bytes;
        type Error = std::convert::Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<http_body::Frame<Bytes>, Self::Error>>> {
            match self.0.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    let next = Instant::now() + Duration::from_secs(30);
                    self.0.as_mut().reset(next);
                    Poll::Ready(Some(Ok(http_body::Frame::data(Bytes::from_static(b"x")))))
                }
                Poll::Pending => Poll::Pending,
            }
        }
    }

    /// A server whose database pool never connects; policy tests do not touch it.
    fn lazy_server() -> Arc<Server> {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://anydash@127.0.0.1:1/anydash")
            .expect("lazy pool");
        let config = crate::config::Config::fixture(
            "https://issuer.example.test",
            "https://delcore.example.test",
            "https://prices.example.test/prices.json",
        );
        Arc::new(Server::new(db, config).expect("test server"))
    }

    #[tokio::test]
    async fn metrics_scrape_survives_a_gauge_refresh_failure() {
        use tower::ServiceExt;
        // Nothing listens on 127.0.0.1:1, so every gauge query fails fast.
        let db = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(1))
            .connect_lazy("postgres://anydash@127.0.0.1:1/anydash")
            .expect("lazy pool");
        let config = crate::config::Config::fixture(
            "https://issuer.example.test",
            "https://delcore.example.test",
            "https://prices.example.test/prices.json",
        );
        let token = config.metrics_token.clone();
        let server = Arc::new(Server::new(db, config).expect("test server"));
        server
            .metrics
            .requests
            .with_label_values(&["AccountService", "GetAccount", "server_error"])
            .inc();
        let app = router(server.clone());
        let scrape = |authorization: Option<String>| {
            let mut req = Request::builder().uri("/metrics");
            if let Some(a) = authorization {
                req = req.header("authorization", a);
            }
            app.clone().oneshot(req.body(Body::empty()).unwrap())
        };
        // Without the token the response is still 401 and the refresh is not attempted.
        assert_eq!(
            scrape(None).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(server.metrics.gauge_refresh_failures.get(), 0);
        for expected in 1..=2 {
            let response = scrape(Some(format!("Bearer {token}"))).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let text = String::from_utf8(body.to_vec()).unwrap();
            assert!(text.contains(
                "anydash_api_requests_total{method=\"GetAccount\",outcome=\"server_error\",service=\"AccountService\"} 1"
            ));
            // The counter is incremented before the registry is encoded.
            assert!(
                text.contains(&format!("anydash_gauge_refresh_failures_total {expected}")),
                "{text}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn handler_exceeding_the_deadline_returns_504_and_counts_a_timeout() {
        use tower::ServiceExt;
        let server = lazy_server();
        let app = Router::new()
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(REQUEST_TIMEOUT + Duration::from_secs(1)).await;
                    StatusCode::OK
                }),
            )
            .route("/fast", get(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(server.clone(), policy));
        let send = |path: &'static str| {
            app.clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        };
        let fast = send("/fast").await.unwrap();
        assert_eq!(fast.status(), StatusCode::OK);
        assert_eq!(server.metrics.timeouts.get(), 0);
        let slow = send("/slow").await.unwrap();
        assert_eq!(slow.status(), StatusCode::GATEWAY_TIMEOUT);
        assert!(slow.headers().contains_key(CORRELATION_HEADER));
        assert_eq!(server.metrics.timeouts.get(), 1);
        assert!(
            server
                .metrics
                .encode()
                .contains("anydash_api_timeouts_total 1")
        );
        // A later normal request is unaffected.
        assert_eq!(send("/fast").await.unwrap().status(), StatusCode::OK);
        assert_eq!(server.metrics.timeouts.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_body_is_bounded_by_the_request_deadline() {
        let start = Instant::now();
        let body = Body::new(Trickle(Box::pin(tokio::time::sleep(Duration::from_secs(
            30,
        )))));
        assert_eq!(
            buffer_body(body, start + REQUEST_TIMEOUT).await,
            Err(BodyFailure::DeadlineExceeded)
        );
        assert_eq!(start.elapsed(), REQUEST_TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn complete_and_oversized_bodies() {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        assert_eq!(
            buffer_body(Body::from("{}"), deadline).await,
            Ok(Bytes::from_static(b"{}"))
        );
        assert_eq!(
            buffer_body(Body::from(vec![0u8; HARD_BODY_LIMIT + 1]), deadline).await,
            Err(BodyFailure::Rejected)
        );
    }
}
