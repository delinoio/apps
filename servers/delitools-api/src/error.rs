// SPDX-License-Identifier: FSL-1.1-ALv2
//! Typed DeliTools failures: a stable `ErrorReason`, the documented Connect code mapping, an
//! English message and the request ID. Messages never carry inputs, schemas, results, credentials
//! or database text.
use connectrpc::{ConnectError, ErrorCode};
use delino_protocol::delitools::v1::{ErrorDetail, ErrorReason};

/// Header carrying the UUID v7 request ID on every response, success or failure.
pub const REQUEST_ID_HEADER: &str = "X-Delitools-Request-Id";

pub type Result<T> = std::result::Result<T, Fault>;

#[derive(Debug)]
pub struct Fault {
    pub reason: ErrorReason,
    pub code: ErrorCode,
    pub message: String,
    /// Request ID reported instead of the current call's own, used when a duplicate call is told
    /// about the existing request that holds its idempotency key.
    pub request_id: Option<String>,
}

/// Connect code documented for each reason in the protocol contract.
pub fn code_for(reason: ErrorReason) -> ErrorCode {
    use ErrorReason::*;
    match reason {
        ERROR_REASON_INVALID_INPUT | ERROR_REASON_INVALID_SCHEMA | ERROR_REASON_INVALID_MODEL => {
            ErrorCode::InvalidArgument
        }
        ERROR_REASON_UNAUTHENTICATED => ErrorCode::Unauthenticated,
        ERROR_REASON_FORBIDDEN => ErrorCode::PermissionDenied,
        ERROR_REASON_BILLING_BLOCKED
        | ERROR_REASON_PRICE_UNAVAILABLE
        | ERROR_REASON_INPUT_CONFLICT
        | ERROR_REASON_EXTRACTION_IMPOSSIBLE => ErrorCode::FailedPrecondition,
        ERROR_REASON_LIMIT => ErrorCode::ResourceExhausted,
        ERROR_REASON_PROCESSING => ErrorCode::Aborted,
        ERROR_REASON_ALREADY_PROCESSED => ErrorCode::AlreadyExists,
        ERROR_REASON_UNKNOWN_OUTCOME => ErrorCode::Unknown,
        ERROR_REASON_INVALID_OUTPUT => ErrorCode::DataLoss,
        ERROR_REASON_PROVIDER => ErrorCode::Unavailable,
        ERROR_REASON_TIMEOUT => ErrorCode::DeadlineExceeded,
        ERROR_REASON_CANCELED => ErrorCode::Canceled,
        ERROR_REASON_UNSPECIFIED | ERROR_REASON_INTERNAL => ErrorCode::Internal,
    }
}

/// Bounded metric and log label for a reason.
pub fn reason_label(reason: ErrorReason) -> &'static str {
    use ErrorReason::*;
    match reason {
        ERROR_REASON_UNSPECIFIED => "unspecified",
        ERROR_REASON_INVALID_INPUT => "invalid_input",
        ERROR_REASON_INVALID_SCHEMA => "invalid_schema",
        ERROR_REASON_INVALID_MODEL => "invalid_model",
        ERROR_REASON_UNAUTHENTICATED => "unauthenticated",
        ERROR_REASON_FORBIDDEN => "forbidden",
        ERROR_REASON_BILLING_BLOCKED => "billing_blocked",
        ERROR_REASON_LIMIT => "limit",
        ERROR_REASON_PROCESSING => "processing",
        ERROR_REASON_ALREADY_PROCESSED => "already_processed",
        ERROR_REASON_INPUT_CONFLICT => "input_conflict",
        ERROR_REASON_UNKNOWN_OUTCOME => "unknown_outcome",
        ERROR_REASON_EXTRACTION_IMPOSSIBLE => "extraction_impossible",
        ERROR_REASON_INVALID_OUTPUT => "invalid_output",
        ERROR_REASON_PROVIDER => "provider",
        ERROR_REASON_TIMEOUT => "timeout",
        ERROR_REASON_CANCELED => "canceled",
        ERROR_REASON_PRICE_UNAVAILABLE => "price_unavailable",
        ERROR_REASON_INTERNAL => "internal",
    }
}

