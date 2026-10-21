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
    let mut remaining = s.as_str();
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
    Duration::try_from_secs_f64(seconds).map_err(Into::into)
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
