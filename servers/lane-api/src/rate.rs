// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Fault, Result};
use sqlx::PgPool;

/// A separate committed transaction counts rejected requests as well as successful mutations.
pub async fn check(pool: &PgPool, scope: &str, identity: &str, limit: i64) -> Result<()> {
    let count: i64 = sqlx::query_scalar(
        "INSERT INTO lane_rate_limits(scope,identity,window_start,requests) \
         VALUES($1,$2,date_trunc('minute',clock_timestamp()),1) \
         ON CONFLICT(scope,identity) DO UPDATE SET \
         requests=CASE WHEN lane_rate_limits.window_start=EXCLUDED.window_start \
         THEN lane_rate_limits.requests+1 ELSE 1 END,window_start=EXCLUDED.window_start \
         RETURNING requests",
    )
    .bind(scope)
    .bind(identity)
    .fetch_one(pool)
    .await?;
    if count > limit {
        return Err(Fault::new(
            connectrpc::ErrorCode::ResourceExhausted,
            16,
            "request rate limit exceeded",
        ));
    }
    Ok(())
}
