// SPDX-License-Identifier: FSL-1.1-ALv2
//! Typed API errors and their `ErrorDetail` mapping.
use connectrpc::{ConnectError, ErrorCode};
use delino_protocol::anydash::v1::{
    ClientKind, ErrorDetail, ErrorReason, QuotaInfo, QuotaKind, RateLimitScope,
};
use std::time::Duration;

pub type Result<T> = std::result::Result<T, ApiError>;

/// An API failure. Every failure leaves the server as a Connect error with exactly one
/// `anydash.v1.ErrorDetail` carrying the request correlation ID.
#[derive(Clone, Debug)]
pub struct ApiError {
    pub reason: ErrorReason,
    pub message: String,
    pub detail: Box<Detail>,
}

/// Reason-specific `ErrorDetail` fields.
#[derive(Clone, Debug, Default)]
pub struct Detail {
    pub field: String,
    pub retry_after: Option<Duration>,
    pub scope: Option<RateLimitScope>,
    pub quota: Option<(QuotaKind, u32, u32)>,
    pub minimum_client_version: String,
    pub client_kind: Option<ClientKind>,
    pub current_version: u64,
    pub current_lease_epoch: u64,
}

impl ApiError {
    pub fn new(reason: ErrorReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
            detail: Box::default(),
        }
    }
    pub fn field(mut self, field: impl Into<String>) -> Self {
        self.detail.field = field.into();
        self
    }
    pub fn unauthenticated() -> Self {
        Self::new(ErrorReason::Unauthenticated, "authentication failed")
    }
    pub fn denied() -> Self {
        Self::new(ErrorReason::PermissionDenied, "permission denied")
    }
    pub fn not_found(field: &str) -> Self {
        Self::new(ErrorReason::NotFound, "not found").field(field)
    }
    pub fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::new(ErrorReason::InvalidArgument, message).field(field)
    }
    pub fn version_conflict(current: u64) -> Self {
        let mut e = Self::new(
            ErrorReason::VersionConflict,
            "the dashboard changed; load the latest version and edit again",
        );
        e.detail.current_version = current;
        e
    }
    pub fn rate_limited(scope: RateLimitScope, retry: Duration) -> Self {
        let mut e = Self::new(ErrorReason::RateLimited, "rate limit exceeded");
        e.detail.scope = Some(scope);
        e.detail.retry_after = Some(retry);
        e
    }
    pub fn quota(kind: QuotaKind, limit: u32, used: u32) -> Self {
        let mut e = Self::new(ErrorReason::QuotaExceeded, "quota exceeded");
        e.detail.quota = Some((kind, limit, used));
        e
    }
    pub fn update_required(minimum: &str, kind: ClientKind) -> Self {
        let mut e = Self::new(ErrorReason::UpdateRequired, "update required");
        e.detail.minimum_client_version = minimum.into();
        e.detail.client_kind = Some(kind);
        e
    }
    pub fn lease_expired(current: u64) -> Self {
        let mut e = Self::new(ErrorReason::LeaseExpired, "the lease is not valid");
        e.detail.current_lease_epoch = current;
        e
    }
    pub fn dependency() -> Self {
        Self::new(
            ErrorReason::DependencyUnavailable,
            "a dependency is unavailable",
        )
    }
    pub fn internal() -> Self {
        Self::new(ErrorReason::Internal, "internal error")
    }
    pub fn space_inactive() -> Self {
        Self::new(ErrorReason::SpaceInactive, "the space is not active")
    }
    pub fn payload_too_large() -> Self {
        Self::new(
            ErrorReason::PayloadTooLarge,
            "the request is too large; split it",
        )
    }
    pub fn precondition(reason: ErrorReason, message: &str) -> Self {
        Self::new(reason, message)
    }

    /// The Connect code fixed by the protocol error table.
    pub fn code(&self) -> ErrorCode {
        match self.reason {
            ErrorReason::Unauthenticated
            | ErrorReason::RefreshTokenReused
            | ErrorReason::DeviceCredentialsInactive => ErrorCode::Unauthenticated,
            ErrorReason::PermissionDenied
            | ErrorReason::SpaceInactive
            | ErrorReason::DataOwnerRequired => ErrorCode::PermissionDenied,
            ErrorReason::NotFound => ErrorCode::NotFound,
            ErrorReason::InvalidArgument
            | ErrorReason::EnrollmentCodeMismatch
            | ErrorReason::PayloadTooLarge
            | ErrorReason::RequestIdReused
            | ErrorReason::ProviderCredentialRejected => ErrorCode::InvalidArgument,
            ErrorReason::VersionConflict | ErrorReason::KeyRotationFailed => ErrorCode::Aborted,
            ErrorReason::RateLimited | ErrorReason::QuotaExceeded => ErrorCode::ResourceExhausted,
            ErrorReason::DependencyUnavailable => ErrorCode::Unavailable,
            ErrorReason::Internal | ErrorReason::Unspecified => ErrorCode::Internal,
            _ => ErrorCode::FailedPrecondition,
        }
    }

    pub fn detail(&self, correlation: &str) -> ErrorDetail {
        ErrorDetail {
            reason: self.reason.into(),
            correlation_id: correlation.into(),
            retry_after: self.detail.retry_after.map(crate::time::duration).into(),
            rate_limit_scope: self
                .detail
                .scope
                .unwrap_or(RateLimitScope::Unspecified)
                .into(),
            quota: self
                .detail
                .quota
                .map(|(kind, limit, used)| QuotaInfo {
                    kind: kind.into(),
                    limit,
                    used,
                    ..Default::default()
                })
                .into(),
            minimum_client_version: self.detail.minimum_client_version.clone(),
            client_kind: self
                .detail
                .client_kind
                .unwrap_or(ClientKind::Unspecified)
                .into(),
            field: self.detail.field.clone(),
            current_version: self.detail.current_version,
            current_lease_epoch: self.detail.current_lease_epoch,
            ..Default::default()
        }
    }

    pub fn connect(&self, correlation: &str) -> ConnectError {
        let mut headers = axum::http::HeaderMap::new();
        if let Ok(value) = correlation.parse() {
            headers.insert(crate::CORRELATION_HEADER, value);
        }
        if let Some(retry) = self.detail.retry_after {
            let seconds = retry.as_secs() + u64::from(retry.subsec_nanos() > 0);
            headers.insert("retry-after", seconds.max(1).into());
        }
        ConnectError::new(self.code(), self.message.clone())
            .with_detail(connectrpc::ErrorDetail::from_message(
                "anydash.v1.ErrorDetail",
                &self.detail(correlation),
            ))
            .with_headers(headers)
    }

    /// A bounded label for logs.
    pub fn reason_name(&self) -> &'static str {
        use buffa::Enumeration;
        self.reason.proto_name()
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ApiError {}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::Database(ref d) => {
                tracing::error!(
                    category = "persistence",
                    code = d.code().as_deref().unwrap_or(""),
                    "database statement failed"
                );
                Self::internal()
            }
            _ => {
                tracing::error!(category = "persistence", "database unavailable");
                Self::dependency()
            }
        }
    }
}

