// SPDX-License-Identifier: FSL-1.1-ALv2
//! Response adaptation for errors that the Connect transport produces before the DevBase handler
//! runs.
//!
//! The transport decodes a JSON request before it calls the service, so a decoding failure never
//! reaches `Server::handle` and its safe error conversion. The transport renders that failure as
//! `failed to decode JSON: {parser error}`, and parsers such as the `FieldMask` deserializer
//! interpolate the submitted value into the diagnostic. The reply would then echo request text
//! that can be personal data, and it would lack the `ErrorDetail` and trace header that every
//! DevBase error carries.
//!
//! [`sanitize_decode_failure`] runs around the whole Connect service, inside the shared request
//! policy. It replaces only that decoding failure of a known procedure with the same typed error
//! shape as every other DevBase failure. It never inspects, stores or logs the replaced
//! diagnostic, and it does not depend on the shape of the submitted value.
use crate::{Op, typed_error};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::Response,
};
use server_common::Fault;

/// Message prefix the transport gives every JSON decoding failure (`connectrpc::codec`). It
/// identifies only the failure category, never the submitted value that follows it.
const DECODE_FAILURE_PREFIX: &str = "failed to decode JSON";

/// Fixed message of the replacement error. It names no field, value or parser detail.
const SAFE_MESSAGE: &str = "request body could not be decoded";

/// Upper bound of an inspected transport error body. A decoding failure of a message within the
/// 64 KiB application limit is far smaller; the size interceptor rejects larger messages before
/// decoding. A body beyond this bound is not a decoding failure of an accepted message and is
/// replaced as well, so no unexpectedly large diagnostic can be forwarded.
const MAX_INSPECTED_BODY: usize = 1 << 20;

/// Replaces a JSON decoding failure of a known DevBase procedure with a fixed typed
/// `invalid_argument` error: HTTP 400, the safe message, `ERROR_REASON_INVALID_INPUT` and a fresh
/// server-generated trace in `ErrorDetail` and `X-Devbase-Trace-Id`. The incoming trace header is
/// never read. Every other response, including application errors, passes through unchanged.
pub(crate) async fn sanitize_decode_failure(request: Request, next: Next) -> Response {
    // Only the exact allowlisted procedures qualify, so request-controlled text never selects
    // the adaptation and unknown paths keep the transport's own routing response.
    let procedure = Op::from_procedure_path(request.uri().path()).map(Op::name);
    let request_headers = request.headers().clone();
    let response = next.run(request).await;
    let Some(procedure) = procedure else {
        return response;
    };
    // Handler-produced errors always carry the trace header, so an untyped JSON 400 can only
    // come from the transport. Anything else is not a candidate and is returned untouched.
    if response.status() != StatusCode::BAD_REQUEST
        || response.headers().contains_key("x-devbase-trace-id")
        || !is_json(response.headers())
    {
        return response;
    }
    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body, MAX_INSPECTED_BODY).await.ok();
    let decode_failure = match &bytes {
        Some(bytes) => is_decode_failure(bytes),
        // The oversized or unreadable body has been consumed and cannot be forwarded.
        None => true,
    };
    if !decode_failure {
        return Response::from_parts(parts, Body::from(bytes.unwrap_or_default()));
    }
    let trace = server_common::id();
    // Only the bounded category and the procedure are logged, never the replaced diagnostic.
    tracing::warn!(trace_id=%trace,procedure,category="request_decode","request body could not be decoded");
    typed_error(Fault::invalid(SAFE_MESSAGE), &trace)
        .into_http_response(&request_headers)
        .map(Body::new)
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
}

/// True for a Connect error object with code `invalid_argument` whose message begins with the
/// transport's decoding-failure prefix. The message is only compared, never copied or logged.
fn is_decode_failure(body: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct Wire<'a> {
        code: &'a str,
        #[serde(default)]
        message: &'a str,
    }
    serde_json::from_slice::<Wire>(body).is_ok_and(|wire| {
        wire.code == "invalid_argument" && wire.message.starts_with(DECODE_FAILURE_PREFIX)
    })
}
