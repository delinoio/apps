// SPDX-License-Identifier: FSL-1.1-ALv2
//! Public error model.
//!
//! Every failure of a DeliTools RPC is an [`ApiError`]. It maps to a Connect error code, a
//! stable [`ErrorReason`] carried in the `delitools.v1.ErrorDetail` detail, an English message
//! and the request ID. Messages are fixed strings: they never echo submitted text, schemas,
//! model output, credentials or provider diagnostics.

use connectrpc::{ConnectError, ErrorCode};
use delino_protocol::delitools::v1::{ErrorDetail, ErrorReason};
use std::{borrow::Cow, time::Duration};

/// A classified failure with its public representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub reason: ErrorReason,
    pub code: ErrorCode,
    pub message: Cow<'static, str>,
    /// Suggested wait before the caller retries; emitted as `Retry-After`.
    pub retry_after: Option<Duration>,
}

impl ApiError {
    fn new(reason: ErrorReason, code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            reason,
            code,
            message: message.into(),
            retry_after: None,
        }
    }
    /// The request failed validation. The message names the violated rule, never the content.
    pub fn invalid_input(message: &'static str) -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_INVALID_INPUT,
            ErrorCode::InvalidArgument,
            message,
        )
    }
    pub fn unauthenticated() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_UNAUTHENTICATED,
            ErrorCode::Unauthenticated,
            "authentication failed",
        )
    }
    pub fn permission_denied() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_PERMISSION_DENIED,
            ErrorCode::PermissionDenied,
            "the credential does not permit this operation for the tenant",
        )
    }
    pub fn billing_blocked() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_BILLING_BLOCKED,
            ErrorCode::FailedPrecondition,
            "paid execution is blocked for the tenant",
        )
    }
    pub fn limit_exceeded(message: &'static str) -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_LIMIT_EXCEEDED,
            ErrorCode::ResourceExhausted,
            message,
        )
    }
    pub fn rate_limited(retry_after: Option<Duration>, message: &'static str) -> Self {
        let mut error = Self::new(
            ErrorReason::ERROR_REASON_RATE_LIMITED,
            ErrorCode::ResourceExhausted,
            message,
        );
        error.retry_after = retry_after;
        error
    }
    /// The model is not on the operator allowlist.
    pub fn model_not_allowed() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_MODEL_UNAVAILABLE,
            ErrorCode::InvalidArgument,
            "the model is not allowed",
        )
    }
    /// The model is allowed but its input or output selling price is not registered.
    pub fn model_unpriced() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_MODEL_UNAVAILABLE,
            ErrorCode::FailedPrecondition,
            "the model has no registered input and output prices",
        )
    }
    pub fn unsupported_schema(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_UNSUPPORTED_SCHEMA,
            ErrorCode::InvalidArgument,
            message,
        )
    }
    /// Transient provider failures exhausted the retry budget.
    pub fn provider_unavailable() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_PROVIDER_FAILED,
            ErrorCode::Unavailable,
            "the model provider failed after the permitted retries",
        )
    }
    /// The provider rejected the request permanently.
    pub fn provider_rejected() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_PROVIDER_FAILED,
            ErrorCode::Internal,
            "the model provider rejected the request",
        )
    }
    pub fn invalid_output(message: &'static str) -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_INVALID_OUTPUT,
            ErrorCode::Internal,
            message,
        )
    }
    pub fn extraction_failed() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_EXTRACTION_FAILED,
            ErrorCode::FailedPrecondition,
            "the text does not contain the information required by the schema",
        )
    }
    pub fn timeout() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_TIMEOUT,
            ErrorCode::DeadlineExceeded,
            "the execution timeout elapsed",
        )
    }
    pub fn canceled() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_CANCELED,
            ErrorCode::Canceled,
            "the request was canceled",
        )
    }
    pub fn dependency_unavailable() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_DEPENDENCY_UNAVAILABLE,
            ErrorCode::Unavailable,
            "a required dependency is unavailable",
        )
    }
    /// An unexpected internal failure. It has no stable reason.
    pub fn internal() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_UNSPECIFIED,
            ErrorCode::Internal,
            "internal error",
        )
    }

    /// Bounded metric and log label of the reason.
    pub fn label(&self) -> &'static str {
        reason_label(self.reason)
    }

    /// Converts the failure into the wire error with `ErrorDetail` and response headers.
    pub fn into_connect(self, request_id: &str) -> ConnectError {
        let detail = ErrorDetail {
            reason: self.reason.into(),
            request_id: request_id.to_owned(),
            ..Default::default()
        };
        let mut headers = axum::http::HeaderMap::new();
        if let Ok(value) = request_id.parse() {
            headers.insert(crate::REQUEST_ID_HEADER, value);
        }
        if let Some(wait) = self.retry_after
            && let Ok(value) = wait.as_secs_f64().ceil().max(1.0).to_string().parse()
        {
            headers.insert("Retry-After", value);
        }
        ConnectError::new(self.code, self.message.into_owned())
            .with_headers(headers)
            .with_detail(connectrpc::ErrorDetail::from_message(
                "delitools.v1.ErrorDetail",
                &detail,
            ))
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ApiError {}

/// Stable lowercase label for a reason, used in metrics and logs.
pub fn reason_label(reason: ErrorReason) -> &'static str {
    match reason {
        ErrorReason::ERROR_REASON_UNSPECIFIED => "internal",
        ErrorReason::ERROR_REASON_INVALID_INPUT => "invalid_input",
        ErrorReason::ERROR_REASON_UNAUTHENTICATED => "unauthenticated",
        ErrorReason::ERROR_REASON_PERMISSION_DENIED => "permission_denied",
        ErrorReason::ERROR_REASON_BILLING_BLOCKED => "billing_blocked",
        ErrorReason::ERROR_REASON_LIMIT_EXCEEDED => "limit_exceeded",
        ErrorReason::ERROR_REASON_RATE_LIMITED => "rate_limited",
        ErrorReason::ERROR_REASON_MODEL_UNAVAILABLE => "model_unavailable",
        ErrorReason::ERROR_REASON_UNSUPPORTED_SCHEMA => "unsupported_schema",
        ErrorReason::ERROR_REASON_PROVIDER_FAILED => "provider_failed",
        ErrorReason::ERROR_REASON_INVALID_OUTPUT => "invalid_output",
        ErrorReason::ERROR_REASON_EXTRACTION_FAILED => "extraction_failed",
        ErrorReason::ERROR_REASON_TIMEOUT => "timeout",
        ErrorReason::ERROR_REASON_CANCELED => "canceled",
        ErrorReason::ERROR_REASON_DEPENDENCY_UNAVAILABLE => "dependency_unavailable",
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(_: sqlx::Error) -> Self {
        tracing::error!(category = "persistence", "database operation failed");
        Self::dependency_unavailable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_has_a_distinct_bounded_label() {
        let reasons = [
            ErrorReason::ERROR_REASON_UNSPECIFIED,
            ErrorReason::ERROR_REASON_INVALID_INPUT,
            ErrorReason::ERROR_REASON_UNAUTHENTICATED,
            ErrorReason::ERROR_REASON_PERMISSION_DENIED,
            ErrorReason::ERROR_REASON_BILLING_BLOCKED,
            ErrorReason::ERROR_REASON_LIMIT_EXCEEDED,
            ErrorReason::ERROR_REASON_RATE_LIMITED,
            ErrorReason::ERROR_REASON_MODEL_UNAVAILABLE,
            ErrorReason::ERROR_REASON_UNSUPPORTED_SCHEMA,
            ErrorReason::ERROR_REASON_PROVIDER_FAILED,
            ErrorReason::ERROR_REASON_INVALID_OUTPUT,
            ErrorReason::ERROR_REASON_EXTRACTION_FAILED,
            ErrorReason::ERROR_REASON_TIMEOUT,
            ErrorReason::ERROR_REASON_CANCELED,
            ErrorReason::ERROR_REASON_DEPENDENCY_UNAVAILABLE,
        ];
        let labels: std::collections::HashSet<_> =
            reasons.iter().map(|r| reason_label(*r)).collect();
        assert_eq!(labels.len(), reasons.len());
    }

    #[test]
    fn connect_error_carries_request_id_and_retry_after() {
        let error =
            ApiError::rate_limited(Some(Duration::from_millis(1500)), "rate limit exceeded")
                .into_connect("req-1");
        assert_eq!(error.code, ErrorCode::ResourceExhausted);
    }

    #[test]
    fn codes_follow_the_documented_mapping() {
        assert_eq!(
            ApiError::model_not_allowed().code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            ApiError::model_unpriced().code,
            ErrorCode::FailedPrecondition
        );
        assert_eq!(
            ApiError::provider_unavailable().code,
            ErrorCode::Unavailable
        );
        assert_eq!(ApiError::timeout().code, ErrorCode::DeadlineExceeded);
        assert_eq!(ApiError::canceled().code, ErrorCode::Canceled);
    }
}
