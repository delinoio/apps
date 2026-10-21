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
pub fn canonical_id(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|v| v.get_version_num() == 7 && v.to_string() == s)
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
