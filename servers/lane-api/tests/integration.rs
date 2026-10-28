// SPDX-License-Identifier: FSL-1.1-ALv2
use lane_api::{Principal, account_lock, control, entities, hash, id, migration};
use serde_json::json;
use sqlx::Executor;

#[tokio::test]
#[ignore = "requires disposable LANE_TEST_DATABASE_URL"]
async fn rejection_migration_preserves_prior_digests_and_legacy_rows() {
    let url = std::env::var("LANE_TEST_DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("lane_rejection_migration_{}", id().replace('-', ""));
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    let search = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |conn, _| {
            let query = format!("SET search_path TO {search}");
            Box::pin(async move {
                conn.execute(query.as_str()).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    pool.execute("CREATE TABLE lane_schema_migrations(name text PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())").await.unwrap();
    let prior = [
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
        (
            "00007_planning",
            include_str!("../migrations/00007_planning.sql"),
        ),
    ];
    for (name, sql) in prior {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO lane_schema_migrations(name,digest) VALUES($1,$2)")
            .bind(name)
            .bind(hash(sql.as_bytes()))
            .execute(&pool)
            .await
            .unwrap();
    }
    let legacy_account = id();
    let legacy_request = id();
    sqlx::query("INSERT INTO lane_sync_rejections(account_id,request_id,reason) VALUES($1,$2,19)")
        .bind(&legacy_account)
        .bind(&legacy_request)
        .execute(&pool)
        .await
        .unwrap();
    assert!(migration::check(&pool).await.is_err());
    let before: Vec<(String, String)> =
        sqlx::query_as("SELECT name,digest FROM lane_schema_migrations ORDER BY name")
            .fetch_all(&pool)
            .await
            .unwrap();
    migration::migrate(&pool).await.unwrap();
    migration::check(&pool).await.unwrap();
    let after: Vec<(String,String)> = sqlx::query_as("SELECT name,digest FROM lane_schema_migrations WHERE name<>'00008_sync_rejections' ORDER BY name")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(after, before);
    let uncorrelated: bool = sqlx::query_scalar("SELECT epoch IS NULL AND device_id IS NULL AND entity_id IS NULL AND version_id IS NULL AND base_heads IS NULL FROM lane_sync_rejections WHERE account_id=$1 AND request_id=$2")
        .bind(&legacy_account).bind(&legacy_request).fetch_one(&pool).await.unwrap();
    assert!(uncorrelated);
    migration::migrate(&pool).await.unwrap();
    assert!(migration::check(&pool).await.is_ok());
    pool.close().await;
    admin
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
#[ignore = "requires disposable LANE_TEST_DATABASE_URL"]
async fn complete_schema_deletion_directives_redaction_and_rollback() {
    let url = std::env::var("LANE_TEST_DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("lane_integrated_{}", id().replace('-', ""));
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    let search = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |conn, _| {
            let query = format!("SET search_path TO {search}");
            Box::pin(async move {
                conn.execute(query.as_str()).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    migration::migrate(&pool).await.unwrap();
    migration::migrate(&pool).await.unwrap();
    migration::check(&pool).await.unwrap();
    let p = Principal {
        account_id: id(),
        tenant_id: id(),
        session_id: id(),
        device_id: id(),
        device_name: "fixture".into(),
        origin_kind: "ORIGIN_KIND_CLI".into(),
    };
    let epoch = id();
    sqlx::query("INSERT INTO lane_accounts VALUES($1,$2,$3)")
        .bind(&p.account_id)
        .bind(&p.tenant_id)
        .bind(&epoch)
        .execute(&pool)
        .await
        .unwrap();
    let deleted = id();
    let kept = id();
    let other = id();
    for (old, mode) in [
        (&deleted, "DATA_DELETION_MODE_DELETE_EVERYWHERE"),
        (&kept, "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL"),
    ] {
        sqlx::query("INSERT INTO lane_account_deletions(account_id,old_epoch,new_epoch,mode) VALUES($1,$2,$3,$4)").bind(&p.account_id).bind(old).bind(id()).bind(mode).execute(&pool).await.unwrap();
    }
    let mut tx = pool.begin().await.unwrap();
    account_lock(&mut tx, &p.account_id).await.unwrap();
    assert!(
        control::deletion_pull(&mut tx, &p, &json!({"epoch":epoch}))
            .await
            .unwrap()
            .is_none()
    );
    let directive = control::deletion_pull(&mut tx, &p, &json!({"epoch":deleted}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        directive,
        json!({"changes":[{"sequence":"0","deletion":{"kind":"ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT","id":p.account_id,"deletionVersionId":epoch}}],"nextCursor":"","hasMore":false})
    );
    assert_eq!(
        directive["changes"][0]["deletion"]["kind"],
        "ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT"
    );
    assert!(directive["changes"][0].get("entity").is_none());
    for old in [&kept, &other] {
        assert_eq!(
            control::deletion_pull(&mut tx, &p, &json!({"epoch":old}))
                .await
                .unwrap_err()
                .reason,
            17
        );
    }
    let other_p = Principal {
        account_id: id(),
        tenant_id: id(),
        ..p.clone()
    };
    sqlx::query("INSERT INTO lane_accounts VALUES($1,$2,$3)")
        .bind(&other_p.account_id)
        .bind(&other_p.tenant_id)
        .bind(id())
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        control::deletion_pull(&mut tx, &other_p, &json!({"epoch":deleted}))
            .await
            .unwrap_err()
            .reason,
        17
    );
    let repo = id();
    let task = id();
    entities::dispatch(
        &mut tx,
        &p,
        "CreateRepository",
        &json!({"meta":{"requestId":id()},"id":repo,"url":"https://example.com/a/b"}),
        65536,
    )
    .await
    .unwrap();
    entities::dispatch(&mut tx,&p,"CreateTask",&json!({"meta":{"requestId":id()},"id":task,"repository":{"id":repo},"title":"private synthetic title","prompt":"private synthetic prompt"}),65536).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    account_lock(&mut tx, &p.account_id).await.unwrap();
    lane_api::clear_account_content(&mut tx, &p.account_id)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM lane_entities WHERE account_id=$1")
        .bind(&p.account_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    let mut tx = pool.begin().await.unwrap();
    account_lock(&mut tx, &p.account_id).await.unwrap();
    lane_api::clear_account_content(&mut tx, &p.account_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for table in [
        "lane_entities",
        "lane_versions",
        "lane_heads",
        "lane_entity_receipts",
        "lane_changes",
        "lane_push_deliveries",
        "lane_webhook_events",
    ] {
        let n: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE account_id=$1"))
                .bind(&p.account_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 0, "{table}");
    }
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM lane_account_deletions WHERE account_id=$1")
            .bind(&p.account_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(retained, 2);
    let counters: i64 = sqlx::query_scalar("SELECT count(*) FROM lane_push_counters WHERE total=0")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(counters, 3);
    pool.close().await;
    admin
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
#[ignore = "requires disposable LANE_TEST_DATABASE_URL"]
async fn migrate_runs_the_webhook_cleanup_for_repositories_deleted_before_alias_resolution() {
    let url = std::env::var("LANE_TEST_DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("lane_cleanup_{}", id().replace('-', ""));
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    let search = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |conn, _| {
            let query = format!("SET search_path TO {search}");
            Box::pin(async move {
                conn.execute(query.as_str()).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    migration::migrate(&pool).await.unwrap();
    // A receiver that kept a synchronized alias of a repository that was later deleted.
    let (account, canonical, alias, receiver) = (id(), id(), id(), id());
    sqlx::query("INSERT INTO lane_entities(account_id,id,kind,deleted,accepted_version_id) VALUES($1,$2,1,true,$3)")
        .bind(&account)
        .bind(&canonical)
        .bind(id())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO lane_repository_mappings(account_id,local_id,canonical_id) VALUES($1,$2,$3)",
    )
    .bind(&account)
    .bind(&alias)
    .bind(&canonical)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO lane_webhook_receivers(account_id,id,repository_id,name,encrypted_key) VALUES($1,$2,$3,'retained','\\x00')")
        .bind(&account)
        .bind(&receiver)
        .bind(&alias)
        .execute(&pool)
        .await
        .unwrap();
    migration::migrate(&pool).await.unwrap();
    let (name, key, deletion): (String, Option<Vec<u8>>, Option<String>) = sqlx::query_as(
        "SELECT name,encrypted_key,deletion_version_id FROM lane_webhook_receivers WHERE id=$1",
    )
    .bind(&receiver)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(name, "");
    assert!(key.is_none() && deletion.is_some());
    migration::migrate(&pool).await.unwrap();
    let again: (Option<String>, i64) = sqlx::query_as(
        "SELECT deletion_version_id,revision FROM lane_webhook_receivers WHERE id=$1",
    )
    .bind(&receiver)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(again.0, deletion);
    assert_eq!(again.1, 2);
    pool.close().await;
    admin
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .unwrap();
}
