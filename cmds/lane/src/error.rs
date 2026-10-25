// SPDX-License-Identifier: Apache-2.0
use serde::{Deserialize, Serialize};
pub type Result<T> = std::result::Result<T, Failure>;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub reason: String,
    pub message: String,
    #[serde(default = "crate::protocol::id")]
    pub correlation_id: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entity_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub current_head_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<serde_json::Value>,
}
impl Failure {
    pub fn new(reason: &str, message: &str) -> Self {
        Self {
            reason: format!("ERROR_REASON_{reason}"),
            message: message.into(),
            correlation_id: crate::protocol::id(),
            retryable: reason == "STORAGE_BUSY",
            entity_id: String::new(),
            current_head_ids: vec![],
            conflict: None,
        }
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
pub fn invalid(message: &str) -> Failure {
    Failure::new("INVALID_INPUT", message)
}
