// SPDX-License-Identifier: FSL-1.1-ALv2
//! Explicit forward migrations. `migrate` applies them; serving only verifies the schema.
use sqlx::PgPool;

const MIGRATIONS: &[(&str, &str)] = &[(
    "00001_foundation",
    include_str!("../migrations/00001_foundation.sql"),
)];

pub async fn migrate(db: &PgPool) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('anydash_schema_migration',0))")
        .execute(&mut *tx)
        .await?;
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS anydash_schema_migrations(name text PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())",
    )
    .execute(&mut *tx)
    .await?;
    for (name, sql) in MIGRATIONS {
        let digest = crate::sha256(sql.as_bytes());
        let old: Option<String> =
            sqlx::query_scalar("SELECT digest FROM anydash_schema_migrations WHERE name=$1")
                .bind(name)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(old) = old {
            anyhow::ensure!(old == digest, "applied migration {name} changed");
            continue;
        }
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO anydash_schema_migrations(name,digest) VALUES($1,$2)")
            .bind(name)
            .bind(digest)
            .execute(&mut *tx)
            .await?;
        tracing::info!(migration = name, "migration applied");
    }
    tx.commit().await?;
    Ok(())
}

/// Refuses a missing, older, newer or modified schema. Never migrates.
pub async fn check(db: &PgPool) -> anyhow::Result<()> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT name,digest FROM anydash_schema_migrations ORDER BY name")
            .fetch_all(db)
            .await
            .map_err(|_| anyhow::anyhow!("missing AnyDash schema; run anydash-api migrate"))?;
    anyhow::ensure!(
        rows.len() == MIGRATIONS.len(),
        "unsupported AnyDash schema version; run explicit migrations"
    );
    for ((name, digest), (expected, sql)) in rows.iter().zip(MIGRATIONS) {
        anyhow::ensure!(
            name == expected && digest == &crate::sha256(sql.as_bytes()),
            "unsupported AnyDash schema"
        );
    }
    Ok(())
}
