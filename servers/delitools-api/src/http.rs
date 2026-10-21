// SPDX-License-Identifier: FSL-1.1-ALv2
//! HTTP surface: the Connect service plus `/healthz`, `/readyz` and `/metrics`.
//!
//! `/healthz` reports process liveness. `/readyz` applies the documented readiness rule (schema
//! compatible and a synchronized catalog price). `/metrics` serves Prometheus text with bounded
//! labels. Expose the three operational paths on a restricted network only. A policy layer assigns
//! a fresh UUID v7 request ID to every request, ignoring any client-supplied value, bounds the
//! whole request by the live execution timeout plus the dependency timeout, and returns the
//! request ID on every response.
use crate::{
    Server,
    error::{Fault, REQUEST_ID_HEADER},
    platform::Platform,
    settings::{MAX_INPUT_BYTES_CEILING, MESSAGE_OVERHEAD_BYTES},
};
use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use delino_protocol::delitools::v1::ErrorReason;
use std::{sync::Arc, time::Duration};
use tracing::Instrument;

#[derive(Clone)]
struct Policy {
    platform: Arc<Platform>,
}

async fn bounded(State(policy): State<Policy>, mut request: Request, next: Next) -> Response {
    let request_id = server_common::id();
    let value = HeaderValue::from_str(&request_id).expect("UUID is a valid header value");
    request
        .headers_mut()
        .insert(REQUEST_ID_HEADER, value.clone());
    let request_headers = request.headers().clone();
    let deadline = Duration::from_secs(u64::from(
        policy.platform.settings().execution_timeout_seconds,
    )) + policy.platform.config.timeout();
    let span = tracing::info_span!("delitools.http", request_id = %request_id);
    let mut response = match tokio::time::timeout(deadline, next.run(request).instrument(span))
        .await
    {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!(request_id = %request_id, category = "deadline_exceeded", "request deadline exceeded");
            Fault::new(
                ErrorReason::ERROR_REASON_TIMEOUT,
                "request deadline exceeded",
            )
            .into_connect(&request_id)
            .into_http_response(&request_headers)
            .map(axum::body::Body::new)
        }
    };
    response.headers_mut().insert(REQUEST_ID_HEADER, value);
    response
}

pub fn router(server: Arc<Server>) -> Router {
    let platform = server.platform.clone();
    let rpc = connectrpc::Router::new()
        .add_service(server)
        .into_axum_service()
        .with_limits(
            connectrpc::Limits::default()
                .with_max_message_size(MAX_INPUT_BYTES_CEILING as usize + MESSAGE_OVERHEAD_BYTES),
        );
    let ready = platform.clone();
    let metrics = platform.clone();
    Router::new()
        .route("/healthz", get(|| async { StatusCode::OK }))
        .route(
            "/readyz",
            get(move || {
                let platform = ready.clone();
                async move {
                    match tokio::time::timeout(Duration::from_secs(2), platform.ready()).await {
                        Ok(true) => StatusCode::OK,
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
                        metrics.metrics.encode(),
                    )
                        .into_response()
                }
            }),
        )
        .fallback_service(rpc)
        .layer(middleware::from_fn_with_state(Policy { platform }, bounded))
}
