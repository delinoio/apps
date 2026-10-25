// SPDX-License-Identifier: FSL-1.1-ALv2
//! Fresh schema initialization and checksum validation against disposable PostgreSQL.
use devbase_api::store;
use sha2::{Digest, Sha256};

const FIRST: &str = include_str!("../migrations/00001_devbase.sql");
const SECOND: &str = include_str!("../migrations/00002_deleted_tenants.sql");
fn digest(sql: &str) -> String {
    hex::encode(Sha256::digest(sql))
}

async fn empty_schema() -> (sqlx::PgPool, sqlx::PgPool, String) {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("devbase_{}", uuid::Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {search}"))
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE devbase_schema(version integer PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL)")
        .execute(&db)
        .await
        .unwrap();
    (admin, db, schema)
}

async fn drop_schema(admin: sqlx::PgPool, db: sqlx::PgPool, schema: String) {
    db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn fresh_schema_is_checked_and_migration_is_idempotent() {
    let (admin, db, schema) = empty_schema().await;
    assert!(store::check(&db).await.is_err());
    store::migrate(&db).await.unwrap();
    store::check(&db).await.unwrap();
    let before: Vec<(i32, String)> =
        sqlx::query_as("SELECT version,digest FROM devbase_schema ORDER BY version")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(before, vec![(1, digest(FIRST)), (2, digest(SECOND))]);
    store::migrate(&db).await.unwrap();
    let after: Vec<(i32, String)> =
        sqlx::query_as("SELECT version,digest FROM devbase_schema ORDER BY version")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(before, after);
    sqlx::query("UPDATE devbase_schema SET digest='invalid' WHERE version=1")
        .execute(&db)
        .await
        .unwrap();
    assert!(store::check(&db).await.is_err());
    assert!(store::migrate(&db).await.is_err());
    drop_schema(admin, db, schema).await;
}
