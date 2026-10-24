// SPDX-License-Identifier: FSL-1.1-ALv2
//! HTTP surface: Connect RPCs plus `/healthz`, `/readyz` and `/metrics`.
//!
//! There is no CORS support: the API is called by server-side developers with API keys or user
//! tokens, so any request that carries an `Origin` header is rejected.

use crate::{REQUEST_ID_HEADER, Server, config::Limits, error::ApiError, store};
use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use connectrpc::ErrorCode;
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// The absolute execution deadline of one request, anchored when the HTTP layer received it.
///
/// `transport_policy` inserts it into the request extensions so that the handler, which only
/// runs after the whole body is received and decoded, shares one deadline with the transport
/// instead of starting a fresh execution budget.
#[derive(Clone, Debug)]
pub struct IngressClock {
    /// The instant the execution timeout expires, measured from request receipt.
    pub deadline: Instant,
    /// Cancelled by the handler as soon as it is dispatched; see [`IngressClock::dispatch`].
    dispatched: CancellationToken,
}

impl IngressClock {
    fn new(deadline: Instant) -> Self {
        Self {
            deadline,
            dispatched: CancellationToken::new(),
        }
    }

    /// Tells the transport that the body is complete and the handler runs. From then on the
    /// transport grants the finalization allowance beyond the deadline instead of cutting the
    /// request at the deadline.
    pub fn dispatch(&self) {
        self.dispatched.cancel();
    }
}

pub fn router(server: Server) -> Router {
    // One decoded byte can take six bytes on the wire (`\u00XX` for a control
    // character), so the transport bound covers the worst-case JSON escaping of the
    // configured input and schema limits. Handlers apply the exact limits after decoding.
    let limit = server
        .config
        .limits
        .max_input_bytes
        .saturating_add(server.config.limits.max_schema_bytes)
        .saturating_mul(6)
        .saturating_add(64 << 10);
    let rpc = connectrpc::Router::new()
        .add_service(Arc::new(server.clone()))
        .into_axum_service()
        .with_limits(connectrpc::Limits::default().with_max_message_size(limit));
    let metrics = server.metrics.clone();
    let ready = server.db.clone();
    let limits = server.config.limits.clone();
    Router::new()
        .route("/healthz", get(|| async { StatusCode::OK }))
        .route(
            "/readyz",
            get(move || {
                let db = ready.clone();
                async move {
                    match tokio::time::timeout(Duration::from_secs(1), store::check(&db)).await {
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
        .fallback_service(
            Router::new()
                .fallback_service(rpc)
                .layer(middleware::from_fn_with_state(limits, transport_policy)),
        )
}

/// Request headers through which a caller can ask Connect to apply its own deadline.
///
/// Connect resolves these before any interceptor runs, drops the handler future when the
/// deadline passes (cancelling provider work) and answers with a plain `deadline_exceeded`
/// that carries neither `ErrorDetail` nor the request ID. They are removed in
/// [`transport_policy`]. `grpc-timeout` is not a documented protocol of this API; it is removed
/// only because the router also accepts it and it would select the same untyped deadline.
const CALLER_DEADLINE_HEADERS: [&str; 2] = ["connect-timeout-ms", "grpc-timeout"];

/// Rejects browser-origin requests and enforces the request deadline.
///
/// Caller-asserted deadline headers are ignored: the server-owned execution deadline is the only
/// deadline. A caller that wants a shorter wait cancels by closing the call, which cancels the
/// provider work as for any disconnect. Absent, short, long and malformed header values therefore
/// behave identically.
///
/// The execution timeout starts here, at receipt. Until the handler is dispatched (the body is
/// complete and decoded) the request is cut at the execution deadline with the typed `TIMEOUT`
/// error, so a slow upload can neither reach provider execution nor outlive the budget. After
/// dispatch the handler owns the deadline: it cancels provider work at the same instant, and the
/// transport margin only leaves time to finalize accounting and write the response. The margin
/// never authorizes another provider attempt.
async fn transport_policy(
    State(limits): State<Limits>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.headers().contains_key("origin") {
        return StatusCode::FORBIDDEN.into_response();
    }
    for header in CALLER_DEADLINE_HEADERS {
        request.headers_mut().remove(header);
    }
    let received_at = Instant::now();
    // `None` only for a configuration that bypassed validation: requests then fail closed.
    let (Some(deadline), Some(final_deadline)) = (
        limits.deadline_from(received_at),
        limits.transport_deadline_from(received_at),
    ) else {
        let request_id = uuid::Uuid::now_v7().to_string();
        tracing::error!(category = "deadline", request_id = %request_id, "the execution timeout exceeds the monotonic clock range");
        return error_response(ApiError::internal(), &request_id);
    };
    let clock = IngressClock::new(deadline);
    let dispatched = clock.dispatched.clone();
    request.extensions_mut().insert(clock);
    let response = next.run(request);
    tokio::pin!(response);
    // Phase 1: receiving and decoding the body. The execution deadline applies.
    let phase_one = tokio::select! {
        response = &mut response => Some(response),
        _ = dispatched.cancelled() => None,
        _ = tokio::time::sleep_until(deadline) => {
            return timeout_response("the request deadline passed before the request was complete");
        }
    };
    if let Some(response) = phase_one {
        return response;
    }
    // Phase 2: the handler runs against the same absolute deadline; the margin is only the
    // allowance to finalize accounting and answer.
    match tokio::time::timeout_at(final_deadline, response).await {
        Ok(response) => response,
        Err(_) => timeout_response("the request deadline passed with the margin"),
    }
}

fn timeout_response(message: &'static str) -> Response {
    let request_id = uuid::Uuid::now_v7().to_string();
    tracing::warn!(category = "timeout", request_id = %request_id, "request deadline exceeded: {message}");
    let error = ApiError::timeout();
    debug_assert_eq!(error.code, ErrorCode::DeadlineExceeded);
    error_response(error, &request_id)
}

fn error_response(error: ApiError, request_id: &str) -> Response {
    let error = error.into_connect(request_id);
    (
        error.http_status(),
        [
            ("Content-Type", "application/json"),
            (REQUEST_ID_HEADER, request_id),
        ],
        error.to_json(),
    )
        .into_response()
}
