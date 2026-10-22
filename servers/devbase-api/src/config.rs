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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Every environment variable that `Config::load` reads.
    const CONFIG_KEYS: &[&str] = &[
        "MAX_PROJECTS_PER_TENANT",
        "MAX_USERS_PER_PROJECT",
        "MAX_ACTIVE_KEYS_PER_PROJECT",
        "USER_RATE_PER_SECOND",
        "USER_RATE_BURST",
        "MANAGEMENT_RATE_PER_SECOND",
        "MANAGEMENT_RATE_BURST",
        "DEPENDENCY_TIMEOUT",
        "REQUEST_TIMEOUT",
        "CLEANUP_INTERVAL",
        "WEB_ORIGIN",
        "LISTEN",
    ];

    // The process environment is shared by all test threads, so tests that load configuration
    // serialize on this lock.
    static ENVIRONMENT: Mutex<()> = Mutex::new(());

    struct RestoreEnvironment(Vec<(&'static str, Option<String>)>);

    impl Drop for RestoreEnvironment {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                // SAFETY: callers hold the ENVIRONMENT lock, which serializes every test that
                // reads or writes these variables.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    /// Loads `Config` with only `vars` set among the variables it reads, then restores the
    /// previous process environment. Reuse this helper for any test that needs `Config::load`.
    fn load_with(vars: &[(&str, &str)]) -> anyhow::Result<Config> {
        let _lock = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = RestoreEnvironment(
            CONFIG_KEYS
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect(),
        );
        // SAFETY: the ENVIRONMENT lock is held; see RestoreEnvironment.
        unsafe {
            for key in CONFIG_KEYS {
                std::env::remove_var(key);
            }
            for (key, value) in vars {
                std::env::set_var(key, value);
            }
        }
        Config::load()
    }

    #[test]
    fn cleanup_interval_defaults_and_accepts_positive_values() {
        let config = load_with(&[]).unwrap();
        assert_eq!(config.cleanup_interval, Duration::from_secs(300));

        let config = load_with(&[("CLEANUP_INTERVAL", "")]).unwrap();
        assert_eq!(config.cleanup_interval, Duration::from_secs(300));

        let config = load_with(&[("CLEANUP_INTERVAL", "1ns")]).unwrap();
        assert_eq!(config.cleanup_interval, Duration::from_nanos(1));

        let config = load_with(&[("CLEANUP_INTERVAL", "0.5ns")]).unwrap();
        assert_eq!(config.cleanup_interval, Duration::from_nanos(1));

        let config = load_with(&[("CLEANUP_INTERVAL", "1m30s")]).unwrap();
        assert_eq!(config.cleanup_interval, Duration::from_secs(90));
    }

    #[test]
    fn cleanup_interval_rejects_values_that_round_to_zero() {
        for value in ["0.1ns", "0.49ns", "0.0000000001s", "0ns"] {
            let error = load_with(&[("CLEANUP_INTERVAL", value)])
                .err()
                .unwrap_or_else(|| panic!("{value} must be rejected"))
                .to_string();
            assert!(error.contains("CLEANUP_INTERVAL"), "{value}: {error}");
        }
    }

    #[test]
    fn cleanup_interval_rejects_malformed_values() {
        for value in ["-1ns", "garbage", "NaNs", "infs"] {
            assert!(
                load_with(&[("CLEANUP_INTERVAL", value)]).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn timeouts_keep_their_ordering_check() {
        let config =
            load_with(&[("DEPENDENCY_TIMEOUT", "3s"), ("REQUEST_TIMEOUT", "10s")]).unwrap();
        assert_eq!(config.dependency_timeout, Duration::from_secs(3));
        assert_eq!(config.request_timeout, Duration::from_secs(10));

        let result = load_with(&[("DEPENDENCY_TIMEOUT", "11s"), ("REQUEST_TIMEOUT", "10s")]);
        assert!(result.is_err());
    }
}
