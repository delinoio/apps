// SPDX-License-Identifier: FSL-1.1-ALv2
//! HTTP surface: Connect RPCs plus `/healthz`, `/readyz` and `/metrics`.
//!
//! There is no CORS support: the API is called by server-side developers with API keys or user
//! tokens, so any request that carries an `Origin` header is rejected.

use crate::{REQUEST_ID_HEADER, Server, error::ApiError, store, telemetry::Metrics};
use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use connectrpc::{
    CodecFormat, ConnectError, ErrorCode, Interceptor, Next as RpcNext, async_trait,
    interceptor::{UnaryRequest, UnaryResponse},
};
use std::{sync::Arc, time::Duration};

/// Margin added to the execution timeout before the HTTP layer gives up.
const TRANSPORT_MARGIN: Duration = Duration::from_secs(5);

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
        .with_limits(connectrpc::Limits::default().with_max_message_size(limit))
        .with_interceptor(DecodeErrorSanitizer {
            metrics: server.metrics.clone(),
        });
    let metrics = server.metrics.clone();
    let ready = server.db.clone();
    let deadline = server.config.limits.execution_timeout + TRANSPORT_MARGIN;
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
                .layer(middleware::from_fn_with_state(deadline, transport_policy)),
        )
}

/// Replaces the dependency's JSON decoding diagnostics with the fixed, typed
/// `INVALID_INPUT` error.
///
/// Workaround scope: `connectrpc` decodes a JSON request body after the interceptor chain
/// starts and reports a failure as an untyped `invalid_argument` whose message embeds the
/// `serde_json` diagnostic. That diagnostic can echo submitted content, for example a string
/// supplied where `texts` expects an array. The server contract forbids echoing content, and
/// the failure happens before any handler can classify it. Every error that a handler returns
/// is an [`ApiError`] and carries an `ErrorDetail`, so an `invalid_argument` without details
/// on a JSON call can only come from the decoder. Binary calls and all other codes pass
/// through unchanged. Remove this interceptor when `connectrpc` offers a content-free decode
/// error or a decode error hook.
struct DecodeErrorSanitizer {
    metrics: Arc<Metrics>,
}

#[async_trait]
impl Interceptor for DecodeErrorSanitizer {
    async fn intercept_unary(
        &self,
        req: UnaryRequest,
        next: RpcNext<'_>,
    ) -> Result<UnaryResponse, ConnectError> {
        let json = req.payload.format() == CodecFormat::Json;
        match next.run(req).await {
            Err(error)
                if json && error.code == ErrorCode::InvalidArgument && error.details.is_empty() =>
            {
                let request_id = uuid::Uuid::now_v7().to_string();
                // The original diagnostic is deliberately dropped: it may contain content.
                tracing::warn!(category = "decode", request_id = %request_id, "request body could not be decoded");
                let error = ApiError::invalid_input(DECODE_FAILURE_MESSAGE);
                self.metrics
                    .denied
                    .with_label_values(&[error.label()])
                    .inc();
                Err(error.into_connect(&request_id))
            }
            other => other,
        }
    }
}

/// Fixed public message of a request body that cannot be decoded.
const DECODE_FAILURE_MESSAGE: &str = "request body is invalid";

async fn transport_policy(
    State(deadline): State<Duration>,
    request: Request,
    next: Next,
) -> Response {
    if request.headers().contains_key("origin") {
        return StatusCode::FORBIDDEN.into_response();
    }
    match tokio::time::timeout(deadline, next.run(request)).await {
        Ok(response) => response,
        Err(_) => {
            let request_id = uuid::Uuid::now_v7().to_string();
            tracing::warn!(category = "timeout", request_id = %request_id, "request deadline exceeded");
            let error = ApiError::timeout().into_connect(&request_id);
            debug_assert_eq!(error.code, ErrorCode::DeadlineExceeded);
            (
                error.http_status(),
                [
                    ("Content-Type", "application/json"),
                    (REQUEST_ID_HEADER, request_id.as_str()),
                ],
                error.to_json(),
            )
                .into_response()
        }
    }
}
