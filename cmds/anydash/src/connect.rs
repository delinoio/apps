// SPDX-License-Identifier: Apache-2.0
//! Connect unary protocol helpers: binary Protobuf bodies and JSON error envelopes carrying
//! exactly one `anydash.v1.ErrorDetail`.
use crate::{
    error::{Failure, new_id},
    pb::{ErrorDetail, ErrorReason},
};
use base64::Engine as _;
use prost::Message as _;
use std::time::Duration;
pub const CONTENT_TYPE: &str = "application/proto";
pub const DETAIL_TYPE: &str = "anydash.v1.ErrorDetail";
#[must_use]
pub const fn http_status(code: &str) -> u16 {
    match code.as_bytes() {
        b"invalid_argument" | b"failed_precondition" | b"out_of_range" => 400,
        b"unauthenticated" => 401,
        b"permission_denied" => 403,
        b"not_found" => 404,
        b"aborted" | b"already_exists" => 409,
        b"resource_exhausted" => 429,
        b"unimplemented" => 501,
        b"unavailable" => 503,
        b"deadline_exceeded" => 504,
        _ => 500,
    }
}
/// Encode a failure as a Connect error body and its HTTP status.
#[must_use]
pub fn encode_error(failure: &Failure) -> (u16, Vec<u8>) {
    let mut detail = (*failure.detail).clone();
    detail.reason = failure.reason.into();
    detail.correlation_id.clone_from(&failure.correlation_id);
    // An API failure can carry its wait only in the `Retry-After` header; keep it for local
    // IPC clients.
    if detail.retry_after.is_none()
        && let Some(wait) = failure.retry_after
    {
        detail.retry_after = Some(crate::error::duration(wait));
    }
    let code = failure.connect_code();
    let body = serde_json::json!({
        "code": code,
        "message": failure.message,
        "details": [{
            "type": DETAIL_TYPE,
            "value": base64::engine::general_purpose::STANDARD_NO_PAD.encode(detail.encode_to_vec()),
        }],
    });
    (http_status(code), body.to_string().into_bytes())
}
#[must_use]
pub fn unimplemented() -> (u16, Vec<u8>) {
    let body = serde_json::json!({"code": "unimplemented", "message": "Unknown method."});
    (501, body.to_string().into_bytes())
}
/// Decode a Connect error body. A body without an `ErrorDetail` keeps the Connect code.
#[must_use]
pub fn decode_error(
    status: u16,
    body: &[u8],
    correlation: Option<&str>,
    retry_after: Option<Duration>,
) -> Failure {
    let value: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let message = value["message"]
        .as_str()
        .unwrap_or("Request failed.")
        .to_owned();
    let code = value["code"].as_str().unwrap_or("");
    let detail = value["details"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["type"].as_str() == Some(DETAIL_TYPE))
        .find_map(|d| {
            let text = d["value"].as_str()?.trim_end_matches('=');
            let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(text)
                .ok()?;
            ErrorDetail::decode(bytes.as_slice()).ok()
        });
    let mut detail = detail.unwrap_or_else(|| ErrorDetail {
        reason: match (code, status) {
            ("unimplemented", _) | (_, 501) => ErrorReason::Unspecified,
            ("unauthenticated", _) | (_, 401) => ErrorReason::Unauthenticated,
            ("resource_exhausted", _) | (_, 429) => ErrorReason::RateLimited,
            ("unavailable", _) | (_, 502..=504) => ErrorReason::DependencyUnavailable,
            _ => ErrorReason::Internal,
        }
        .into(),
        ..ErrorDetail::default()
    });
    if detail.correlation_id.is_empty() {
        detail.correlation_id = correlation.map_or_else(new_id, str::to_owned);
    }
    let mut failure = Failure::from_detail(detail, &message, retry_after);
    if code == "unimplemented" || status == 501 {
        "The other component does not support this request; update the older AnyDash component."
            .clone_into(&mut failure.message);
    }
    failure
}
