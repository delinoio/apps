// SPDX-License-Identifier: FSL-1.1-ALv2
//! Protobuf time conversions and fixed AnyDash intervals.
use buffa_types::google::protobuf::{Duration as ProtoDuration, Timestamp};
use chrono::{DateTime, TimeZone, Utc};

/// 15-minute usage bucket length in seconds.
pub const BUCKET_SECONDS: i64 = 900;
/// Retention of usage buckets, hourly snapshots and rotated refresh tokens.
pub const RETENTION_DAYS: i64 = 90;
/// Lease validity after the holder's latest heartbeat or renewal; also the online window.
pub const LEASE_SECONDS: i64 = 300;
/// Physical deletion deadline after a deletion request.
pub const DELETION_HOURS: i64 = 24;

pub fn timestamp(t: DateTime<Utc>) -> Timestamp {
    Timestamp {
        seconds: t.timestamp(),
        nanos: t.timestamp_subsec_nanos() as i32,
        ..Default::default()
    }
}
/// Reads a timestamp field; absent or out-of-range values are `None`.
pub fn from_timestamp(t: &Timestamp) -> Option<DateTime<Utc>> {
    if !(0..1_000_000_000).contains(&t.nanos) {
        return None;
    }
    let value = Utc.timestamp_opt(t.seconds, t.nanos as u32).single()?;
    // Store PostgreSQL microsecond precision.
    DateTime::from_timestamp_micros(value.timestamp_micros())
}
pub fn duration(d: std::time::Duration) -> ProtoDuration {
    ProtoDuration {
        seconds: d.as_secs() as i64,
        nanos: d.subsec_nanos() as i32,
        ..Default::default()
    }
}
pub fn seconds(n: i64) -> ProtoDuration {
    ProtoDuration {
        seconds: n,
        nanos: 0,
        ..Default::default()
    }
}
/// The current time at PostgreSQL microsecond precision.
pub fn now() -> DateTime<Utc> {
    server_common::now()
}
pub fn aligned(t: DateTime<Utc>) -> bool {
    t.timestamp_subsec_nanos() == 0 && t.timestamp().rem_euclid(BUCKET_SECONDS) == 0
}
pub fn floor_bucket(t: DateTime<Utc>) -> DateTime<Utc> {
    let s = t.timestamp();
    Utc.timestamp_opt(s - s.rem_euclid(BUCKET_SECONDS), 0)
        .single()
        .expect("bucket floor is representable")
}
pub fn ceil_bucket(t: DateTime<Utc>) -> DateTime<Utc> {
    let floor = floor_bucket(t);
    if floor == t {
        floor
    } else {
        floor + chrono::Duration::seconds(BUCKET_SECONDS)
    }
}
pub fn floor_hour(t: DateTime<Utc>) -> DateTime<Utc> {
    let s = t.timestamp();
    Utc.timestamp_opt(s - s.rem_euclid(3600), 0)
        .single()
        .expect("hour floor is representable")
}
/// The oldest bucket start accepted now: the first bucket boundary at or after 90 days ago.
pub fn retention_start(now: DateTime<Utc>) -> DateTime<Utc> {
    ceil_bucket(now - chrono::Duration::days(RETENTION_DAYS))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buckets_align_to_fifteen_utc_minutes() {
        let t = Utc.with_ymd_and_hms(2026, 3, 1, 10, 14, 59).unwrap();
        assert_eq!(
            floor_bucket(t),
            Utc.with_ymd_and_hms(2026, 3, 1, 10, 0, 0).unwrap()
        );
        assert_eq!(
            ceil_bucket(t),
            Utc.with_ymd_and_hms(2026, 3, 1, 10, 15, 0).unwrap()
        );
        assert!(aligned(
            Utc.with_ymd_and_hms(2026, 3, 1, 10, 45, 0).unwrap()
        ));
        assert!(!aligned(t));
        let ts = timestamp(t);
        assert_eq!(from_timestamp(&ts), Some(t));
        assert_eq!(
            from_timestamp(&Timestamp {
                seconds: 0,
                nanos: -1,
                ..Default::default()
            }),
            None
        );
    }
}
