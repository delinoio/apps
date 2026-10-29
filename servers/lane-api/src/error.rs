// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::Fault;
use buffa::Enumeration;
use connectrpc::{ConnectError, ErrorCode};
use delino_protocol::lane::v1::{ConstraintViolation, ErrorDetail, ErrorReason};

pub fn reason(f: &Fault) -> ErrorReason {
    // Shared dependency helpers have their own reason numbers. Codes define their Lane mapping.
    match f.code {
        ErrorCode::Unauthenticated => ErrorReason::Unauthenticated,
        ErrorCode::PermissionDenied => ErrorReason::Forbidden,
        ErrorCode::Unavailable => ErrorReason::DependencyUnavailable,
        ErrorCode::NotFound => ErrorReason::NotFound,
        ErrorCode::AlreadyExists => ErrorReason::RequestIdReused,
        // A constraint violation packs its code above the reason bits (see planning::violation).
        _ => ErrorReason::from_i32(f.reason & 0xff).unwrap_or(ErrorReason::InvalidInput),
    }
}
pub fn detail(f: &Fault, correlation: &str, entity: &str, heads: Vec<String>) -> ErrorDetail {
    ErrorDetail {
        reason: reason(f).into(),
        correlation_id: correlation.into(),
        entity_id: entity.into(),
        current_head_ids: heads,
        retryable: matches!(
            f.code,
            ErrorCode::Unavailable | ErrorCode::ResourceExhausted
        ),
        violation: crate::planning::violation_of(f)
            .and_then(|v| ConstraintViolation::from_i32(v as i32))
            .unwrap_or(ConstraintViolation::Unspecified)
            .into(),
        ..Default::default()
    }
}
pub fn connect(f: Fault, correlation: &str, entity: &str, heads: Vec<String>) -> ConnectError {
    let detail = detail(&f, correlation, entity, heads);
    let mut headers = axum::http::HeaderMap::new();
    if let Ok(id) = correlation.parse() {
        headers.insert("x-request-id", id);
    }
    f.connect()
        .with_detail(connectrpc::ErrorDetail::from_message(
            "lane.v1.ErrorDetail",
            &detail,
        ))
        .with_headers(headers)
}
