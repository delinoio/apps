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
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use connectrpc::ErrorCode;
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
        Ok(response) => typed_transport_error(response, &request_id).await,
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
    // A duplicate-call error carries the existing request's ID in its own header; every other
    // response carries this call's ID.
    if !response.headers().contains_key(REQUEST_ID_HEADER) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

/// Largest error body inspected for normalization. Connect error bodies are small JSON objects.
const MAX_ERROR_BODY_BYTES: usize = 64 << 10;

/// The Connect code the framework used for an HTTP status, for a body too large to read. Codes
/// that share a status (`invalid_argument`, `failed_precondition` and `out_of_range` all use 400)
/// resolve to the first listed, which is the code the framework itself produces for undecodable
/// input. A status no Connect code maps to is an internal fault.
fn code_for_status(status: StatusCode) -> ErrorCode {
    [
        ErrorCode::InvalidArgument,
        ErrorCode::Unauthenticated,
        ErrorCode::PermissionDenied,
        ErrorCode::NotFound,
        ErrorCode::AlreadyExists,
        ErrorCode::ResourceExhausted,
        ErrorCode::Unimplemented,
        ErrorCode::Unavailable,
        ErrorCode::DeadlineExceeded,
        ErrorCode::Canceled,
    ]
    .into_iter()
    .find(|code| code.http_status() == status)
    .unwrap_or(ErrorCode::Internal)
}

/// The typed fault for a failure the Connect framework produced before a handler ran. The
/// framework message is dropped because decoder errors can quote request content; the Connect
/// code (and the HTTP status the framework chose) are kept.
fn transport_fault(code: ErrorCode) -> Fault {
    match code {
        ErrorCode::InvalidArgument => Fault::invalid_input("the request could not be decoded"),
        ErrorCode::ResourceExhausted => Fault::limit("the request exceeds a size limit"),
        ErrorCode::Unimplemented => Fault::not_implemented("the procedure is not available"),
        ErrorCode::DeadlineExceeded => Fault::new(
            ErrorReason::ERROR_REASON_TIMEOUT,
            "request deadline exceeded",
        ),
        ErrorCode::Canceled => Fault::new(ErrorReason::ERROR_REASON_CANCELED, "request canceled"),
        other => Fault {
            code: other,
            ..Fault::new(
                ErrorReason::ERROR_REASON_INTERNAL,
                "the request could not be processed",
            )
        },
    }
}

