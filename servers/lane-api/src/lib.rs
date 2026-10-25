// SPDX-License-Identifier: FSL-1.1-ALv2
pub use server_common::{Fault, Result, canonical_id, id, now, string};
use serde_json::Value;
use sqlx::PgConnection;

/// Identity established by the authenticated adapter, never by a request body.
#[derive(Clone, Debug)]
pub struct Principal {
    pub account_id: String,
    pub tenant_id: String,
    pub session_id: String,
    pub device_id: String,
    pub device_name: String,
    pub origin_kind: String,
}
impl Principal {
    pub fn origin(&self) -> Value {
        serde_json::json!({"kind": self.origin_kind, "accountId": self.account_id,
            "deviceId": self.device_id, "deviceName": self.device_name})
    }
}
pub fn hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}
pub async fn account_lock(db: &mut PgConnection, account: &str) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(account).execute(db).await?;
    Ok(())
}
