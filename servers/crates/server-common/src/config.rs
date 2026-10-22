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
// Parses a compound duration such as `1m30s`. The result must be positive after conversion to
// nanosecond precision: a positive value below half a nanosecond rounds to zero, which periodic
// timers reject at runtime, so it is rejected here with the setting name.
fn parse_duration(key: &str, value: &str) -> Result<Duration> {
    let mut remaining = value;
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
    if parsed.is_zero() {
        bail!("{key} must be positive")
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

    fn parse(value: &str) -> Result<Duration> {
        parse_duration("CLEANUP_INTERVAL", value)
    }

    #[test]
    fn duration_accepts_positive_values() {
        assert_eq!(parse("3s").unwrap(), Duration::from_secs(3));
        assert_eq!(parse("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse("1ns").unwrap(), Duration::from_nanos(1));
        assert_eq!(parse("0.5ns").unwrap(), Duration::from_nanos(1));
        assert_eq!(parse("0.6ns").unwrap(), Duration::from_nanos(1));
    }

    #[test]
    fn duration_rejects_values_that_round_to_zero() {
        for value in ["0.1ns", "0.49ns", "0.0000000001s"] {
            let error = parse(value).unwrap_err().to_string();
            assert!(error.contains("CLEANUP_INTERVAL"), "{value}: {error}");
        }
    }

    #[test]
    fn duration_rejects_invalid_values() {
        for value in ["0ns", "0s", "-1ns", "garbage", "5", "1x", "NaNs", "infs"] {
            assert!(parse(value).is_err(), "{value}");
        }
        assert!(parse("99999999999999999999999999h").is_err());
    }

    #[test]
    fn duration_defaults_when_unset_or_empty() {
        let default = Duration::from_secs(300);
        // The key is unique to this test, so concurrent tests never observe it.
        assert_eq!(
            duration("SERVER_COMMON_TEST_UNSET_DURATION", default).unwrap(),
            default
        );
    }

    #[tokio::test]
    async fn smallest_duration_drives_a_tokio_interval() {
        let period = parse("1ns").unwrap();
        let mut interval = tokio::time::interval(period);
        tokio::time::timeout(Duration::from_secs(5), interval.tick())
            .await
            .expect("first tick");
    }
}