impl From<server_common::Fault> for ApiError {
    fn from(f: server_common::Fault) -> Self {
        match f.code {
            ErrorCode::Unauthenticated => Self::unauthenticated(),
            ErrorCode::PermissionDenied => Self::denied(),
            ErrorCode::Unavailable | ErrorCode::DeadlineExceeded => Self::dependency(),
            _ => Self::internal(),
        }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(_: serde_json::Error) -> Self {
        tracing::error!(category = "serialization", "stored value is unreadable");
        Self::internal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_maps_to_the_protocol_code() {
        let cases = [
            (ErrorReason::Unauthenticated, ErrorCode::Unauthenticated),
            (ErrorReason::PermissionDenied, ErrorCode::PermissionDenied),
            (ErrorReason::NotFound, ErrorCode::NotFound),
            (ErrorReason::InvalidArgument, ErrorCode::InvalidArgument),
            (ErrorReason::VersionConflict, ErrorCode::Aborted),
            (ErrorReason::RateLimited, ErrorCode::ResourceExhausted),
            (ErrorReason::QuotaExceeded, ErrorCode::ResourceExhausted),
            (ErrorReason::UpdateRequired, ErrorCode::FailedPrecondition),
            (ErrorReason::LeaseExpired, ErrorCode::FailedPrecondition),
            (
                ErrorReason::EnrollmentExpired,
                ErrorCode::FailedPrecondition,
            ),
            (
                ErrorReason::EnrollmentInvalidated,
                ErrorCode::FailedPrecondition,
            ),
            (
                ErrorReason::EnrollmentCodeMismatch,
                ErrorCode::InvalidArgument,
            ),
            (
                ErrorReason::EnrollmentAlreadyUsed,
                ErrorCode::FailedPrecondition,
            ),
            (ErrorReason::RefreshTokenReused, ErrorCode::Unauthenticated),
            (
                ErrorReason::DeviceCredentialsInactive,
                ErrorCode::Unauthenticated,
            ),
            (
                ErrorReason::AccountNotConnected,
                ErrorCode::FailedPrecondition,
            ),
            (ErrorReason::SpaceInactive, ErrorCode::PermissionDenied),
            (
                ErrorReason::OnboardingRequired,
                ErrorCode::FailedPrecondition,
            ),
            (ErrorReason::PayloadTooLarge, ErrorCode::InvalidArgument),
            (ErrorReason::DataOwnerRequired, ErrorCode::PermissionDenied),
            (
                ErrorReason::ConnectionAccountMismatch,
                ErrorCode::FailedPrecondition,
            ),
            (ErrorReason::DependencyUnavailable, ErrorCode::Unavailable),
            (ErrorReason::Internal, ErrorCode::Internal),
            (ErrorReason::RequestIdReused, ErrorCode::InvalidArgument),
        ];
        for (reason, code) in cases {
            assert_eq!(ApiError::new(reason, "x").code(), code, "{reason:?}");
        }
    }

    #[test]
    fn detail_carries_correlation_and_typed_fields() {
        let e = ApiError::rate_limited(RateLimitScope::UserToken, Duration::from_millis(1500));
        let d = e.detail("corr");
        assert_eq!(d.correlation_id, "corr");
        assert_eq!(d.rate_limit_scope, RateLimitScope::UserToken);
        assert_eq!(d.retry_after.as_option().unwrap().seconds, 1);
        let c = e.connect("corr");
        assert_eq!(c.response_headers()["retry-after"], "2");
        assert_eq!(c.response_headers()[crate::CORRELATION_HEADER], "corr");
        let q = ApiError::quota(QuotaKind::WidgetsPerDashboard, 100, 100).detail("c");
        assert_eq!(q.quota.as_option().unwrap().limit, 100);
        let v = ApiError::version_conflict(7).detail("c");
        assert_eq!(v.current_version, 7);
    }
}
