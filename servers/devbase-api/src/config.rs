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
            // The limiter reports at most 1/rate seconds as the retry delay. Reject rates whose
            // reciprocal does not fit a Duration or whose whole seconds do not fit the signed
            // 64-bit `ErrorDetail.retry_after.seconds` field.
            anyhow::ensure!(
                Duration::try_from_secs_f64(1.0 / n)
                    .is_ok_and(|retry| i64::try_from(retry.as_secs()).is_ok()),
                "{key} must have a representable retry delay"
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
    const ENV_KEYS: [&str; 7] = [
        "DEPENDENCY_TIMEOUT",
        "REQUEST_TIMEOUT",
        "CLEANUP_INTERVAL",
        "USER_RATE_PER_SECOND",
        "USER_RATE_BURST",
        "MANAGEMENT_RATE_PER_SECOND",
        "MANAGEMENT_RATE_BURST",
    ];
    const RATE_KEYS: [(&str, &str); 2] = [
        ("USER_RATE_PER_SECOND", "USER_RATE_BURST"),
        ("MANAGEMENT_RATE_PER_SECOND", "MANAGEMENT_RATE_BURST"),
    ];

    fn load_with(settings: &[(&str, &str)]) -> anyhow::Result<Config> {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: ENV_LOCK serializes every test that touches these variables.
        unsafe {
            for key in ENV_KEYS {
                std::env::remove_var(key);
            }
            for (key, value) in settings {
                std::env::set_var(key, value);
            }
        }
        let result = Config::load();
        // SAFETY: still holding ENV_LOCK.
        unsafe {
            for key in ENV_KEYS {
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

    fn rate_error(rate_key: &str, burst_key: &str, rate: &str) -> String {
        load_with(&[(rate_key, rate), (burst_key, "1")])
            .err()
            .unwrap_or_else(|| panic!("{rate_key}={rate}"))
            .to_string()
    }

    #[test]
    fn rejects_rates_without_representable_retry_delay() {
        let subnormal = format!("{:e}", f64::from_bits(1));
        for (rate_key, burst_key) in RATE_KEYS {
            for value in ["1e-30", subnormal.as_str()] {
                let err = rate_error(rate_key, burst_key, value);
                assert!(err.contains(rate_key), "{value}: {err}");
            }
        }
    }

    #[test]
    fn rate_acceptance_matches_duration_conversion_at_boundary() {
        let boundary = 2_f64.powi(-64);
        let neighbors = [
            f64::from_bits(boundary.to_bits() - 1),
            boundary,
            f64::from_bits(boundary.to_bits() + 1),
        ];
        for (rate_key, burst_key) in RATE_KEYS {
            for rate in neighbors {
                let accepted = Duration::try_from_secs_f64(1.0 / rate)
                    .is_ok_and(|retry| i64::try_from(retry.as_secs()).is_ok());
                let loaded = load_with(&[(rate_key, &format!("{rate:e}")), (burst_key, "1")]);
                assert_eq!(loaded.is_ok(), accepted, "{rate_key}={rate:e}");
            }
        }
    }

    #[test]
    fn rejects_retry_delay_beyond_signed_seconds_field() {
        // 1e-19 gives a retry of 1e19 seconds: it fits Duration (u64 seconds) but not the
        // signed 64-bit seconds of ErrorDetail.retry_after, so it must be rejected at startup.
        assert!(Duration::try_from_secs_f64(1.0 / 1e-19).is_ok());
        let boundary = 2_f64.powi(-63);
        let rejected = [
            "1e-19".to_string(),
            format!("{:e}", f64::from_bits(boundary.to_bits() - 1)),
            format!("{boundary:e}"),
        ];
        for (rate_key, burst_key) in RATE_KEYS {
            for value in &rejected {
                let err = rate_error(rate_key, burst_key, value);
                assert!(err.contains(rate_key), "{value}: {err}");
            }
            let accepted = f64::from_bits(boundary.to_bits() + 1);
            let c = load_with(&[(rate_key, &format!("{accepted:e}")), (burst_key, "1")]).unwrap();
            let rate = if rate_key == "USER_RATE_PER_SECOND" {
                c.user_rate
            } else {
                c.management_rate
            };
            let retry = Duration::try_from_secs_f64(1.0 / rate).unwrap();
            assert!(i64::try_from(retry.as_secs()).is_ok());
        }
    }

    #[test]
    fn accepts_defaults_and_small_representable_rates() {
        let c = load_with(&[]).unwrap();
        assert_eq!(c.user_rate, 1000.0);
        assert_eq!(c.management_rate, 50.0);
        for (rate_key, burst_key) in RATE_KEYS {
            for value in ["0.001", "1e-18"] {
                load_with(&[(rate_key, value), (burst_key, "1")]).expect(value);
            }
        }
    }

    #[test]
    fn keeps_existing_rate_validation() {
        for (rate_key, burst_key) in RATE_KEYS {
            for value in ["0", "-1", "inf", "NaN", "abc"] {
                load_with(&[(rate_key, value), (burst_key, "1")])
                    .err()
                    .unwrap_or_else(|| panic!("{rate_key}={value}"));
            }
            load_with(&[(rate_key, "1"), (burst_key, "0")])
                .err()
                .expect("zero burst");
            let err = rate_error(rate_key, burst_key, "2");
            assert!(err.contains("burst must not be smaller"), "{err}");
        }
    }
}
