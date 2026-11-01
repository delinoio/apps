// SPDX-License-Identifier: Apache-2.0
//! Errors carried by the CLI, the local IPC service and the API client. Messages are short
//! English text without secrets, tokens, prompts or provider response bodies.
use crate::pb::{ErrorDetail, ErrorReason};
use std::time::Duration;
pub type Result<T> = std::result::Result<T, Failure>;
#[derive(Debug, Clone)]
pub struct Failure {
    pub reason: ErrorReason,
    pub message: String,
    pub correlation_id: String,
    pub retry_after: Option<Duration>,
    pub detail: Box<ErrorDetail>,
    /// The AnyDash API could not be reached (network, timeout or unreadable response).
    pub transport: bool,
}
#[must_use]
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}
impl Failure {
    /// A local failure with a new correlation ID, written to the local log.
    #[must_use]
    pub fn local(reason: ErrorReason, message: &str) -> Self {
        let correlation_id = new_id();
        tracing::warn!(
            event = "local_error",
            reason = reason.as_str_name(),
            correlation_id = %correlation_id,
            "{message}"
        );
        Self {
            reason,
            message: message.to_owned(),
            detail: Box::new(ErrorDetail {
                reason: reason.into(),
                correlation_id: correlation_id.clone(),
                ..ErrorDetail::default()
            }),
            correlation_id,
            retry_after: None,
            transport: false,
        }
    }
    #[must_use]
    pub fn invalid(field: &str, message: &str) -> Self {
        let mut failure = Self::local(ErrorReason::InvalidArgument, message);
        field.clone_into(&mut failure.detail.field);
        failure
    }
    #[must_use]
    pub fn internal(message: &str) -> Self {
        Self::local(ErrorReason::Internal, message)
    }
    #[must_use]
    pub fn rate_limited(retry_after: Duration, message: &str) -> Self {
        let mut failure = Self::local(ErrorReason::RateLimited, message);
        failure.retry_after = Some(retry_after);
        failure.detail.retry_after = Some(duration(retry_after));
        failure.detail.rate_limit_scope = crate::pb::RateLimitScope::ManualRefresh.into();
        failure
    }
    /// Build a failure from a decoded API or IPC error detail.
    #[must_use]
    pub fn from_detail(detail: ErrorDetail, message: &str, retry_after: Option<Duration>) -> Self {
        let reason = ErrorReason::try_from(detail.reason).unwrap_or(ErrorReason::Unspecified);
        let retry_after = retry_after.or_else(|| {
            detail
                .retry_after
                .as_ref()
                .and_then(|d| u64::try_from(d.seconds).ok())
                .map(Duration::from_secs)
        });
        Self {
            reason,
            message: message.to_owned(),
            correlation_id: detail.correlation_id.clone(),
            retry_after,
            detail: Box::new(detail),
            transport: false,
        }
    }
    /// The Connect code for this reason, following the protocol error table.
    #[must_use]
    pub const fn connect_code(&self) -> &'static str {
        match self.reason {
            ErrorReason::Unauthenticated
            | ErrorReason::RefreshTokenReused
            | ErrorReason::DeviceCredentialsInactive => "unauthenticated",
            ErrorReason::PermissionDenied
            | ErrorReason::SpaceInactive
            | ErrorReason::DataOwnerRequired => "permission_denied",
            ErrorReason::NotFound => "not_found",
            ErrorReason::InvalidArgument
            | ErrorReason::EnrollmentCodeMismatch
            | ErrorReason::PayloadTooLarge
            | ErrorReason::RequestIdReused
            | ErrorReason::ProviderCredentialRejected => "invalid_argument",
            ErrorReason::VersionConflict | ErrorReason::KeyRotationFailed => "aborted",
            ErrorReason::RateLimited | ErrorReason::QuotaExceeded => "resource_exhausted",
            ErrorReason::UpdateRequired
            | ErrorReason::LeaseExpired
            | ErrorReason::EnrollmentExpired
            | ErrorReason::EnrollmentInvalidated
            | ErrorReason::EnrollmentAlreadyUsed
            | ErrorReason::AccountNotConnected
            | ErrorReason::OnboardingRequired
            | ErrorReason::ConnectionAccountMismatch
            | ErrorReason::AlreadyLoggedIn
            | ErrorReason::NotLoggedIn
            | ErrorReason::KeyUnavailable
            | ErrorReason::ServiceManaged => "failed_precondition",
            ErrorReason::DependencyUnavailable => "unavailable",
            ErrorReason::Internal | ErrorReason::Unspecified => "internal",
        }
    }
    /// The JSON error object of `--json` output.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let mut error = serde_json::json!({
            "reason": self.reason.as_str_name(),
            "message": self.message,
            "correlationId": self.correlation_id,
        });
        if let Some(retry) = self.retry_after {
            error["retryAfterSeconds"] = retry.as_secs().max(1).into();
        }
        error
    }
}
#[must_use]
pub fn duration(d: Duration) -> prost_types::Duration {
    prost_types::Duration {
        seconds: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        nanos: i32::try_from(d.subsec_nanos()).unwrap_or(0),
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}
impl From<rusqlite::Error> for Failure {
    fn from(_: rusqlite::Error) -> Self {
        Self::internal("Local storage operation failed.")
    }
}
impl From<std::io::Error> for Failure {
    fn from(_: std::io::Error) -> Self {
        Self::internal("Local I/O operation failed.")
    }
}
