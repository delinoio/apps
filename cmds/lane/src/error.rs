// SPDX-License-Identifier: Apache-2.0
use serde::{Deserialize, Serialize};
/// Named planning constraint violations carried by `ERROR_REASON_CONSTRAINT_VIOLATION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Violation {
    #[serde(rename = "CONSTRAINT_VIOLATION_PARENT_CYCLE")]
    ParentCycle,
    #[serde(rename = "CONSTRAINT_VIOLATION_DEPENDENCY_CYCLE")]
    DependencyCycle,
    #[serde(rename = "CONSTRAINT_VIOLATION_DUPLICATE_CYCLE")]
    DuplicateCycle,
    #[serde(rename = "CONSTRAINT_VIOLATION_SELF_LINK")]
    SelfLink,
    #[serde(rename = "CONSTRAINT_VIOLATION_CROSS_REPOSITORY")]
    CrossRepository,
    #[serde(rename = "CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND")]
    ReferenceNotFound,
    #[serde(rename = "CONSTRAINT_VIOLATION_DUPLICATE_NAME")]
    DuplicateName,
    #[serde(rename = "CONSTRAINT_VIOLATION_LABEL_LIMIT")]
    LabelLimit,
    #[serde(rename = "CONSTRAINT_VIOLATION_DATE_ORDER")]
    DateOrder,
    #[serde(rename = "CONSTRAINT_VIOLATION_FIELD_RANGE")]
    FieldRange,
}
impl Violation {
    /// The protocol enum name, for example `CONSTRAINT_VIOLATION_PARENT_CYCLE`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParentCycle => "CONSTRAINT_VIOLATION_PARENT_CYCLE",
            Self::DependencyCycle => "CONSTRAINT_VIOLATION_DEPENDENCY_CYCLE",
            Self::DuplicateCycle => "CONSTRAINT_VIOLATION_DUPLICATE_CYCLE",
            Self::SelfLink => "CONSTRAINT_VIOLATION_SELF_LINK",
            Self::CrossRepository => "CONSTRAINT_VIOLATION_CROSS_REPOSITORY",
            Self::ReferenceNotFound => "CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND",
            Self::DuplicateName => "CONSTRAINT_VIOLATION_DUPLICATE_NAME",
            Self::LabelLimit => "CONSTRAINT_VIOLATION_LABEL_LIMIT",
            Self::DateOrder => "CONSTRAINT_VIOLATION_DATE_ORDER",
            Self::FieldRange => "CONSTRAINT_VIOLATION_FIELD_RANGE",
        }
    }
    /// Parse a protocol enum name; `UNSPECIFIED` and unknown names have no violation.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::ParentCycle,
            Self::DependencyCycle,
            Self::DuplicateCycle,
            Self::SelfLink,
            Self::CrossRepository,
            Self::ReferenceNotFound,
            Self::DuplicateName,
            Self::LabelLimit,
            Self::DateOrder,
            Self::FieldRange,
        ]
        .into_iter()
        .find(|v| v.as_str() == name)
    }
}
pub type Result<T> = std::result::Result<T, Failure>;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub reason: String,
    pub message: Box<str>,
    #[serde(default = "default_correlation")]
    pub correlation_id: Box<str>,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "str::is_empty")]
    pub entity_id: Box<str>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub current_head_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<Box<serde_json::Value>>,
    /// Named planning constraint, set only when the reason is `CONSTRAINT_VIOLATION`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub violation: Option<Violation>,
}
fn default_correlation() -> Box<str> {
    crate::protocol::id().into()
}
impl Failure {
    #[must_use]
    pub fn new(reason: &str, message: &str) -> Self {
        Self {
            reason: format!("ERROR_REASON_{reason}"),
            message: message.into(),
            correlation_id: crate::protocol::id().into(),
            retryable: reason == "STORAGE_BUSY",
            entity_id: "".into(),
            current_head_ids: vec![],
            conflict: None,
            violation: None,
        }
    }
    /// A planning constraint failure naming the violation (for example `ParentCycle`)
    /// and, when known, the offending entity.
    #[must_use]
    pub fn constraint(violation: Violation, entity_id: &str, message: &str) -> Self {
        let mut failure = Self::new("CONSTRAINT_VIOLATION", message);
        failure.violation = Some(violation);
        failure.entity_id = entity_id.into();
        failure
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}
impl From<rusqlite::Error> for Failure {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                Self::new("STORAGE_BUSY", "Storage is busy; retry the same request.")
            }
            Some(rusqlite::ErrorCode::DiskFull) => Self::new("STORAGE_FULL", "Storage is full."),
            _ => Self::new("DEPENDENCY_UNAVAILABLE", "Local storage operation failed."),
        }
    }
}
impl From<std::io::Error> for Failure {
    fn from(_: std::io::Error) -> Self {
        Self::new("DEPENDENCY_UNAVAILABLE", "Local I/O operation failed.")
    }
}
impl From<serde_json::Error> for Failure {
    fn from(_: serde_json::Error) -> Self {
        Self::new("INVALID_INPUT", "Invalid JSON input.")
    }
}
#[must_use]
pub fn invalid(message: &str) -> Failure {
    Failure::new("INVALID_INPUT", message)
}
