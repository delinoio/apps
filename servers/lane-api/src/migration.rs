// SPDX-License-Identifier: FSL-1.1-ALv2
use sqlx::PgPool;
const MIGRATIONS: &[(&str, &str)] = &[
    ("00001_auth", include_str!("../migrations/00001_auth.sql")),
    (
        "00002_entities",
        include_str!("../migrations/00002_entities.sql"),
    ),
    (
        "00003_webhooks",
        include_str!("../migrations/00003_webhooks.sql"),
    ),
    ("00004_push", include_str!("../migrations/00004_push.sql")),
    (
        "00005_limits",
        include_str!("../migrations/00005_limits.sql"),
    ),
    (
        "00006_captures",
        include_str!("../migrations/00006_captures.sql"),
    ),
];
pub async fn migrate(db: &PgPool) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('lane_schema_migration',0))")
        .execute(&mut *tx)
        .await?;
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS lane_schema_migrations(name text PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())").execute(&mut *tx).await?;
    for (name, sql) in MIGRATIONS {
        let digest = crate::hash(sql.as_bytes());
        let old: Option<String> =
            sqlx::query_scalar("SELECT digest FROM lane_schema_migrations WHERE name=$1")
                .bind(name)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(old) = old {
            anyhow::ensure!(old == digest, "applied migration changed");
            continue;
        }
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO lane_schema_migrations(name,digest) VALUES($1,$2)")
            .bind(name)
            .bind(digest)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
pub async fn check(db: &PgPool) -> anyhow::Result<()> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT name,digest FROM lane_schema_migrations ORDER BY name")
            .fetch_all(db)
            .await?;
    anyhow::ensure!(
        rows.len() == MIGRATIONS.len(),
        "unsupported or missing Lane schema; run explicit migrations"
    );
    for ((name, digest), (expected, sql)) in rows.iter().zip(MIGRATIONS) {
        anyhow::ensure!(
            name == expected && digest == &crate::hash(sql.as_bytes()),
            "unsupported Lane schema"
        );
    }
    Ok(())
}
