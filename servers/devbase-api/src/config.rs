// SPDX-License-Identifier: FSL-1.1-ALv2
use server_common::config::{duration, positive, valid_origin};
use std::time::Duration;
pub struct Config {
    pub projects: i64,
    pub users: i64,
    pub keys: i64,
    pub user_rate: f64,
    pub user_burst: u32,
    pub management_rate: f64,
    pub management_burst: u32,
    pub dependency_timeout: Duration,
    pub request_timeout: Duration,
    pub cleanup_interval: Duration,
    pub origin: String,
    pub listen: String,
}
impl Config {
    pub fn load() -> anyhow::Result<Self> {
        fn rate(key: &str, default: f64) -> anyhow::Result<f64> {
            let n = std::env::var(key)
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(default);
            anyhow::ensure!(
                n > 0.0 && n.is_finite(),
                "{key} must be positive and finite"
            );
            Ok(n)
        }
        let c = Self {
            projects: positive("MAX_PROJECTS_PER_TENANT", 100)?,
            users: positive("MAX_USERS_PER_PROJECT", 1_000_000)?,
            keys: positive("MAX_ACTIVE_KEYS_PER_PROJECT", 20)?,
            user_rate: rate("USER_RATE_PER_SECOND", 1000.0)?,
            user_burst: positive("USER_RATE_BURST", 2000)?.try_into()?,
            management_rate: rate("MANAGEMENT_RATE_PER_SECOND", 50.0)?,
            management_burst: positive("MANAGEMENT_RATE_BURST", 100)?.try_into()?,
            dependency_timeout: duration("DEPENDENCY_TIMEOUT", Duration::from_secs(3))?,
            request_timeout: duration("REQUEST_TIMEOUT", Duration::from_secs(10))?,
            cleanup_interval: duration("CLEANUP_INTERVAL", Duration::from_secs(300))?,
            origin: std::env::var("WEB_ORIGIN").unwrap_or_default(),
            listen: std::env::var("LISTEN")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "127.0.0.1:46317".into()),
        };
        anyhow::ensure!(
            c.user_burst as f64 >= c.user_rate && c.management_burst as f64 >= c.management_rate,
            "burst must not be smaller than sustained rate"
        );
        anyhow::ensure!(
            c.dependency_timeout <= c.request_timeout,
            "dependency timeout must not exceed request timeout"
        );
        anyhow::ensure!(
            c.origin.is_empty() || valid_origin(&c.origin),
            "WEB_ORIGIN must be one exact http or https origin"
        );
        Ok(c)
    }
}
