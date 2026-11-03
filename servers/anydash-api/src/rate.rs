// SPDX-License-Identifier: FSL-1.1-ALv2
//! Fixed 60-second request windows shared by every replica through PostgreSQL.
use crate::{ApiError, Result};
use delino_protocol::anydash::v1::RateLimitScope;
use sqlx::{PgExecutor, PgPool};
use std::time::Duration;

pub const USER_TOKEN_LIMIT: i32 = 600;
pub const DEVICE_TOKEN_LIMIT: i32 = 60;
pub const START_ENROLLMENT_LIMIT: i32 = 10;
/// Invalid refresh tokens per client IP address. They are counted apart from the device limit,
/// so an unauthenticated caller cannot exhaust the quota of a device whose ID it knows.
pub const INVALID_REFRESH_TOKEN_LIMIT: i32 = 60;

/// The subject of the invalid refresh-token limit for one client IP address.
pub fn invalid_refresh_subject(client_ip: &str) -> String {
    format!("invalid-refresh:{client_ip}")
}

/// Counts one request and rejects it once the window holds more than `limit` requests.
///
/// On a pool the count commits in its own statement, so rejected requests count too. A handler
/// that charges inside its transaction passes the transaction's connection instead of waiting
/// for a second pooled connection while it holds row locks; it must then commit the transaction
/// before it returns a rejection, so that the count persists.
pub async fn check<'e>(
    db: impl PgExecutor<'e>,
    scope: RateLimitScope,
    subject: &str,
    limit: i32,
) -> Result<()> {
    use buffa::Enumeration;
    let (count, retry_ms): (i32, f64) = sqlx::query_as(
        "WITH w AS (SELECT date_bin('60 seconds', clock_timestamp(), TIMESTAMPTZ '2000-01-01') AS s) \
         INSERT INTO rate_limit_windows(scope,subject,window_start,count) SELECT $1,$2,w.s,1 FROM w \
         ON CONFLICT (scope,subject,window_start) DO UPDATE SET count=rate_limit_windows.count+1 \
         RETURNING count, extract(epoch FROM (window_start + interval '60 seconds' - clock_timestamp()))::float8 * 1000",
    )
    .bind(scope.to_i32() as i16)
    .bind(subject)
    .fetch_one(db)
    .await?;
    if count > limit {
        let retry = Duration::from_millis(retry_ms.clamp(1.0, 60_000.0) as u64);
        return Err(ApiError::rate_limited(scope, retry));
    }
    Ok(())
}

/// Rejects the request when the current window already holds `limit` requests, without counting
/// it. Used before authentication so that a caller over an unauthenticated limit learns nothing.
pub async fn ensure_open(
    pool: &PgPool,
    scope: RateLimitScope,
    subject: &str,
    limit: i32,
) -> Result<()> {
    use buffa::Enumeration;
    let row: Option<(i32, f64)> = sqlx::query_as(
        "SELECT count, extract(epoch FROM (window_start + interval '60 seconds' - clock_timestamp()))::float8 * 1000 \
         FROM rate_limit_windows WHERE scope=$1 AND subject=$2 \
         AND window_start=date_bin('60 seconds', clock_timestamp(), TIMESTAMPTZ '2000-01-01')",
    )
    .bind(scope.to_i32() as i16)
    .bind(subject)
    .fetch_optional(pool)
    .await?;
    if let Some((count, retry_ms)) = row
        && count >= limit
    {
        let retry = Duration::from_millis(retry_ms.clamp(1.0, 60_000.0) as u64);
        return Err(ApiError::rate_limited(scope, retry));
    }
    Ok(())
}
