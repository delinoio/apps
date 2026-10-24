// SPDX-License-Identifier: FSL-1.1-ALv2
pub mod authority;
pub mod config;
pub mod error;
pub mod limit;
use chrono::{DateTime, Utc};
pub use error::{Fault, Result};
use serde_json::Value;

pub fn id() -> String {
    uuid::Uuid::now_v7().to_string()
}
pub fn now() -> DateTime<Utc> {
    DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap()
}
pub fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
pub fn integer(v: &Value, key: &str) -> i64 {
    v[key]
        .as_i64()
        .or_else(|| v[key].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}
/// Reports whether `s` is a canonical lowercase hyphenated UUID v7 as defined by RFC 9562: version
/// nibble 7 and the RFC 4122 variant (fourth-group leading nibble 8 to b). The variant is read
/// separately from the version by the `uuid` crate, so both are checked explicitly.
pub fn canonical_id(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|v| is_uuid_v7(&v) && v.to_string() == s)
}
/// Reports whether a parsed UUID has version 7 and the RFC 4122 variant.
pub fn is_uuid_v7(v: &uuid::Uuid) -> bool {
    v.get_version_num() == 7 && v.get_variant() == uuid::Variant::RFC4122
}
pub fn uuid(s: &str) -> Result<uuid::Uuid> {
    uuid::Uuid::parse_str(s).map_err(|_| Fault::invalid("invalid UUID"))
}
pub fn logger() -> anyhow::Result<()> {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    match std::env::var("LOG_FORMAT").unwrap_or_default().as_str() {
        "json" => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .with_writer(std::io::stderr)
            .init(),
        "" | "text" => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(std::env::var("NO_COLOR").unwrap_or_default().is_empty())
            .with_writer(std::io::stderr)
            .init(),
        _ => anyhow::bail!("LOG_FORMAT must be text or json"),
    }
    Ok(())
}
pub async fn shutdown() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=term.recv()=>{} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
pub mod http;
pub mod telemetry;

pub mod transport;

#[cfg(test)]
mod canonical_id_tests {
    use super::*;

    #[test]
    fn accepts_only_the_rfc_variant_nibbles_for_version_7() {
        for n in 0..16u8 {
            let id = format!("01900000-0000-7000-{n:x}000-000000000001");
            assert_eq!(canonical_id(&id), (8..12).contains(&n), "{id}");
            let parsed = uuid::Uuid::parse_str(&id).expect("synthetic id must parse");
            assert_eq!(is_uuid_v7(&parsed), (8..12).contains(&n), "{id}");
        }
    }

    #[test]
    fn keeps_version_case_and_format_controls() {
        assert!(canonical_id(&uuid::Uuid::now_v7().to_string()));
        // Version 4, uppercase and compact text stay rejected.
        assert!(!canonical_id("01900000-0000-4000-8000-000000000001"));
        assert!(!canonical_id("01900000-0000-7000-8000-00000000000A"));
        assert!(!canonical_id("0190000000007000800000000000001"));
        assert!(!canonical_id("019000000000700080000000000000010"));
        assert!(!canonical_id(""));
    }
}
