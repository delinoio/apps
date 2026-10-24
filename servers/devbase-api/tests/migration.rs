// SPDX-License-Identifier: FSL-1.1-ALv2
//! Forward migration that clears the end-user ID from scrubbed replay records.
//!
//! The case is ignored and needs a disposable `TEST_DATABASE_URL`; it creates and drops its own
//! random schema.
use devbase_api::store;
use sha2::{Digest, Sha256};
use sqlx::Row;

const FIRST: &str = include_str!("../migrations/00001_devbase.sql");
const SECOND: &str = include_str!("../migrations/00002_deleted_tenants.sql");
const THIRD: &str = include_str!("../migrations/00003_clear_scrubbed_replay_targets.sql");
// Applied migrations are immutable; these digests pin the released bytes of the first two files.
const FIRST_DIGEST: &str = "f4138e46cd97c59a6daaab11ca07cce03716dd43c3b017dec71cafb9deaae5fa";
const SECOND_DIGEST: &str = "44dc2ee4f8ac9a97e47a4074ee169f1cf1b18c1d17e7c4e1078af8057ba7959a";

fn digest(sql: &str) -> String {
    hex::encode(Sha256::digest(sql))
}

async fn insert(
    db: &sqlx::PgPool,
    key: &str,
    operation: &str,
    scrubbed: bool,
    target: Option<uuid::Uuid>,
) {
    let (fingerprint, response): (Vec<u8>, Option<Vec<u8>>) = if scrubbed {
        (vec![], None)
    } else {
        (vec![1, 2, 3], Some(vec![4, 5]))
    };
    sqlx::query("INSERT INTO replays(principal,idempotency_key,operation,tenant_id,project_id,target_user_id,fingerprint,response,scrubbed,created_at,expires_at) VALUES('caller',$1,$2,$3,$4,$5,$6,$7,$8,now(),now()+interval '24 hours')")
        .bind(key)
        .bind(operation)
        .bind(uuid::Uuid::now_v7())
        .bind(uuid::Uuid::now_v7())
        .bind(target)
        .bind(fingerprint)
        .bind(response)
        .bind(scrubbed)
        .execute(db)
        .await
        .unwrap();
}

async fn targets(db: &sqlx::PgPool) -> Vec<(String, bool, bool, bool)> {
    sqlx::query("SELECT idempotency_key,scrubbed,target_user_id IS NOT NULL AS has_target,response IS NOT NULL AS has_response FROM replays ORDER BY idempotency_key")
        .fetch_all(db)
        .await
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.get("idempotency_key"),
                r.get("scrubbed"),
                r.get("has_target"),
                r.get("has_response"),
            )
        })
        .collect()
}

#[test]
fn first_two_migrations_keep_their_released_bytes() {
    assert_eq!(digest(FIRST), FIRST_DIGEST);
    assert_eq!(digest(SECOND), SECOND_DIGEST);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn migration_clears_scrubbed_replay_targets_once() {
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
    // Build the schema exactly as the first two releases recorded it.
    sqlx::query("CREATE TABLE devbase_schema(version integer PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL)")
        .execute(&db)
        .await
        .unwrap();
    for (version, sql) in [(1, FIRST), (2, SECOND)] {
        sqlx::raw_sql(sql).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO devbase_schema VALUES($1,$2,now())")
            .bind(version)
            .bind(digest(sql))
            .execute(&db)
            .await
            .unwrap();
    }
    assert!(store::check(&db).await.is_err(), "two-migration schema");
    let user = Some(uuid::Uuid::now_v7());
    insert(&db, "scrubbed-create", "CreateEndUser", true, user).await;
    insert(&db, "scrubbed-update", "UpdateEndUser", true, user).await;
    insert(&db, "scrubbed-clean", "UpdateEndUser", true, None).await;
    insert(&db, "active-create", "CreateEndUser", false, user).await;
    insert(&db, "delete-user", "DeleteEndUser", false, user).await;
    let audit_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&db)
        .await
        .unwrap();

    store::migrate(&db).await.unwrap();
    let expected = vec![
        ("active-create".to_owned(), false, true, true),
        ("delete-user".to_owned(), false, true, true),
        ("scrubbed-clean".to_owned(), true, false, false),
        ("scrubbed-create".to_owned(), true, false, false),
        ("scrubbed-update".to_owned(), true, false, false),
    ];
    assert_eq!(targets(&db).await, expected);
    // Delete and active records keep their data.
    let kept: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM replays WHERE NOT scrubbed AND length(fingerprint)=3",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(kept, 2);

    // A second run is a no-op and the recorded digests stay the same.
    store::migrate(&db).await.unwrap();
    store::check(&db).await.unwrap();
    assert_eq!(targets(&db).await, expected);
    let rows = sqlx::query("SELECT version,digest FROM devbase_schema ORDER BY version")
        .fetch_all(&db)
        .await
        .unwrap();
    let recorded: Vec<(i32, String)> = rows
        .iter()
        .map(|r| (r.get("version"), r.get("digest")))
        .collect();
    assert_eq!(
        recorded,
        vec![
            (1, FIRST_DIGEST.to_owned()),
            (2, SECOND_DIGEST.to_owned()),
            (3, digest(THIRD))
        ]
    );
    let audit_after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(audit_after, audit_before);

    db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