impl Fault {
    pub fn new(reason: ErrorReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            code: code_for(reason),
            message: message.into(),
            request_id: None,
        }
    }
    /// Report an existing request's ID in the error detail and header.
    pub fn with_request_id(mut self, request_id: String) -> Self {
        self.request_id = Some(request_id);
        self
    }
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorReason::ERROR_REASON_INVALID_INPUT, message)
    }
    pub fn invalid_schema(message: impl Into<String>) -> Self {
        Self::new(ErrorReason::ERROR_REASON_INVALID_SCHEMA, message)
    }
    pub fn invalid_model(message: impl Into<String>) -> Self {
        Self::new(ErrorReason::ERROR_REASON_INVALID_MODEL, message)
    }
    pub fn unauthenticated() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_UNAUTHENTICATED,
            "authentication failed",
        )
    }
    pub fn forbidden() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_FORBIDDEN,
            "operation is not permitted",
        )
    }
    pub fn billing_blocked() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_BILLING_BLOCKED,
            "billing prevents new paid execution for this tenant",
        )
    }
    pub fn limit(message: impl Into<String>) -> Self {
        Self::new(ErrorReason::ERROR_REASON_LIMIT, message)
    }
    pub fn price_unavailable() -> Self {
        Self::new(
            ErrorReason::ERROR_REASON_PRICE_UNAVAILABLE,
            "no synchronized catalog price is available for the model",
        )
    }
    pub fn internal() -> Self {
        Self::new(ErrorReason::ERROR_REASON_INTERNAL, "internal error")
    }
    /// A required dependency (tenant authority, billing check) could not answer. New paid work
    /// fails closed. The reason stays `INTERNAL`; the Connect code is `unavailable` so clients can
    /// retry. The protocol contract documents this single deliberate deviation from the table.
    pub fn dependency_unavailable() -> Self {
        Self {
            reason: ErrorReason::ERROR_REASON_INTERNAL,
            code: ErrorCode::Unavailable,
            message: "a required dependency is unavailable".into(),
            request_id: None,
        }
    }
    /// An operator target (request record) does not exist or has expired. The reason is
    /// `INVALID_INPUT` because the protocol has no not-found reason; the code is `not_found`.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            reason: ErrorReason::ERROR_REASON_INVALID_INPUT,
            code: ErrorCode::NotFound,
            message: message.into(),
            request_id: None,
        }
    }
    /// An RPC whose implementation is delivered by a later layer.
    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self {
            reason: ErrorReason::ERROR_REASON_UNSPECIFIED,
            code: ErrorCode::Unimplemented,
            message: message.into(),
            request_id: None,
        }
    }
    /// Convert to a Connect error with the typed detail and the request ID header.
    pub fn into_connect(self, request_id: &str) -> ConnectError {
        let request_id = self.request_id.as_deref().unwrap_or(request_id);
        let detail = ErrorDetail {
            reason: self.reason.into(),
            request_id: request_id.to_owned(),
            ..Default::default()
        };
        let mut headers = axum::http::HeaderMap::new();
        if let Ok(value) = request_id.parse() {
            headers.insert(REQUEST_ID_HEADER, value);
        }
        ConnectError::new(self.code, self.message)
            .with_headers(headers)
            .with_detail(connectrpc::ErrorDetail::from_message(
                "delitools.v1.ErrorDetail",
                &detail,
            ))
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Fault {}

impl From<sqlx::Error> for Fault {
    fn from(error: sqlx::Error) -> Self {
        // Database messages can contain customer values; retain only the SQLSTATE.
        let sqlstate = match &error {
            sqlx::Error::Database(db) => db.code().map(|code| code.into_owned()),
            _ => None,
        };
        tracing::error!(
            category = "persistence",
            sqlstate = sqlstate.as_deref(),
            "database operation failed"
        );
        Self::internal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ErrorReason::*;
    use buffa::Enumeration;

    #[test]
    fn every_reason_maps_to_the_documented_connect_code() {
        let table = [
            (ERROR_REASON_INVALID_INPUT, ErrorCode::InvalidArgument),
            (ERROR_REASON_INVALID_SCHEMA, ErrorCode::InvalidArgument),
            (ERROR_REASON_INVALID_MODEL, ErrorCode::InvalidArgument),
            (ERROR_REASON_UNAUTHENTICATED, ErrorCode::Unauthenticated),
            (ERROR_REASON_FORBIDDEN, ErrorCode::PermissionDenied),
            (ERROR_REASON_BILLING_BLOCKED, ErrorCode::FailedPrecondition),
            (ERROR_REASON_LIMIT, ErrorCode::ResourceExhausted),
            (ERROR_REASON_PROCESSING, ErrorCode::Aborted),
            (ERROR_REASON_ALREADY_PROCESSED, ErrorCode::AlreadyExists),
            (ERROR_REASON_INPUT_CONFLICT, ErrorCode::FailedPrecondition),
            (ERROR_REASON_UNKNOWN_OUTCOME, ErrorCode::Unknown),
            (
                ERROR_REASON_EXTRACTION_IMPOSSIBLE,
                ErrorCode::FailedPrecondition,
            ),
            (ERROR_REASON_INVALID_OUTPUT, ErrorCode::DataLoss),
            (ERROR_REASON_PROVIDER, ErrorCode::Unavailable),
            (ERROR_REASON_TIMEOUT, ErrorCode::DeadlineExceeded),
            (ERROR_REASON_CANCELED, ErrorCode::Canceled),
            (
                ERROR_REASON_PRICE_UNAVAILABLE,
                ErrorCode::FailedPrecondition,
            ),
            (ERROR_REASON_INTERNAL, ErrorCode::Internal),
        ];
        for (reason, code) in table {
            assert_eq!(code_for(reason), code, "{reason:?}");
            assert_eq!(Fault::new(reason, "x").code, code);
            assert!(!reason_label(reason).is_empty());
        }
    }

    #[test]
    fn labels_are_unique_and_bounded() {
        let mut seen = std::collections::HashSet::new();
        for raw in 0..=18 {
            let reason = ErrorReason::from_i32(raw).expect("known reason");
            assert!(seen.insert(reason_label(reason)));
        }
        assert_eq!(seen.len(), 19);
    }

    #[test]
    fn connect_error_carries_typed_detail_and_request_header() {
        let id = server_common::id();
        let error = Fault::forbidden().into_connect(&id);
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert_eq!(error.details.len(), 1);
        assert_eq!(
            error
                .response_headers()
                .get(REQUEST_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            id
        );
    }

    #[test]
    fn documented_deviations_keep_their_reason() {
        let unavailable = Fault::dependency_unavailable();
        assert_eq!(unavailable.code, ErrorCode::Unavailable);
        assert_eq!(unavailable.reason, ERROR_REASON_INTERNAL);
        let missing = Fault::not_found("request not found");
        assert_eq!(missing.code, ErrorCode::NotFound);
        let stub = Fault::not_implemented("not available");
        assert_eq!(stub.code, ErrorCode::Unimplemented);
        assert_eq!(stub.reason, ERROR_REASON_UNSPECIFIED);
    }
}
