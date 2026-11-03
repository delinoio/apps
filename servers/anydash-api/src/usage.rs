// SPDX-License-Identifier: FSL-1.1-ALv2
//! Query periods and local hourly and daily views built from 15-minute UTC buckets.
use crate::time::{BUCKET_SECONDS, RETENTION_DAYS, floor_bucket};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Offset, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use delino_protocol::anydash::v1::{QueryPeriod, TimeGranularity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// One local hour or day. A repeated local hour appears twice with different offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Point {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub utc_offset_minutes: i32,
}

/// The first instant of a local calendar day. When midnight does not exist because of a DST
/// gap, the day starts at the first valid local time after it.
pub fn local_day_start(tz: Tz, date: NaiveDate) -> DateTime<Utc> {
    let midnight = date.and_hms_opt(0, 0, 0).expect("midnight");
    for minutes in (0..=180).step_by(15) {
        let candidate = midnight + Duration::minutes(minutes);
        if let Some(t) = tz.from_local_datetime(&candidate).earliest() {
            return t.with_timezone(&Utc);
        }
    }
    // No real zone skips more than three hours; fall back to the UTC interpretation.
    Utc.from_utc_datetime(&midnight)
}

/// The UTC range of a query period in the selected time zone. Ranges end now and never start
/// earlier than the 90-day retention.
pub fn period_range(period: QueryPeriod, tz: Tz, now: DateTime<Utc>) -> Range {
    let today = now.with_timezone(&tz).date_naive();
    let days_back = |n: i64| local_day_start(tz, today - Duration::days(n - 1));
    let start = match period {
        QueryPeriod::Last24Hours => now - Duration::hours(24),
        QueryPeriod::Today => local_day_start(tz, today),
        QueryPeriod::ThisWeek => local_day_start(
            tz,
            today - Duration::days(i64::from(today.weekday().num_days_from_monday())),
        ),
        QueryPeriod::ThisMonth => local_day_start(tz, today.with_day(1).expect("first day")),
        QueryPeriod::Last7Days => days_back(7),
        QueryPeriod::Last30Days => days_back(30),
        QueryPeriod::Last90Days => days_back(90),
        _ => now - Duration::hours(24),
    };
    Range {
        start: start.max(now - Duration::days(RETENTION_DAYS)),
        end: now,
    }
}

/// The local hour or day that contains `t`.
pub fn point_of(granularity: TimeGranularity, tz: Tz, t: DateTime<Utc>) -> Point {
    let local = t.with_timezone(&tz);
    let offset = local.offset().fix().local_minus_utc();
    match granularity {
        TimeGranularity::Day => {
            let date = local.date_naive();
            let start = local_day_start(tz, date);
            let end = local_day_start(tz, date + Duration::days(1));
            Point {
                start,
                end,
                utc_offset_minutes: start.with_timezone(&tz).offset().fix().local_minus_utc() / 60,
            }
        }
        _ => {
            let naive = local.naive_local();
            let hour = naive
                .with_minute(0)
                .and_then(|n| n.with_second(0))
                .and_then(|n| n.with_nanosecond(0))
                .expect("hour start");
            let start = Utc.from_utc_datetime(&(hour - Duration::seconds(i64::from(offset))));
            Point {
                start,
                end: start + Duration::hours(1),
                utc_offset_minutes: offset / 60,
            }
        }
    }
}

/// Every local hour or day that intersects the range, in time order. Skipped DST hours have no
/// point; repeated hours appear twice.
pub fn points(granularity: TimeGranularity, tz: Tz, range: Range) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::new();
    let mut t = floor_bucket(range.start);
    while t < range.end {
        let p = point_of(granularity, tz, t);
        if out.last() != Some(&p) {
            out.push(p);
        }
        t += Duration::seconds(BUCKET_SECONDS);
    }
    out
}

