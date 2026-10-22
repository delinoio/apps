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

    // Config::load reads process-global environment variables, so tests that mutate them run
    // one at a time. Other test modules in this crate do not call Config::load.
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const DURATION_KEYS: [&str; 3] = ["DEPENDENCY_TIMEOUT", "REQUEST_TIMEOUT", "CLEANUP_INTERVAL"];

    fn load_with(settings: &[(&str, &str)]) -> anyhow::Result<Config> {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: ENV_LOCK serializes every test that touches these variables.
        unsafe {
            for key in DURATION_KEYS {
                std::env::remove_var(key);
            }
            for (key, value) in settings {
                std::env::set_var(key, value);
            }
        }
        let result = Config::load();
        // SAFETY: still holding ENV_LOCK.
        unsafe {
            for key in DURATION_KEYS {
                std::env::remove_var(key);
            }
        }
        result
    }

    #[test]
    fn rejects_cleanup_interval_that_rounds_to_zero() {
        for value in ["0.1ns", "0.49ns", "0.0000000001s", "0ns"] {
            let err = load_with(&[("CLEANUP_INTERVAL", value)])
                .err()
                .expect(value);
            assert!(
                err.to_string().contains("CLEANUP_INTERVAL"),
                "{value}: {err}"
            );
        }
    }

    #[test]
    fn accepts_valid_cleanup_interval_and_default() {
        let c = load_with(&[("CLEANUP_INTERVAL", "1ns")]).unwrap();
        assert_eq!(c.cleanup_interval, Duration::from_nanos(1));
        let c = load_with(&[("CLEANUP_INTERVAL", "1m30s")]).unwrap();
        assert_eq!(c.cleanup_interval, Duration::from_secs(90));
        let c = load_with(&[]).unwrap();
        assert_eq!(c.cleanup_interval, Duration::from_secs(300));
        let c = load_with(&[("CLEANUP_INTERVAL", "")]).unwrap();
        assert_eq!(c.cleanup_interval, Duration::from_secs(300));
    }

    #[test]
    fn rejects_zero_rounding_timeouts() {
        for key in ["DEPENDENCY_TIMEOUT", "REQUEST_TIMEOUT"] {
            let err = load_with(&[(key, "0.1ns")]).err().expect(key);
            assert!(err.to_string().contains(key), "{key}: {err}");
        }
    }

    #[test]
    fn enforces_timeout_ordering() {
        let err = load_with(&[("DEPENDENCY_TIMEOUT", "11s"), ("REQUEST_TIMEOUT", "10s")])
            .err()
            .expect("ordering error");
        assert!(
            err.to_string()
                .contains("dependency timeout must not exceed")
        );
        let c = load_with(&[("DEPENDENCY_TIMEOUT", "3s"), ("REQUEST_TIMEOUT", "10s")]).unwrap();
        assert_eq!(c.dependency_timeout, Duration::from_secs(3));
        assert_eq!(c.request_timeout, Duration::from_secs(10));
    }
}
