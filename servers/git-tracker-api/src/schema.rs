// SPDX-License-Identifier: FSL-1.1-ALv2
//! Explicit, forward-only schema migrations.
//!
//! Migration files are immutable once released. Every later change to the schema is a new,
//! numbered file appended to [`MIGRATIONS`]; there are no reverse migrations. `migrate` records the
//! SHA-256 digest of each applied file in `git_tracker_schema`, so it is safe to run repeatedly and
//! refuses a database whose recorded files differ from this binary's.
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

/// Ordered migration files. Version `n` is `MIGRATIONS[n - 1]`.
pub const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/00001_git_tracker.sql"),
    include_str!("../migrations/00002_webhook_events.sql"),
];

fn digest(sql: &str) -> String {
    hex::encode(Sha256::digest(sql))
}

/// Verifies that the database holds exactly this binary's migrations. A database that predates
/// version tracking, or one that misses a later migration, fails until `migrate` runs.
pub async fn check(pool: &PgPool) -> anyhow::Result<()> {
    let present: bool = sqlx::query_scalar("SELECT to_regclass('git_tracker_schema') IS NOT NULL")
        .fetch_one(pool)
        .await?;
    anyhow::ensure!(present, "incompatible schema: run migrate");
    let rows = sqlx::query("SELECT version,digest FROM git_tracker_schema ORDER BY version")
        .fetch_all(pool)
        .await?;
    anyhow::ensure!(
        rows.len() == MIGRATIONS.len(),
        "incompatible schema: run migrate"
    );
    for (i, row) in rows.iter().enumerate() {
        anyhow::ensure!(
            row.get::<i32, _>("version") == i as i32 + 1
                && row.get::<String, _>("digest") == digest(MIGRATIONS[i]),
            "migration checksum mismatch"
        );
    }
    Ok(())
}

/// Applies the migrations that the database lacks, in one transaction under an advisory lock.
///
/// Compatibility: Git Tracker once shipped without version tracking, as a single bootstrap script
/// that is not repeatable. A database that already holds the bootstrap tables but has no version
/// record is recorded as version 1 without running that script again, then upgraded. Remove this
/// branch only when no such database can remain.
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('git_tracker_schema',0))")
        .execute(&mut *tx)
        .await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS git_tracker_schema(version integer PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL)")
        .execute(&mut *tx)
        .await?;
    let rows = sqlx::query("SELECT version,digest FROM git_tracker_schema ORDER BY version")
        .fetch_all(&mut *tx)
        .await?;
    anyhow::ensure!(
        rows.len() <= MIGRATIONS.len(),
        "database schema is newer than this binary"
    );
    let legacy = rows.is_empty()
        && sqlx::query_scalar::<_, bool>(
            "SELECT to_regclass('git_tracker_repositories') IS NOT NULL",
        )
        .fetch_one(&mut *tx)
        .await?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let digest = digest(sql);
        if let Some(row) = rows.get(i) {
            anyhow::ensure!(
                row.get::<i32, _>("version") == i as i32 + 1
                    && row.get::<String, _>("digest") == digest,
                "migration checksum mismatch"
            );
            continue;
        }
        if i == 0 && legacy {
            tracing::info!("recording the existing bootstrap schema as migration 1");
        } else {
            sqlx::raw_sql(sql).execute(&mut *tx).await?;
            tracing::info!(version = i + 1, "migration applied");
        }
        sqlx::query("INSERT INTO git_tracker_schema VALUES($1,$2,$3)")
            .bind(i as i32 + 1)
            .bind(digest)
            .bind(server_common::now())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
