// SPDX-License-Identifier: FSL-1.1-ALv2
use anyhow::{Result, bail};
use std::{env, time::Duration};
pub fn required(key: &str) -> Result<String> {
    let v = env::var(key).unwrap_or_default();
    if v.is_empty() {
        bail!("{key} is required")
    }
    Ok(v)
}
pub fn positive(key: &str, default: i64) -> Result<i64> {
    let n = env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(default);
    if n <= 0 {
        bail!("{key} must be positive")
    }
    Ok(n)
}
pub fn duration(key: &str, default: Duration) -> Result<Duration> {
    let Some(s) = env::var(key).ok().filter(|v| !v.is_empty()) else {
        return Ok(default);
    };
    parse_duration(key, &s)
}
// Pure parser behind `duration()`; it reads no process environment so tests stay isolated.
fn parse_duration(key: &str, s: &str) -> Result<Duration> {
    let mut remaining = s;
    let mut seconds = 0.0;
    while !remaining.is_empty() {
        let n = remaining
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .ok_or_else(|| anyhow::anyhow!("{key} must be a duration"))?;
        let value: f64 = remaining[..n].parse()?;
        remaining = &remaining[n..];
        let (unit, scale) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("μs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(u, _)| remaining.starts_with(u))
        .ok_or_else(|| anyhow::anyhow!("{key} must be a duration"))?;
        remaining = &remaining[unit.len()..];
        seconds += value * scale;
    }
    if !seconds.is_finite() || seconds <= 0.0 {
        bail!("{key} must be positive")
    }
    let parsed = Duration::try_from_secs_f64(seconds)?;
    // Positive fractional-nanosecond inputs (below 0.5ns) round to Duration::ZERO. Consumers such
    // as tokio::time::interval reject a zero period, so reject the setting at startup instead.
    if parsed.is_zero() {
        bail!("{key} must be at least 1ns after conversion to nanosecond precision")
    }
    Ok(parsed)
}
pub fn safe_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|u| {
        u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.fragment().is_none()
            && (u.scheme() == "https"
                || (u.scheme() == "http"
                    && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
    })
}
pub fn valid_origin(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https") && u.origin().ascii_serialization() == value
    })
}

// Development-only dotenv loading retains injected and explicitly empty process values.
// Normal serving and operator commands never read a local environment file.
pub fn load_dev_env() -> Result<()> {
    match dotenvy::from_filename(".env") {
        Ok(_) => Ok(()),
        Err(dotenvy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => bail!("could not load development environment"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_positive_inputs_that_round_to_zero() {
        for input in ["0.1ns", "0.49ns", "0.0000000001s", "0.0001us"] {
            let err = parse_duration("CLEANUP_INTERVAL", input).unwrap_err();
            assert!(
                err.to_string().contains("CLEANUP_INTERVAL"),
                "{input}: {err}"
            );
        }
    }

    #[test]
    fn preserves_valid_durations() {
        let cases = [
            ("1ns", Duration::from_nanos(1)),
            ("0.5ns", Duration::from_nanos(1)),
            ("0.6ns", Duration::from_nanos(1)),
            ("3s", Duration::from_secs(3)),
            ("5m", Duration::from_secs(300)),
            ("1m30s", Duration::from_secs(90)),
            ("2160h", Duration::from_secs(7_776_000)),
            ("9999999999h", Duration::from_secs(35_999_999_996_400)),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_duration("K", input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn unset_or_empty_value_uses_default() {
        let default = Duration::from_secs(300);
        // These keys are private to this test, so mutating them cannot race other tests.
        assert_eq!(
            duration("SERVER_COMMON_TEST_UNSET_DURATION", default).unwrap(),
            default
        );
        // SAFETY: the key is read only by this test.
        unsafe { env::set_var("SERVER_COMMON_TEST_EMPTY_DURATION", "") };
        assert_eq!(
            duration("SERVER_COMMON_TEST_EMPTY_DURATION", default).unwrap(),
            default
        );
    }

    #[test]
    fn env_adapter_rejects_zero_rounding_values() {
        // SAFETY: the key is read only by this test.
        unsafe { env::set_var("SERVER_COMMON_TEST_ZERO_DURATION", "0.1ns") };
        let err = duration("SERVER_COMMON_TEST_ZERO_DURATION", Duration::from_secs(1)).unwrap_err();
        assert!(err.to_string().contains("SERVER_COMMON_TEST_ZERO_DURATION"));
    }

    #[test]
    fn keeps_rejecting_invalid_inputs() {
        for input in [
            "0ns",
            "-1ns",
            "garbage",
            "NaNs",
            "infs",
            "99999999999999999999999999h",
        ] {
            assert!(parse_duration("K", input).is_err(), "{input}");
        }
    }

    #[tokio::test]
    async fn one_nanosecond_period_reaches_first_tick() {
        let period = parse_duration("K", "1ns").unwrap();
        tokio::time::timeout(Duration::from_secs(5), async move {
            tokio::time::interval(period).tick().await;
        })
        .await
        .expect("first tick");
    }
}
