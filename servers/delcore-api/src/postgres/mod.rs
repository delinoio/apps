// SPDX-License-Identifier: FSL-1.1-ALv2
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};

pub mod allowances;
pub mod audit;
pub mod catalog;
pub mod period;
pub mod rating;
pub use allowances::*;
pub use audit::*;
pub use catalog::*;
pub use period::*;
pub use rating::*;

pub const SCHEMA_VERSION: i32 = 14;
const MIGRATIONS: [&str; 14] = [
    include_str!("../../migrations/00001_foundation.sql"),
    include_str!("../../migrations/00002_payment_history.sql"),
    include_str!("../../migrations/00003_retention.sql"),
    include_str!("../../migrations/00004_allowances.sql"),
    include_str!("../../migrations/00005_settlement_refunds.sql"),
    include_str!("../../migrations/00006_catalog_versions.sql"),
    include_str!("../../migrations/00007_event_metering.sql"),
    include_str!("../../migrations/00008_metering_rating.sql"),
    include_str!("../../migrations/00009_metering_status.sql"),
    include_str!("../../migrations/00010_metering_attribute_retention.sql"),
    include_str!("../../migrations/00011_call_time_prices.sql"),
    include_str!("../../migrations/00012_api_keys.sql"),
    include_str!("../../migrations/00013_entitlement_window_index.sql"),
    include_str!("../../migrations/00014_deferrable_group_names.sql"),
];
#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
}
impl Store {
    pub async fn open(url: &str) -> Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(20)
            .connect(url)
            .await?;
        Ok(Self { pool })
    }
    pub async fn check_schema(&self) -> Result<()> {
        check_schema(&self.pool).await
    }
    pub async fn migrate(&self) -> Result<()> {
        migrate(&self.pool).await
    }
    pub async fn acquire_instance(&self) -> Result<PgConnection> {
        acquire_instance(&self.pool).await
    }
    pub async fn begin(&self) -> std::result::Result<Transaction<'_, Postgres>, sqlx::Error> {
        begin(&self.pool).await
    }
}
pub async fn check_schema(pool: &PgPool) -> Result<()> {
    let version: i32 = sqlx::query_scalar("SELECT COALESCE(max(version),0) FROM delcore_schema")
        .fetch_one(pool)
        .await
        .map_err(|_| anyhow::anyhow!("schema unavailable: run migrate"))?;
    ensure!(
        version == SCHEMA_VERSION,
        "incompatible schema version {version}; expected {SCHEMA_VERSION}"
    );
    Ok(())
}
pub async fn migrate(pool: &PgPool) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(41820001)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS delcore_schema(version integer PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL)").execute(&mut *tx).await?;
    let version: i32 = sqlx::query_scalar("SELECT COALESCE(max(version),0) FROM delcore_schema")
        .fetch_one(&mut *tx)
        .await?;
    ensure!(
        version <= SCHEMA_VERSION,
        "database schema is newer than this binary"
    );
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let number = index as i32 + 1;
        let digest = hex::encode(Sha256::digest(sql.as_bytes()));
        if number <= version {
            let previous: String =
                sqlx::query_scalar("SELECT digest FROM delcore_schema WHERE version=$1")
                    .bind(number)
                    .fetch_one(&mut *tx)
                    .await?;
            ensure!(previous == digest, "migration checksum mismatch");
        } else {
            sqlx::raw_sql(sql).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO delcore_schema VALUES($1,$2,now())")
                .bind(number)
                .bind(digest)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
pub async fn acquire_instance(pool: &PgPool) -> Result<PgConnection> {
    // Detach this session: dropping it must close the lock, not return it to the pool.
    let mut connection = pool.acquire().await?.detach();
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(41820002)")
        .fetch_one(&mut connection)
        .await?;
    ensure!(acquired, "delcore instance lock unavailable");
    Ok(connection)
}
pub async fn begin(pool: &PgPool) -> std::result::Result<Transaction<'_, Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // Single-instance v1 serializes state transitions. External requests must run
    // outside this lock. Remove only with per-aggregate locks and concurrency tests.
    sqlx::query("SELECT pg_advisory_xact_lock(41820003)")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_inventory_is_complete() {
        assert_eq!(MIGRATIONS.len(), SCHEMA_VERSION as usize);
        for sql in MIGRATIONS {
            assert!(!sql.is_empty());
        }
    }

    struct Database {
        pool: PgPool,
        schema: String,
        url: String,
    }
    impl Database {
        async fn new() -> Self {
            let url =
                std::env::var("TEST_DATABASE_URL").expect("requires disposable TEST_DATABASE_URL");
            let admin = PgPool::connect(&url).await.unwrap();
            let schema = format!("delcore_migration_{}", uuid::Uuid::now_v7().simple());
            sqlx::query(&format!("CREATE SCHEMA {schema}"))
                .execute(&admin)
                .await
                .unwrap();
            admin.close().await;
            let search = schema.clone();
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(3)
                .after_connect(move |connection, _| {
                    let search = search.clone();
                    Box::pin(async move {
                        sqlx::query(&format!("SET search_path TO {search}"))
                            .execute(connection)
                            .await?;
                        Ok(())
                    })
                })
                .connect(&url)
                .await
                .unwrap();
            Self { pool, schema, url }
        }
    }
    impl Drop for Database {
        fn drop(&mut self) {
            let url = self.url.clone();
            let schema = self.schema.clone();
            // An independent runtime removes the isolated schema even when an
            // assertion unwinds the test runtime.
            std::thread::spawn(move || {
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(async move {
                        if let Ok(pool) = PgPool::connect(&url).await {
                            let _ = sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
                                .execute(&pool)
                                .await;
                            pool.close().await;
                        }
                    });
            })
            .join()
            .unwrap();
        }
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing to disposable PostgreSQL"]
    async fn price_history_upgrade_baselines_current_version_and_rolls_back_mutations() {
        let db = Database::new().await;
        let mut tx = db.pool.begin().await.unwrap();
        sqlx::query("CREATE TABLE delcore_schema(version integer PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL)").execute(&mut *tx).await.unwrap();
        for (index, sql) in MIGRATIONS[..10].iter().enumerate() {
            sqlx::raw_sql(sql).execute(&mut *tx).await.unwrap();
            sqlx::query("INSERT INTO delcore_schema VALUES($1,$2,now())")
                .bind(index as i32 + 1)
                .bind(hex::encode(Sha256::digest(sql.as_bytes())))
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        let entity = ensure_catalog_entity(&mut tx, "price", "plan:legacy", "requests", "")
            .await
            .unwrap();
        let a = select_catalog_version(&mut tx, &entity, &serde_json::json!({"unit_price":"1"}))
            .await
            .unwrap()
            .0;
        let b = select_catalog_version(&mut tx, &entity, &serde_json::json!({"unit_price":"2"}))
            .await
            .unwrap()
            .0;
        tx.commit().await.unwrap();
        let before = chrono::Utc::now();
        migrate(&db.pool).await.unwrap();
        migrate(&db.pool).await.unwrap();
        let history: Vec<(String, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as("SELECT version_id::text,effective_at FROM catalog_price_activations")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].0, b);
        assert!(history[0].1 >= before);
        let mut tx = db.pool.begin().await.unwrap();
        select_catalog_version(&mut tx, &entity, &serde_json::json!({"unit_price":"1"}))
            .await
            .unwrap();
        let restored: String = sqlx::query_scalar("SELECT version_id::text FROM catalog_price_activations ORDER BY effective_at DESC LIMIT 1")
            .fetch_one(&mut *tx).await.unwrap();
        assert_eq!(restored, a);
        tx.rollback().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_price_activations")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        db.pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing to disposable PostgreSQL"]
    async fn migrations_are_idempotent_and_record_original_checksums() {
        let db = Database::new().await;
        assert!(check_schema(&db.pool).await.is_err());
        migrate(&db.pool).await.unwrap();
        check_schema(&db.pool).await.unwrap();
        let before: Vec<(i32, String, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as("SELECT version,digest,applied_at FROM delcore_schema ORDER BY version")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(before.len(), MIGRATIONS.len());
        for (index, (version, digest, _)) in before.iter().enumerate() {
            assert_eq!(*version, index as i32 + 1);
            assert_eq!(
                *digest,
                hex::encode(Sha256::digest(MIGRATIONS[index].as_bytes()))
            );
        }
        migrate(&db.pool).await.unwrap();
        let after: Vec<(i32, String, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as("SELECT version,digest,applied_at FROM delcore_schema ORDER BY version")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(before, after);
        db.pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing to disposable PostgreSQL"]
    async fn migration_checksum_tampering_is_rejected_without_repair() {
        let db = Database::new().await;
        migrate(&db.pool).await.unwrap();
        sqlx::query("UPDATE delcore_schema SET digest='tampered' WHERE version=1")
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(
            migrate(&db.pool)
                .await
                .unwrap_err()
                .to_string()
                .contains("migration checksum mismatch")
        );
        let digest: String =
            sqlx::query_scalar("SELECT digest FROM delcore_schema WHERE version=1")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(digest, "tampered");
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM delcore_schema")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count, i64::from(SCHEMA_VERSION));
        db.pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing to disposable PostgreSQL"]
    async fn newer_schema_is_rejected_by_startup_and_migration() {
        let db = Database::new().await;
        migrate(&db.pool).await.unwrap();
        sqlx::query("INSERT INTO delcore_schema VALUES($1,'future',now())")
            .bind(SCHEMA_VERSION + 1)
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(
            check_schema(&db.pool)
                .await
                .unwrap_err()
                .to_string()
                .contains("incompatible schema version")
        );
        assert!(
            migrate(&db.pool)
                .await
                .unwrap_err()
                .to_string()
                .contains("database schema is newer than this binary")
        );
        let latest: i32 = sqlx::query_scalar("SELECT max(version) FROM delcore_schema")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(latest, SCHEMA_VERSION + 1);
        db.pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing to disposable PostgreSQL"]
    async fn instance_lock_excludes_second_session_and_releases_on_drop() {
        let db = Database::new().await;
        let mut owner = acquire_instance(&db.pool).await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut owner)
            .await
            .unwrap();
        let error = acquire_instance(&db.pool)
            .await
            .expect_err("second instance must fail");
        assert!(error.to_string().contains("instance lock unavailable"));
        drop(owner);
        // Closing a detached socket releases the session lock asynchronously on
        // PostgreSQL. Bound the wait instead of assuming immediate server cleanup.
        let mut successor = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let Ok(connection) = acquire_instance(&db.pool).await {
                    break connection;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("session lock was not released");
        let successor_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut successor)
            .await
            .unwrap();
        assert_ne!(
            pid, successor_pid,
            "owner connection must close, not return to the pool"
        );
        drop(successor);
        db.pool.close().await;
    }
}
