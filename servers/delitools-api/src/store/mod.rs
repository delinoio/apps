// SPDX-License-Identifier: FSL-1.1-ALv2
//! PostgreSQL access: explicit forward-only migrations, schema compatibility check and the
//! single-instance session lock.
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool, Row};

pub mod audit;
pub mod exec;
pub mod inspect;
pub mod keys;
pub mod recovery;
pub mod retention;
pub mod settings;

const MIGRATIONS: [&str; 3] = [
    include_str!("../../migrations/00001_foundation.sql"),
    include_str!("../../migrations/00002_execution.sql"),
    include_str!("../../migrations/00003_audit_reason_characters.sql"),
];
pub const SCHEMA_VERSION: i32 = MIGRATIONS.len() as i32;
/// Transaction-scoped lock serializing concurrent `--migrate` runs.
const MIGRATION_LOCK: i64 = 41_840_001;
/// Session-scoped lock rejecting a second running instance.
const INSTANCE_LOCK: i64 = 41_840_002;

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
}

impl Store {
    pub async fn open(url: &str) -> Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(20)
            .connect(url)
            .await
            .map_err(|_| anyhow::anyhow!("database unavailable"))?;
        Ok(Self { pool })
    }
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn migrate(&self) -> Result<()> {
        migrate(&self.pool).await
    }
    pub async fn check_schema(&self) -> Result<()> {
        check_schema(&self.pool).await
    }
    pub async fn acquire_instance(&self) -> Result<PgConnection> {
        acquire_instance(&self.pool).await
    }
}

fn digest(sql: &str) -> String {
    hex::encode(Sha256::digest(sql.as_bytes()))
}

/// Verify that every embedded migration is applied with an identical digest and nothing newer is
/// present. Startup refuses to serve an incompatible schema.
pub async fn check_schema(pool: &PgPool) -> Result<()> {
    let rows = sqlx::query("SELECT version,digest FROM delitools_schema ORDER BY version")
        .fetch_all(pool)
        .await
        .map_err(|_| anyhow::anyhow!("schema unavailable: run --migrate"))?;
    let version = rows.last().map_or(0, |row| row.get::<i32, _>("version"));
    ensure!(
        version == SCHEMA_VERSION && rows.len() == MIGRATIONS.len(),
        "incompatible schema version {version}; expected {SCHEMA_VERSION}"
    );
    for (index, row) in rows.iter().enumerate() {
        ensure!(
            row.get::<i32, _>("version") == index as i32 + 1
                && row.get::<String, _>("digest") == digest(MIGRATIONS[index]),
            "migration checksum mismatch"
        );
    }
    Ok(())
}

/// Apply missing migrations in one transaction. Reapplying is a no-op; a changed historical
/// migration or a newer database is rejected. There is no reverse migration.
pub async fn migrate(pool: &PgPool) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(MIGRATION_LOCK)
        .execute(&mut *tx)
        .await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS delitools_schema(version integer PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL)").execute(&mut *tx).await?;
    let applied: Vec<(i32, String)> =
        sqlx::query_as("SELECT version,digest FROM delitools_schema ORDER BY version")
            .fetch_all(&mut *tx)
            .await?;
    ensure!(
        applied.len() <= MIGRATIONS.len(),
        "database schema is newer than this binary"
    );
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let number = index as i32 + 1;
        if let Some((version, previous)) = applied.get(index) {
            ensure!(
                *version == number && *previous == digest(sql),
                "migration checksum mismatch"
            );
        } else {
            sqlx::raw_sql(sql).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO delitools_schema VALUES($1,$2,now())")
                .bind(number)
                .bind(digest(sql))
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Take the single-instance lock on a dedicated session. The connection is detached so dropping
/// it closes the session and releases the lock instead of returning it to the pool.
pub async fn acquire_instance(pool: &PgPool) -> Result<PgConnection> {
    let mut connection = pool.acquire().await?.detach();
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(INSTANCE_LOCK)
        .fetch_one(&mut connection)
        .await?;
    ensure!(acquired, "delitools instance lock unavailable");
    Ok(connection)
}

/// Round to microseconds, the PostgreSQL timestamp resolution, so values compare after storage.
pub fn now() -> chrono::DateTime<chrono::Utc> {
    server_common::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_inventory_is_complete_and_forward_only() {
        assert_eq!(MIGRATIONS.len(), SCHEMA_VERSION as usize);
        for (index, sql) in MIGRATIONS.iter().enumerate() {
            assert!(!sql.trim().is_empty());
            assert!(sql.starts_with("-- SPDX-License-Identifier: FSL-1.1-ALv2"));
            let upper = sql.to_uppercase();
            assert!(!upper.contains("DROP TABLE"), "migration {}", index + 1);
        }
        assert_eq!(digest("a"), digest("a"));
        assert_ne!(digest("a"), digest("b"));
    }
}
