// SPDX-License-Identifier: FSL-1.1-ALv2
//! Fixed 60-second request windows shared by every replica through PostgreSQL.
use crate::{ApiError, Result};
use delino_protocol::anydash::v1::RateLimitScope;
use sqlx::PgPool;
use std::time::Duration;

pub const USER_TOKEN_LIMIT: i32 = 600;
pub const DEVICE_TOKEN_LIMIT: i32 = 60;
pub const START_ENROLLMENT_LIMIT: i32 = 10;

/// Counts one request in its own committed statement, so rejected requests count too, and
/// rejects the request once the window holds more than `limit` requests.
pub async fn check(pool: &PgPool, scope: RateLimitScope, subject: &str, limit: i32) -> Result<()> {
    use buffa::Enumeration;
    let (count, retry_ms): (i32, f64) = sqlx::query_as(
        "WITH w AS (SELECT date_bin('60 seconds', clock_timestamp(), TIMESTAMPTZ '2000-01-01') AS s) \
         INSERT INTO rate_limit_windows(scope,subject,window_start,count) SELECT $1,$2,w.s,1 FROM w \
         ON CONFLICT (scope,subject,window_start) DO UPDATE SET count=rate_limit_windows.count+1 \
         RETURNING count, extract(epoch FROM (window_start + interval '60 seconds' - clock_timestamp()))::float8 * 1000",
    )
    .bind(scope.to_i32() as i16)
    .bind(subject)
    .fetch_one(pool)
    .await?;
    if count > limit {
        let retry = Duration::from_millis(retry_ms.clamp(1.0, 60_000.0) as u64);
        return Err(ApiError::rate_limited(scope, retry));
    }
    Ok(())
}
