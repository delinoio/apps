// SPDX-License-Identifier: FSL-1.1-ALv2
//! HTTP routing, Origin and CORS policy, health and metrics endpoints.
use crate::{CORRELATION_HEADER, MAX_BODY_BYTES, Server, call::RequestMeta};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use delino_protocol::anydash::v1::*;
use std::{net::SocketAddr, sync::Arc, time::Duration};

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
    if crate::worker::refresh_gauges(&s).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
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
        let (mut parts, body) = req.into_parts();
        match to_bytes(body, HARD_BODY_LIMIT).await {
            Ok(bytes) => {
                parts.extensions.insert(RequestMeta {
                    correlation: correlation.clone(),
                    client_ip: ip,
                    body_bytes: bytes.len(),
                });
                let req = Request::from_parts(parts, Body::from(bytes));
                match tokio::time::timeout(REQUEST_TIMEOUT, next.run(req)).await {
                    Ok(response) => response,
                    Err(_) => {
                        tracing::warn!(correlation_id = %correlation, "request timed out");
                        StatusCode::GATEWAY_TIMEOUT.into_response()
                    }
                }
            }
            Err(_) => StatusCode::PAYLOAD_TOO_LARGE.into_response(),
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
