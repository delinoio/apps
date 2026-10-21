// SPDX-License-Identifier: FSL-1.1-ALv2
use connectrpc::{ConnectError, ErrorCode};
pub type Result<T> = std::result::Result<T, Fault>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultSource {
    Application,
    Persistence,
}
#[derive(Debug)]
pub struct Fault {
    pub source: FaultSource,
    pub code: ErrorCode,
    pub message: String,
    pub reason: i32,
    pub retry: Option<std::time::Duration>,
}
impl Fault {
    pub fn new(code: ErrorCode, reason: i32, message: impl Into<String>) -> Self {
        Self {
            source: FaultSource::Application,
            code,
            reason,
            message: message.into(),
            retry: None,
        }
    }
    pub fn invalid(s: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, 1, s)
    }
    pub fn unauthenticated() -> Self {
        Self::new(ErrorCode::Unauthenticated, 2, "authentication failed")
    }
    pub fn denied() -> Self {
        Self::new(ErrorCode::PermissionDenied, 3, "permission denied")
    }
    pub fn missing() -> Self {
        Self::new(ErrorCode::NotFound, 4, "resource not found")
    }
    pub fn exists() -> Self {
        Self::new(ErrorCode::AlreadyExists, 5, "resource already exists")
    }
    pub fn stale() -> Self {
        Self::new(
            ErrorCode::Aborted,
            6,
            "version is stale; read the current version and retry",
        )
    }
    pub fn quota() -> Self {
        Self::new(ErrorCode::ResourceExhausted, 8, "resource quota exceeded")
    }
    pub fn unavailable() -> Self {
        Self::new(ErrorCode::Unavailable, 9, "a dependency is unavailable")
    }
    pub fn conflict() -> Self {
        Self::new(
            ErrorCode::AlreadyExists,
            10,
            "idempotency key was used with a different request",
        )
    }
    pub fn precondition(s: impl Into<String>) -> Self {
        Self::new(ErrorCode::FailedPrecondition, 0, s)
    }
    pub fn connect(self) -> ConnectError {
        ConnectError::new(self.code, self.message)
    }
}
impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Fault {}
impl From<sqlx::Error> for Fault {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::missing(),
            sqlx::Error::Database(ref d) if d.code().as_deref() == Some("23505") => Self::exists(),
            sqlx::Error::Database(ref d) if d.code().as_deref() == Some("23503") => Self::missing(),
            _ => {
                tracing::error!(category = "persistence", "database operation failed");
                let mut fault = Self::unavailable();
                fault.source = FaultSource::Persistence;
                fault
            }
        }
    }
}
impl From<serde_json::Error> for Fault {
    fn from(_: serde_json::Error) -> Self {
        Self::invalid("invalid message")
    }
}
impl From<reqwest::Error> for Fault {
    fn from(_: reqwest::Error) -> Self {
        Self::unavailable()
    }
}