/// Give a Connect unary error that the framework generated (malformed JSON or protobuf, a body
/// over the message limit, an unknown procedure, an unsupported content type) the same typed
/// `ErrorDetail` and request ID that handler failures carry. Handler failures already hold a detail and pass through unchanged, as
/// do successes and responses of other shapes (health, readiness, metrics, and the status-only
/// gRPC and streaming forms, which this service does not document).
async fn typed_transport_error(response: Response, request_id: &str) -> Response {
    let status = response.status();
    // The framework answers an unsupported content type with a bare 415 and no body, because no
    // Connect protocol was negotiated that could carry an error.
    let unsupported_media_type = status == StatusCode::UNSUPPORTED_MEDIA_TYPE
        && !response.headers().contains_key(header::CONTENT_TYPE);
    let is_connect_error_body = (status.is_client_error() || status.is_server_error())
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"));
    if !is_connect_error_body && !unsupported_media_type {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let (parsed, bytes) = match axum::body::to_bytes(body, MAX_ERROR_BODY_BYTES).await {
        Ok(bytes) => (
            serde_json::from_slice::<serde_json::Value>(&bytes).ok(),
            Some(bytes),
        ),
        // The body exceeds the inspection cap, for example a decoder diagnostic that quotes a very
        // long caller-controlled field name. The body cannot be trusted or parsed, so keep the
        // framework's HTTP status, derive the Connect code from it and replace the body.
        Err(_) => (None, None),
    };
    let has_detail = parsed
        .as_ref()
        .and_then(|error| error["details"].as_array())
        .is_some_and(|details| !details.is_empty());
    if let (true, Some(bytes)) = (has_detail, bytes) {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let code = if unsupported_media_type {
        ErrorCode::InvalidArgument
    } else if let Some(error) = parsed.as_ref() {
        error["code"]
            .as_str()
            .and_then(|code| code.parse::<ErrorCode>().ok())
            .unwrap_or(ErrorCode::Internal)
    } else {
        code_for_status(status)
    };
    tracing::warn!(
        request_id = %request_id,
        category = "transport_rejected",
        code = code.as_str(),
        status = status.as_u16(),
        "request rejected before reaching a handler"
    );
    let fault = if unsupported_media_type {
        Fault::invalid_input("the request content type is not supported")
    } else {
        transport_fault(code)
    };
    let body = fault.into_connect(request_id).to_json();
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    parts
        .headers
        .insert(header::CONTENT_LENGTH, body.len().into());
    Response::from_parts(parts, Body::from(body))
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
    use buffa::Message;
    use delino_protocol::delitools::v1::ErrorDetail;

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn framework_error(code: connectrpc::ConnectError) -> Response {
        code.into_http_response(&HeaderMap::new()).map(Body::new)
    }

    fn detail_of(error: &serde_json::Value) -> ErrorDetail {
        assert_eq!(error["details"][0]["type"], "delitools.v1.ErrorDetail");
        let raw = STANDARD_NO_PAD
            .decode(error["details"][0]["value"].as_str().unwrap())
            .unwrap();
        ErrorDetail::decode_from_slice(&raw).unwrap()
    }

    /// Framework-generated rejections carry the typed detail and request ID, keep the framework's
    /// status, and never repeat the framework's message, which can quote request content.
    #[tokio::test]
    async fn framework_errors_gain_a_typed_detail_and_request_id() {
        let id = server_common::id();
        for (error, reason, status) in [
            (
                connectrpc::ConnectError::invalid_argument(
                    "failed to decode JSON: unknown field `secret-value`",
                ),
                ErrorReason::ERROR_REASON_INVALID_INPUT,
                StatusCode::BAD_REQUEST,
            ),
            (
                connectrpc::ConnectError::resource_exhausted("message is larger than the limit"),
                ErrorReason::ERROR_REASON_LIMIT,
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (
                connectrpc::ConnectError::unimplemented("no such procedure secret-value"),
                ErrorReason::ERROR_REASON_UNSPECIFIED,
                StatusCode::NOT_IMPLEMENTED,
            ),
            (
                connectrpc::ConnectError::new(ErrorCode::DataLoss, "secret-value"),
                ErrorReason::ERROR_REASON_INTERNAL,
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ] {
            let code = error.code;
            let response = typed_transport_error(framework_error(error), &id).await;
            assert_eq!(response.status(), status, "{code:?}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
            let length = response.headers()[header::CONTENT_LENGTH].clone();
            let body = body_json(response).await;
            assert_eq!(body["code"], code.as_str());
            assert!(!body.to_string().contains("secret-value"), "{body}");
            let detail = detail_of(&body);
            assert_eq!(detail.reason, reason, "{code:?}");
            assert_eq!(detail.request_id, id);
            assert_eq!(length.to_str().unwrap(), body_len(&body).to_string());
        }
    }

    fn body_len(body: &serde_json::Value) -> usize {
        // The rewritten body is exactly the serialized error object.
        serde_json::to_vec(body).unwrap().len()
    }

    /// A framework error body above the inspection cap keeps the framework's HTTP status and the
    /// Connect code that status implies, instead of becoming an internal fault.
    #[tokio::test]
    async fn an_oversized_framework_error_keeps_its_status_and_code() {
        let id = server_common::id();
        let quoted = "secret-value".repeat(MAX_ERROR_BODY_BYTES / 4);
        assert!(quoted.len() > MAX_ERROR_BODY_BYTES);
        for (error, reason, status) in [
            (
                connectrpc::ConnectError::invalid_argument(format!("unknown field `{quoted}`")),
                ErrorReason::ERROR_REASON_INVALID_INPUT,
                StatusCode::BAD_REQUEST,
            ),
            (
                connectrpc::ConnectError::resource_exhausted(quoted.clone()),
                ErrorReason::ERROR_REASON_LIMIT,
                StatusCode::TOO_MANY_REQUESTS,
            ),
        ] {
            let code = error.code;
            let framework = framework_error(error);
            let response = typed_transport_error(framework, &id).await;
            assert_eq!(response.status(), status, "{code:?}");
            let body = body_json(response).await;
            assert_eq!(body["code"], code.as_str());
            assert!(!body.to_string().contains("secret-value"), "{code:?}");
            assert_eq!(detail_of(&body).reason, reason, "{code:?}");
        }
        assert_eq!(
            code_for_status(StatusCode::IM_A_TEAPOT),
            ErrorCode::Internal
        );
    }

    /// An unsupported content type is answered with a bare 415. It gains a typed error body and
    /// keeps its status and its `Accept-Post` hint.
    #[tokio::test]
    async fn an_unsupported_content_type_gains_a_typed_error_body() {
        let id = server_common::id();
        let bare = Response::builder()
            .status(StatusCode::UNSUPPORTED_MEDIA_TYPE)
            .header("accept-post", "application/json")
            .body(Body::empty())
            .unwrap();
        let response = typed_transport_error(bare, &id).await;
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(response.headers()["accept-post"], "application/json");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let body = body_json(response).await;
        assert_eq!(body["code"], "invalid_argument");
        let detail = detail_of(&body);
        assert_eq!(detail.reason, ErrorReason::ERROR_REASON_INVALID_INPUT);
        assert_eq!(detail.request_id, id);
    }

    /// Handler failures already carry a detail with their own reason, and non-error or non-Connect
    /// responses are not touched.
    #[tokio::test]
    async fn typed_and_unrelated_responses_pass_through() {
        let id = server_common::id();
        let handler_id = server_common::id();
        let fault = Fault::forbidden().into_connect(&handler_id);
        let response = typed_transport_error(framework_error(fault), &id).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let detail = detail_of(&body_json(response).await);
        assert_eq!(detail.reason, ErrorReason::ERROR_REASON_FORBIDDEN);
        assert_eq!(detail.request_id, handler_id, "a handler's own ID is kept");

        let ok = Response::new(Body::from("{}"));
        assert_eq!(
            typed_transport_error(ok, &id).await.status(),
            StatusCode::OK
        );
        let unavailable = (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response();
        let response = typed_transport_error(unavailable, &id).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"not ready");
    }
}
