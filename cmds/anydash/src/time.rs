// SPDX-License-Identifier: Apache-2.0
//! UTC time helpers, 15-minute buckets and bucket versions.
use chrono::{DateTime, Utc};
pub const BUCKET_SECONDS: i64 = 900;
pub const RETENTION_SECONDS: i64 = 90 * 86_400;
#[must_use]
pub fn now() -> i64 {
    Utc::now().timestamp()
}
#[must_use]
pub fn now_micros() -> u64 {
    u64::try_from(Utc::now().timestamp_micros()).unwrap_or(0)
}
/// The current UTC time as a timestamp with the server's microsecond precision.
///
/// Snapshot observation times use it so that an observation made right after a lease
/// acquisition is not truncated to a second before the lease's `acquired_at`.
#[must_use]
pub fn now_timestamp() -> prost_types::Timestamp {
    let micros = Utc::now().timestamp_micros();
    prost_types::Timestamp {
        seconds: micros.div_euclid(1_000_000),
        nanos: i32::try_from(micros.rem_euclid(1_000_000) * 1_000).unwrap_or(0),
    }
}
/// Parse an RFC 3339 timestamp with any UTC offset into Unix seconds.
#[must_use]
pub fn parse(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|t| t.with_timezone(&Utc).timestamp())
}
/// The UTC start of the 15-minute bucket that contains `seconds`.
#[must_use]
pub const fn bucket_start(seconds: i64) -> i64 {
    seconds.div_euclid(BUCKET_SECONDS) * BUCKET_SECONDS
}
/// A bucket version: the aggregation's microsecond UTC time, forced above the previous version.
#[must_use]
pub fn next_version(previous: u64, now_micros: u64) -> u64 {
    now_micros.max(previous.saturating_add(1))
}
#[must_use]
pub const fn timestamp(seconds: i64) -> prost_types::Timestamp {
    prost_types::Timestamp { seconds, nanos: 0 }
}
#[must_use]
pub fn optional(seconds: i64) -> Option<prost_types::Timestamp> {
    (seconds > 0).then(|| timestamp(seconds))
}
#[must_use]
pub fn rfc3339(seconds: i64) -> String {
    DateTime::from_timestamp(seconds, 0).map_or_else(String::new, |t| {
        t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    })
}
