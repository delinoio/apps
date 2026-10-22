// SPDX-License-Identifier: FSL-1.1-ALv2
//! Retry policy for transient Gateway failures.
//!
//! Only network failures, HTTP 5xx and HTTP 429 from the Gateway are transient. Invalid or
//! nonconforming model output is never retried to repair it, and a permanent provider rejection
//! (any other 4xx) is final. The caller re-checks authorization before every retry.

use crate::config::RetryPolicy;
use std::time::Duration;

/// Classification of one failed provider attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Network error, HTTP 5xx or HTTP 429. `retry_after` is the provider hint for 429.
    Transient { retry_after: Option<Duration> },
    /// The provider rejected the request; retrying cannot help.
    Permanent,
}

/// Classifies an HTTP status returned by the Gateway.
pub fn classify_status(status: u16, retry_after: Option<Duration>) -> Failure {
    if status == 429 || (500..=599).contains(&status) {
        Failure::Transient { retry_after }
    } else {
        Failure::Permanent
    }
}

impl RetryPolicy {
    /// Total attempts including the first.
    pub fn max_attempts(&self) -> u32 {
        self.max_retries + 1
    }

    /// Upper bound of the wait after `failed_attempts` failures (1-based):
    /// `base * 2^(n-1)` capped at `max_delay`, raised to the provider hint but never above the cap.
    pub fn ceiling(&self, failed_attempts: u32, hint: Option<Duration>) -> Duration {
        let exponent = failed_attempts.saturating_sub(1).min(20);
        let backoff = self
            .base_delay
            .saturating_mul(1u32 << exponent)
            .min(self.max_delay);
        hint.map_or(backoff, |h| backoff.max(h.min(self.max_delay)))
    }

    /// The wait with jitter in `[ceiling/2, ceiling]`. `unit` is a sample in `[0, 1]`.
    pub fn delay(&self, failed_attempts: u32, hint: Option<Duration>, unit: f64) -> Duration {
        let ceiling = self.ceiling(failed_attempts, hint);
        ceiling.mul_f64(0.5 + 0.5 * unit.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RetryPolicy {
        RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
        }
    }

    #[test]
    fn only_network_5xx_and_429_are_transient() {
        for status in [429, 500, 502, 503, 504, 599] {
            assert!(
                matches!(classify_status(status, None), Failure::Transient { .. }),
                "{status}"
            );
        }
        for status in [400, 401, 402, 403, 404, 408, 409, 413, 422] {
            assert_eq!(
                classify_status(status, None),
                Failure::Permanent,
                "{status}"
            );
        }
        assert_eq!(
            classify_status(429, Some(Duration::from_secs(2))),
            Failure::Transient {
                retry_after: Some(Duration::from_secs(2))
            }
        );
    }

    #[test]
    fn attempts_include_the_first_try() {
        assert_eq!(policy().max_attempts(), 4);
        assert_eq!(
            RetryPolicy {
                max_retries: 0,
                ..policy()
            }
            .max_attempts(),
            1
        );
    }

    #[test]
    fn backoff_doubles_and_is_capped() {
        let p = policy();
        assert_eq!(p.ceiling(1, None), Duration::from_millis(100));
        assert_eq!(p.ceiling(2, None), Duration::from_millis(200));
        assert_eq!(p.ceiling(3, None), Duration::from_millis(400));
        assert_eq!(p.ceiling(4, None), Duration::from_millis(500));
        assert_eq!(p.ceiling(1000, None), Duration::from_millis(500));
    }

    #[test]
    fn provider_hint_raises_but_never_exceeds_the_cap() {
        let p = policy();
        assert_eq!(
            p.ceiling(1, Some(Duration::from_millis(300))),
            Duration::from_millis(300)
        );
        assert_eq!(
            p.ceiling(1, Some(Duration::from_secs(60))),
            Duration::from_millis(500)
        );
        assert_eq!(
            p.ceiling(3, Some(Duration::from_millis(10))),
            Duration::from_millis(400)
        );
    }

    #[test]
    fn jitter_stays_within_half_to_full_ceiling() {
        let p = policy();
        assert_eq!(p.delay(2, None, 0.0), Duration::from_millis(100));
        assert_eq!(p.delay(2, None, 1.0), Duration::from_millis(200));
        assert_eq!(p.delay(2, None, 7.0), Duration::from_millis(200));
        assert_eq!(p.delay(2, None, -3.0), Duration::from_millis(100));
    }
}
