// SPDX-License-Identifier: FSL-1.1-ALv2
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/00001_devbase.sql"),
    include_str!("../migrations/00002_deleted_tenants.sql"),
    include_str!("../migrations/00003_clear_scrubbed_replay_targets.sql"),
];
pub async fn check(pool: &PgPool) -> anyhow::Result<()> {
    let rows = sqlx::query("SELECT version,digest FROM devbase_schema ORDER BY version")
        .fetch_all(pool)
        .await?;
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
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(41830001)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS devbase_schema(version integer PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL)").execute(&mut *tx).await?;
    let rows = sqlx::query("SELECT version,digest FROM devbase_schema ORDER BY version")
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
        sqlx::query("INSERT INTO devbase_schema VALUES($1,$2,$3)")
            .bind(i as i32 + 1)
            .bind(digest)
            .bind(server_common::now())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