/// Whether a granularity is allowed for a period.
pub fn granularity_allowed(period: QueryPeriod, granularity: TimeGranularity) -> bool {
    match granularity {
        TimeGranularity::Day => true,
        TimeGranularity::Hour => matches!(
            period,
            QueryPeriod::Last24Hours
                | QueryPeriod::Today
                | QueryPeriod::Last7Days
                | QueryPeriod::ThisWeek
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }
    /// Sums 15-minute bucket values into local points, like the widget queries do.
    fn aggregate(
        g: TimeGranularity,
        tz: Tz,
        buckets: &[(DateTime<Utc>, u64)],
    ) -> Vec<(Point, u64)> {
        let mut out: Vec<(Point, u64)> = Vec::new();
        for (t, v) in buckets {
            let p = point_of(g, tz, *t);
            match out.iter_mut().find(|(q, _)| *q == p) {
                Some((_, sum)) => *sum += v,
                None => out.push((p, *v)),
            }
        }
        out
    }

    #[test]
    fn half_hour_offset_hours_are_exact() {
        let tz: Tz = "Asia/Kolkata".parse().unwrap();
        // 00:30 UTC is 06:00 IST, so the local hour 06:00-07:00 is 00:30-01:30 UTC.
        let buckets: Vec<_> = (0..8)
            .map(|i| (utc(2026, 5, 1, 0, 0) + Duration::minutes(15 * i), 1))
            .collect();
        let hours = aggregate(TimeGranularity::Hour, tz, &buckets);
        assert_eq!(hours.len(), 3);
        assert_eq!(hours[0].0.start, utc(2026, 4, 30, 23, 30));
        assert_eq!(hours[0].1, 2);
        assert_eq!(hours[1].0.start, utc(2026, 5, 1, 0, 30));
        assert_eq!(hours[1].1, 4);
        assert_eq!(hours[1].0.utc_offset_minutes, 330);
        assert_eq!(hours[2].1, 2);
    }

    #[test]
    fn quarter_hour_offset_hours_and_days_are_exact() {
        let tz: Tz = "Asia/Kathmandu".parse().unwrap();
        let p = point_of(TimeGranularity::Hour, tz, utc(2026, 5, 1, 0, 0));
        // 00:00 UTC is 05:45 NPT; the local hour 05:00-06:00 is 23:15-00:15 UTC.
        assert_eq!(p.start, utc(2026, 4, 30, 23, 15));
        assert_eq!(p.end, utc(2026, 5, 1, 0, 15));
        assert_eq!(p.utc_offset_minutes, 345);
        assert_eq!(
            point_of(TimeGranularity::Hour, tz, utc(2026, 5, 1, 0, 15)).start,
            utc(2026, 5, 1, 0, 15)
        );
        // 18:00 UTC is 23:45 local, still on the day that started at 18:15 UTC the day before.
        let day = point_of(TimeGranularity::Day, tz, utc(2026, 5, 1, 18, 0));
        assert_eq!(day.start, utc(2026, 4, 30, 18, 15));
        let day = point_of(TimeGranularity::Day, tz, utc(2026, 5, 1, 18, 15));
        assert_eq!(day.start, utc(2026, 5, 1, 18, 15));
        assert_eq!(day.end - day.start, Duration::hours(24));
    }

    #[test]
    fn dst_days_have_23_or_25_hours_and_repeated_hours_appear_twice() {
        let tz: Tz = "America/New_York".parse().unwrap();
        // Spring forward on 2026-03-08: 02:00-03:00 local does not exist.
        let spring = point_of(TimeGranularity::Day, tz, utc(2026, 3, 8, 12, 0));
        assert_eq!(spring.end - spring.start, Duration::hours(23));
        let hours = points(
            TimeGranularity::Hour,
            tz,
            Range {
                start: spring.start,
                end: spring.end,
            },
        );
        assert_eq!(hours.len(), 23);
        // Fall back on 2026-11-01: 01:00-02:00 local happens twice.
        let fall = point_of(TimeGranularity::Day, tz, utc(2026, 11, 1, 12, 0));
        assert_eq!(fall.end - fall.start, Duration::hours(25));
        let hours = points(
            TimeGranularity::Hour,
            tz,
            Range {
                start: fall.start,
                end: fall.end,
            },
        );
        assert_eq!(hours.len(), 25);
        let one_am: Vec<_> = hours
            .iter()
            .filter(|p| p.start.with_timezone(&tz).hour() == 1)
            .collect();
        assert_eq!(one_am.len(), 2);
        assert_eq!(one_am[0].utc_offset_minutes, -240);
        assert_eq!(one_am[1].utc_offset_minutes, -300);
        // Buckets in the repeated hour are kept apart by offset.
        let buckets = [(utc(2026, 11, 1, 5, 15), 1), (utc(2026, 11, 1, 6, 15), 10)];
        let sums = aggregate(TimeGranularity::Hour, tz, &buckets);
        assert_eq!(sums.len(), 2);
        assert_eq!(sums[0].0.start, utc(2026, 11, 1, 5, 0));
        assert_eq!(sums[1].0.start, utc(2026, 11, 1, 6, 0));
    }

    #[test]
    fn periods_start_at_local_midnight_and_respect_retention() {
        let tz: Tz = "Asia/Kathmandu".parse().unwrap();
        let now = utc(2026, 5, 6, 12, 0); // Wednesday 17:45 local
        assert_eq!(
            period_range(QueryPeriod::Today, tz, now).start,
            utc(2026, 5, 5, 18, 15)
        );
        assert_eq!(
            period_range(QueryPeriod::ThisWeek, tz, now).start,
            utc(2026, 5, 3, 18, 15)
        );
        assert_eq!(
            period_range(QueryPeriod::ThisMonth, tz, now).start,
            utc(2026, 4, 30, 18, 15)
        );
        assert_eq!(
            period_range(QueryPeriod::Last7Days, tz, now).start,
            utc(2026, 4, 29, 18, 15)
        );
        assert_eq!(
            period_range(QueryPeriod::Last24Hours, tz, now).start,
            utc(2026, 5, 5, 12, 0)
        );
        let r = period_range(QueryPeriod::Last90Days, tz, now);
        assert!(r.start >= now - Duration::days(90));
        assert_eq!(r.end, now);
        assert!(granularity_allowed(
            QueryPeriod::Last7Days,
            TimeGranularity::Hour
        ));
        assert!(!granularity_allowed(
            QueryPeriod::Last30Days,
            TimeGranularity::Hour
        ));
    }
}
