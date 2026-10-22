// SPDX-License-Identifier: FSL-1.1-ALv2
//! Explicit migrations and schema compatibility checks.
//!
//! Migrations are embedded SQL files recorded with a SHA-256 digest in `delitools_schema`.
//! `migrate` applies pending files under an advisory lock and refuses a database that is newer
//! than the binary or whose applied digests differ. `check` is read-only and runs at startup and
//! in the readiness probe. Serving never migrates.

use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

const MIGRATIONS: &[&str] = &[include_str!("../migrations/00001_delitools.sql")];

/// Advisory lock serializing `migrate`.
pub const MIGRATION_LOCK: i64 = 41_840_001;
/// Session advisory lock owned by the single serving instance.
pub const INSTANCE_LOCK: i64 = 41_840_002;

/// Number of embedded migrations, exposed for tests.
pub fn migration_count() -> usize {
    MIGRATIONS.len()
}

/// Verifies that the database has exactly the migrations embedded in this binary.
pub async fn check(pool: &PgPool) -> anyhow::Result<()> {
    let rows = sqlx::query("SELECT version,digest FROM delitools_schema ORDER BY version")
        .fetch_all(pool)
        .await
        .map_err(|_| anyhow::anyhow!("incompatible schema: run migrate"))?;
    anyhow::ensure!(
        rows.len() == MIGRATIONS.len(),
        "incompatible schema: run migrate"
    );
    for (i, row) in rows.iter().enumerate() {
        anyhow::ensure!(
            row.get::<i32, _>("version") == i as i32 + 1
                && row.get::<String, _>("digest") == hex::encode(Sha256::digest(MIGRATIONS[i])),
            "migration checksum mismatch"
        );
    }
    Ok(())
}

/// Applies pending migrations in one transaction. Re-running it is a no-op.
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(MIGRATION_LOCK)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS delitools_schema(version integer PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL)",
    )
    .execute(&mut *tx)
    .await?;
    let rows = sqlx::query("SELECT version,digest FROM delitools_schema ORDER BY version")
        .fetch_all(&mut *tx)
        .await?;
    anyhow::ensure!(
        rows.len() <= MIGRATIONS.len(),
        "database schema is newer than this binary"
    );
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let digest = hex::encode(Sha256::digest(sql));
        if let Some(row) = rows.get(i) {
            anyhow::ensure!(
                row.get::<i32, _>("version") == i as i32 + 1
                    && row.get::<String, _>("digest") == digest,
                "migration checksum mismatch"
            );
            continue;
        }
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO delitools_schema VALUES($1,$2,$3)")
            .bind(i as i32 + 1)
            .bind(digest)
            .bind(server_common::now())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
