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
//! The transport likewise rejects a path that is not a registered method with a 404 error that
//! interpolates the submitted path, again without `ErrorDetail` or trace header.
//!
//! [`sanitize_decode_failure`] runs around the whole Connect service, inside the shared request
//! policy. It replaces only those two failures with the same typed error shape as every other
//! DevBase failure. It never inspects, stores or logs the replaced diagnostic, and it does not
//! depend on the shape of the submitted value.
use crate::{Op, typed_error};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::Response,
};
use server_common::Fault;
use std::borrow::Cow;

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

/// Fixed message of the replacement for an unmatched procedure. It never contains the submitted
/// selector.
const UNKNOWN_PROCEDURE_MESSAGE: &str = "procedure not found";

/// Replaces two transport failures that occur before the handler runs with the typed DevBase
/// error shape (fixed message, `ERROR_REASON_INVALID_INPUT` and a fresh server-generated trace in
/// `ErrorDetail` and `X-Devbase-Trace-Id`; the incoming trace header is never read):
///
/// - the JSON decoding failure of a known procedure becomes `invalid_argument` (HTTP 400);
/// - the JSON 404 `unimplemented` rejection of a path that is not an exact DevBase procedure
///   becomes the same code and status with the message `procedure not found`.
///
/// Every other response, including application errors, passes through unchanged.
pub(crate) async fn sanitize_decode_failure(request: Request, next: Next) -> Response {
    // Only the exact allowlist decides which adaptation applies, so request-controlled text never
    // selects it and no raw dependency diagnostic is matched.
    let procedure = Op::from_procedure_path(request.uri().path()).map(Op::name);
    let request_headers = request.headers().clone();
    let response = next.run(request).await;
    match procedure {
        Some(procedure) => sanitize_known(response, procedure, &request_headers).await,
        None => sanitize_unmatched(response).await,
    }
}

async fn sanitize_known(
    response: Response,
    procedure: &'static str,
    request_headers: &HeaderMap,
) -> Response {
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
        .into_http_response(request_headers)
        .map(Body::new)
}

/// The transport reports a path that is not a registered method as a JSON 404 `unimplemented`
/// error whose message interpolates the submitted path, after it has bounded and drained the body
/// and applied the request deadline. Earlier rejections (timeout, size, unsupported protocol)
/// have other statuses or codes and pass through. Only the error code is read, never the message.
async fn sanitize_unmatched(response: Response) -> Response {
    if response.status() != StatusCode::NOT_FOUND
        || response.headers().contains_key("x-devbase-trace-id")
        || !is_json(response.headers())
    {
        return response;
    }
    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body, MAX_INSPECTED_BODY).await.ok();
    let unmatched = match &bytes {
        Some(bytes) => is_unimplemented(bytes),
        // The oversized or unreadable body has been consumed and cannot be forwarded.
        None => true,
    };
    if !unmatched {
        return Response::from_parts(parts, Body::from(bytes.unwrap_or_default()));
    }
    let trace = server_common::id();
    // Neither the path nor the dependency message is logged.
    tracing::info!(trace_id=%trace,category="unknown_procedure","procedure not found");
    // The replacement is always a unary JSON error. The request headers are deliberately not
    // consulted, so a streaming content type cannot select another response shape or status.
    // The invalid-input fault supplies `ERROR_REASON_INVALID_INPUT`; the selector is the invalid
    // input, while the Connect code and the HTTP status keep the routing semantics.
    let mut fault = Fault::invalid(UNKNOWN_PROCEDURE_MESSAGE);
    fault.code = connectrpc::ErrorCode::Unimplemented;
    typed_error(fault, &trace)
        .with_http_status(StatusCode::NOT_FOUND)
        .into_http_response(&HeaderMap::new())
        .map(Body::new)
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
}

/// True for a Connect error object with code `invalid_argument` whose message begins with the
/// transport's decoding-failure prefix. The message is only compared, never logged or returned.
fn is_decode_failure(body: &[u8]) -> bool {
    // `Cow` borrows an unescaped string and owns one that needs unescaping. The transport escapes
    // quotes, backslashes and control characters in the diagnostic, so a plain `&str` would fail
    // to deserialize exactly for the diagnostics that echo such submitted text.
    #[derive(serde::Deserialize)]
    struct Wire<'a> {
        #[serde(borrow)]
        code: Cow<'a, str>,
        #[serde(borrow, default)]
        message: Cow<'a, str>,
    }
    serde_json::from_slice::<Wire>(body).is_ok_and(|wire| {
        wire.code == "invalid_argument" && wire.message.starts_with(DECODE_FAILURE_PREFIX)
    })
}

/// True for a Connect error object with code `unimplemented`. The message is not read.
fn is_unimplemented(body: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct Wire<'a> {
        code: &'a str,
    }
    serde_json::from_slice::<Wire>(body).is_ok_and(|wire| wire.code == "unimplemented")
}

#[cfg(test)]
mod tests {
    use super::is_decode_failure;

    #[test]
    fn decode_failure_with_escaped_diagnostic_is_recognized() {
        let escaped = br#"{"code":"invalid_argument","message":"failed to decode JSON: bad \"q\" and \\ and \u0001"}"#;
        assert!(is_decode_failure(escaped));
        let plain = br#"{"code":"invalid_argument","message":"failed to decode JSON: x"}"#;
        assert!(is_decode_failure(plain));
    }

    #[test]
    fn other_errors_are_not_decode_failures() {
        let other = br#"{"code":"invalid_argument","message":"a \"quoted\" application error"}"#;
        assert!(!is_decode_failure(other));
        let code = br#"{"code":"unauthenticated","message":"failed to decode JSON: x"}"#;
        assert!(!is_decode_failure(code));
        assert!(!is_decode_failure(b"not json"));
    }
}
