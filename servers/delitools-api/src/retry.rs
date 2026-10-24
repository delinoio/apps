// SPDX-License-Identifier: FSL-1.1-ALv2
//! Retry policy for transient Gateway failures.
//!
//! Only network failures, HTTP 5xx and HTTP 429 from the Gateway are transient. Invalid or
//! nonconforming model output is never retried to repair it, and a permanent provider rejection
//! (any other 4xx) is final. The caller re-checks authorization before every retry.

use crate::config::RetryPolicy;
use std::time::{Duration, SystemTime};

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

/// Parses a `Retry-After` header value (RFC 9110 section 10.2.3) into a wait hint.
///
/// A value of ASCII digits is delay-seconds. Anything else is parsed as an HTTP-date (IMF-fixdate,
/// obsolete RFC 850 or asctime) and becomes the time remaining until that date relative to `now`;
/// a date that is not in the future is a zero wait. Absent, malformed, negative, fractional or
/// overflowing values carry no hint. `now` is explicit so callers and tests control the clock.
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return value.parse::<u64>().ok().map(Duration::from_secs);
    }
    let date = httpdate::parse_http_date(value).ok()?;
    Some(date.duration_since(now).unwrap_or(Duration::ZERO))
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

    fn fixed_now() -> SystemTime {
        httpdate::parse_http_date("Sun, 06 Nov 1994 08:49:00 GMT").unwrap()
    }

    #[test]
    fn retry_after_numeric_seconds_are_unchanged() {
        let now = fixed_now();
        for (value, expected) in [
            ("0", Some(Duration::ZERO)),
            ("1", Some(Duration::from_secs(1))),
            ("120", Some(Duration::from_secs(120))),
            ("  7\t", Some(Duration::from_secs(7))),
            ("007", Some(Duration::from_secs(7))),
            ("18446744073709551615", Some(Duration::from_secs(u64::MAX))),
            ("18446744073709551616", None),
            ("99999999999999999999999999", None),
            ("-1", None),
            ("-0", None),
            ("+5", None),
            ("1.5", None),
            ("1e3", None),
            ("", None),
            ("   ", None),
            ("soon", None),
        ] {
            assert_eq!(parse_retry_after(value, now), expected, "{value:?}");
        }
    }

    #[test]
    fn retry_after_http_dates_are_relative_to_now() {
        let now = fixed_now();
        let cases = [
            // IMF-fixdate: 37 seconds, 1 second, equal and past.
            (
                "Sun, 06 Nov 1994 08:49:37 GMT",
                Some(Duration::from_secs(37)),
            ),
            (
                "Sun, 06 Nov 1994 08:49:01 GMT",
                Some(Duration::from_secs(1)),
            ),
            ("Sun, 06 Nov 1994 08:49:00 GMT", Some(Duration::ZERO)),
            ("Sun, 06 Nov 1994 08:48:59 GMT", Some(Duration::ZERO)),
            ("Fri, 01 Jan 1971 00:00:00 GMT", Some(Duration::ZERO)),
            // Obsolete RFC 850 and asctime forms.
            (
                "Sunday, 06-Nov-94 08:49:37 GMT",
                Some(Duration::from_secs(37)),
            ),
            ("Sun Nov  6 08:49:37 1994", Some(Duration::from_secs(37))),
            // Padding is ignored.
            (
                "  Sun, 06 Nov 1994 08:49:37 GMT \t",
                Some(Duration::from_secs(37)),
            ),
            // Malformed dates.
            ("Sun, 06 Nov 1994 08:49:37", None),
            ("Sun, 32 Nov 1994 08:49:37 GMT", None),
            ("1994-11-06T08:49:37Z", None),
            ("Sun, 06 Nov 1994", None),
        ];
        for (value, expected) in cases {
            assert_eq!(parse_retry_after(value, now), expected, "{value:?}");
        }
    }

    #[test]
    fn retry_after_distant_future_date_does_not_overflow() {
        let hint = parse_retry_after("Fri, 31 Dec 9999 23:59:59 GMT", fixed_now());
        assert!(hint.is_some_and(|h| h > Duration::from_secs(60 * 60 * 24 * 365 * 1000)));
    }

    #[test]
    fn date_hints_feed_the_backoff_ceiling_and_jitter() {
        let p = policy();
        let now = fixed_now();
        let far = parse_retry_after("Fri, 31 Dec 9999 23:59:59 GMT", now);
        assert_eq!(p.ceiling(1, far), p.max_delay);
        assert_eq!(p.delay(1, far, 0.0), p.max_delay / 2);
        assert_eq!(p.delay(1, far, 1.0), p.max_delay);
        // A date one second away exceeds the 500ms cap, so it raises the ceiling only to the cap.
        let near = parse_retry_after("Sun, 06 Nov 1994 08:49:01 GMT", now);
        assert_eq!(p.ceiling(3, near), Duration::from_millis(500));
        // A date equal to now is a zero hint and raises nothing.
        let equal = parse_retry_after("Sun, 06 Nov 1994 08:49:00 GMT", now);
        assert_eq!(p.ceiling(3, equal), Duration::from_millis(400));
        // A past date is a zero hint and leaves ordinary backoff.
        let past = parse_retry_after("Sun, 06 Nov 1994 08:48:00 GMT", now);
        assert_eq!(p.ceiling(2, past), Duration::from_millis(200));
        assert_eq!(
            classify_status(503, far),
            Failure::Transient { retry_after: far }
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
