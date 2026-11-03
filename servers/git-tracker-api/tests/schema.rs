// SPDX-License-Identifier: FSL-1.1-ALv2
//! Forward-only migration tracking against a disposable PostgreSQL schema.
use git_tracker_api::schema::{MIGRATIONS, check, migrate};
use sqlx::PgPool;

async fn isolated() -> (PgPool, PgPool, String) {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("tracker_{}", uuid::Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |c, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {search}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    (db, admin, schema)
}

async fn versions(db: &PgPool) -> Vec<i32> {
    sqlx::query_scalar("SELECT version FROM git_tracker_schema ORDER BY version")
        .fetch_all(db)
        .await
        .unwrap()
}

async fn drop_schema(admin: PgPool, schema: String) {
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn migrate_is_repeatable_and_check_requires_every_migration() {
    let (db, admin, schema) = isolated().await;
    assert!(check(&db).await.is_err(), "an empty database is not ready");
    migrate(&db).await.unwrap();
    migrate(&db).await.unwrap();
    check(&db).await.unwrap();
    assert_eq!(
        versions(&db).await,
        (1..=MIGRATIONS.len() as i32).collect::<Vec<_>>()
    );
    let events: bool =
        sqlx::query_scalar("SELECT to_regclass('git_tracker_webhook_events') IS NOT NULL")
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(events);
    drop_schema(admin, schema).await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn database_with_only_the_bootstrap_schema_upgrades_in_place() {
    let (db, admin, schema) = isolated().await;
    // The state of every database created before migration tracking existed.
    sqlx::raw_sql(MIGRATIONS[0]).execute(&db).await.unwrap();
    let space = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO git_tracker_repositories(id,github_id,space_id,full_name,installation_id,public) VALUES($1,42,$2,'owner/repo','1',true)")
        .bind(uuid::Uuid::now_v7())
        .bind(space)
        .execute(&db)
        .await
        .unwrap();
    assert!(check(&db).await.is_err(), "the upgrade is not applied yet");

    migrate(&db).await.unwrap();
    migrate(&db).await.unwrap();
    check(&db).await.unwrap();
    assert_eq!(
        versions(&db).await,
        (1..=MIGRATIONS.len() as i32).collect::<Vec<_>>()
    );
    let kept: i64 =
        sqlx::query_scalar("SELECT count(*) FROM git_tracker_repositories WHERE space_id=$1")
            .bind(space)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(kept, 1, "existing data survives the upgrade");
    drop_schema(admin, schema).await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn migrate_refuses_changed_or_newer_migrations() {
    let (db, admin, schema) = isolated().await;
    migrate(&db).await.unwrap();
    sqlx::query("UPDATE git_tracker_schema SET digest='changed' WHERE version=2")
        .execute(&db)
        .await
        .unwrap();
    assert!(
        migrate(&db)
            .await
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
    assert!(
        check(&db)
            .await
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );

    sqlx::query("UPDATE git_tracker_schema SET digest=(SELECT digest FROM git_tracker_schema WHERE version=1) WHERE version=2")
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM git_tracker_schema WHERE version=2")
        .execute(&db)
        .await
        .unwrap();
    // A missing record on a database that already has the tables is a failure, not a re-run of
    // the migration that created them.
    assert!(migrate(&db).await.is_err());
    sqlx::query("INSERT INTO git_tracker_schema VALUES(2,'x',now()),(3,'x',now())")
        .execute(&db)
        .await
        .unwrap();
    assert!(
        migrate(&db)
            .await
            .unwrap_err()
            .to_string()
            .contains("newer")
    );
    drop_schema(admin, schema).await;
}
