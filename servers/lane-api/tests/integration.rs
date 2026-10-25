// SPDX-License-Identifier: FSL-1.1-ALv2
use lane_api::{Principal, account_lock, control, entities, id, migration};
use serde_json::json;
use sqlx::Executor;

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
