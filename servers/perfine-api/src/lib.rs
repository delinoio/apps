// SPDX-License-Identifier: FSL-1.1-ALv2
pub mod auth;
pub mod evidence;
pub mod github;
pub mod rpc;
pub mod storage;
use server_common::authority::Authority;
use sqlx::PgPool;
pub struct Server {
    pub db: PgPool,
    pub authority: Authority,
    pub actions: auth::Actions,
    pub github: github::GitHub,
    pub storage: storage::R2,
    pub quota: i64,
    pub upload_limit: u64,
    pub rate: i64,
}
pub use server_common::{Fault, Result, string};
pub fn id(s: &str) -> Result<uuid::Uuid> {
    let id = uuid::Uuid::parse_str(s).map_err(|_| Fault::invalid("UUID v7 required"))?;
    if id.get_version_num() != 7
        || id.get_variant() != uuid::Variant::RFC4122
        || id.to_string() != s
    {
        return Err(Fault::invalid("UUID v7 required"));
    }
    Ok(id)
}
pub fn stamp() -> String {
    chrono::Utc::now().to_rfc3339()
}
pub fn hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

pub fn integer(v: &serde_json::Value, k: &str) -> u64 {
    v[k].as_u64()
        .or_else(|| v[k].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}
