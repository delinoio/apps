// SPDX-License-Identifier: Apache-2.0
//! Refresh cadence, exponential backoff, `Retry-After` and the manual refresh interval.
use std::time::Duration;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const LOCAL_SYNC_INTERVAL: Duration = Duration::from_mins(1);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_mins(1);
pub const EXTERNAL_INTERVAL: Duration = Duration::from_mins(5);
pub const MANUAL_REFRESH_INTERVAL: Duration = Duration::from_mins(1);
const BACKOFF_BASE: Duration = Duration::from_mins(1);
const BACKOFF_CAP: Duration = Duration::from_mins(30);
/// The wait after the n-th consecutive failure: `min(30 min, 1 min × 2^(n−1))` plus uniform
/// jitter of up to 20% of that value (`jitter` in `[0, 1)`), capped at 30 minutes.
#[must_use]
pub fn backoff(failures: u32, jitter: f64) -> Duration {
    let exponent = failures.saturating_sub(1).min(16);
    let base = BACKOFF_BASE.saturating_mul(1 << exponent).min(BACKOFF_CAP);
    base.saturating_add(base.mul_f64(0.2 * jitter.clamp(0.0, 1.0)))
        .min(BACKOFF_CAP)
}
#[must_use]
pub fn jitter() -> f64 {
    rand::random::<f64>()
}
/// Parse `Retry-After` as delta seconds or an HTTP date relative to `now` (Unix seconds).
#[must_use]
pub fn retry_after(value: &str, now: i64) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = chrono::DateTime::parse_from_rfc2822(value)
        .ok()?
        .timestamp();
    Some(Duration::from_secs(u64::try_from(at - now).unwrap_or(0)))
}
/// The remaining wait before another manual refresh is allowed, or `None` when allowed.
#[must_use]
pub fn manual_wait(last: i64, now: i64) -> Option<Duration> {
    let interval = i64::try_from(MANUAL_REFRESH_INTERVAL.as_secs()).unwrap_or(60);
    let remaining = last + interval - now;
    (last > 0 && remaining > 0).then(|| Duration::from_secs(u64::try_from(remaining).unwrap_or(0)))
}
#[must_use]
pub fn seconds(d: Duration) -> i64 {
    i64::try_from(d.as_secs()).unwrap_or(i64::MAX)
}
