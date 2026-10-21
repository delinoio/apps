// SPDX-License-Identifier: FSL-1.1-ALv2
//! Storage integration tests: migrations, schema compatibility, the session lock, created_at
//! immutability, retention boundaries, restart recovery and one-time settings seeding.
mod support;
use chrono::{Duration, Utc};
use delitools_api::{
    settings::Settings,
    store::{self, Store, recovery, retention},
};
use sqlx::{PgPool, Row};
use support::{Fixture, pool_for};
use uuid::Uuid;

/// A bare schema with no migrations, for tests of the migration machinery itself.
struct Bare {
    pool: PgPool,
    admin_url: String,
    schema: String,
}
impl Bare {
    async fn new() -> Self {
        let admin_url =
            std::env::var("TEST_DATABASE_URL").expect("requires disposable TEST_DATABASE_URL");
        let schema = format!("delitools_bare_{}", Uuid::now_v7().simple());
        let admin = PgPool::connect(&admin_url).await.unwrap();
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let pool = pool_for(&admin_url, &schema).await;
        Self {
            pool,
            admin_url,
            schema,
        }
    }
    async fn finish(self) {
        self.pool.close().await;
        let admin = PgPool::connect(&self.admin_url).await.unwrap();
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn migrations_are_explicit_idempotent_and_checked() {
    let db = Bare::new().await;
    // Nothing is applied implicitly: the schema check fails before --migrate.
    assert!(store::check_schema(&db.pool).await.is_err());
    store::migrate(&db.pool).await.unwrap();
    store::migrate(&db.pool).await.unwrap();
    store::check_schema(&db.pool).await.unwrap();
    let versions: Vec<i32> =
        sqlx::query_scalar("SELECT version FROM delitools_schema ORDER BY version")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(versions, (1..=store::SCHEMA_VERSION).collect::<Vec<_>>());

    // A changed historical migration is rejected by both the check and a repeated migrate.
    sqlx::query("UPDATE delitools_schema SET digest='tampered' WHERE version=1")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(store::check_schema(&db.pool).await.is_err());
    assert!(store::migrate(&db.pool).await.is_err());
    sqlx::query("UPDATE delitools_schema SET digest=(SELECT digest FROM delitools_schema WHERE version=2) WHERE version=1")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(store::check_schema(&db.pool).await.is_err());
    db.finish().await;

    // A database newer than the binary is refused by startup and by --migrate.
    let db = Bare::new().await;
    store::migrate(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO delitools_schema VALUES($1,'future',now())")
        .bind(store::SCHEMA_VERSION + 1)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(store::check_schema(&db.pool).await.is_err());
    assert!(store::migrate(&db.pool).await.is_err());
    db.finish().await;

    // An older database (one migration missing) is refused until --migrate runs.
    let db = Bare::new().await;
    store::migrate(&db.pool).await.unwrap();
    sqlx::query("DELETE FROM delitools_schema WHERE version=2")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(store::check_schema(&db.pool).await.is_err());
    db.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn schema_stores_no_content_or_recoverable_secret_columns() {
    let f = Fixture::setup().await;
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT table_name||'.'||column_name FROM information_schema.columns WHERE table_schema=current_schema() ORDER BY 1",
    )
    .fetch_all(&f.pool)
    .await
    .unwrap();
    assert!(columns.len() > 50, "{columns:?}");
    // Exact column names that would hold content or a recoverable secret must not exist.
    for column in &columns {
        let name = column.split('.').nth(1).unwrap();
        for forbidden in [
            "text",
            "texts",
            "input",
            "source",
            "schema",
            "json_schema",
            "result",
            "results",
            "output",
            "translation",
            "payload",
            "secret",
            "token",
            "prompt",
            "content",
            "body",
        ] {
            assert_ne!(name, forbidden, "unexpected column {column}");
        }
    }
    // Expiry-governed tables use an immutable creation time.
    for table in [
        "requests",
        "attempts",
        "idempotency_records",
        "audit_records",
        "operational_events",
    ] {
        assert!(
            columns.contains(&format!("{table}.created_at")),
            "{table} lacks created_at"
        );
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn single_instance_session_lock_is_exclusive_and_released_on_drop() {
    let f = Fixture::setup().await;
    let store = Store::from_pool(f.pool.clone());
    let first = store.acquire_instance().await.unwrap();
    assert!(
        store.acquire_instance().await.is_err(),
        "a second instance must be rejected"
    );
    drop(first);
    // The session ends asynchronously after the connection drops; wait briefly.
    let mut acquired = None;
    for _ in 0..50 {
        if let Ok(connection) = store.acquire_instance().await {
            acquired = Some(connection);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        acquired.is_some(),
        "the lock must be released with its session"
    );
    drop(acquired);
    f.finish().await;
}

async fn insert_request(pool: &PgPool, state: i16, created_at: chrono::DateTime<Utc>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO requests(id,tenant_id,capability,state,model,credential_kind,created_at,updated_at) VALUES($1,$2,1,$3,'m',3,$4,$4)")
        .bind(id)
        .bind(Uuid::now_v7())
        .bind(state)
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_attempt(
    pool: &PgPool,
    request: Uuid,
    created_at: chrono::DateTime<Utc>,
    usage_known: bool,
    delivered: bool,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO attempts(id,request_id,tenant_id,capability,model,price_effective_at,catalog_price,occurred_at,measured_usage,delivered_at,retry_at,delivery_failures,created_at,updated_at) VALUES($1,$2,$3,1,'m',$4,'{\"input_usd_per_token\":\"0.000001\"}',$4,$5,$6,$4,3,$4,$4)")
        .bind(id)
        .bind(request)
        .bind(Uuid::now_v7())
        .bind(created_at)
        .bind(usage_known.then(|| serde_json::json!({"input_tokens":"1"})))
        .bind(delivered.then_some(created_at))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_idempotency(
    pool: &PgPool,
    state: i16,
    created_at: chrono::DateTime<Utc>,
) -> (Uuid, String) {
    let tenant = Uuid::now_v7();
    let key = Uuid::now_v7().to_string();
    sqlx::query("INSERT INTO idempotency_records(tenant_id,method,idempotency_key,request_id,input_hash,state,created_at,updated_at) VALUES($1,1,$2,$3,$4,$5,$6,$6)")
        .bind(tenant)
        .bind(&key)
        .bind(Uuid::now_v7())
        .bind(vec![0_u8; 32])
        .bind(state)
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
    (tenant, key)
}

async fn insert_audit_and_event(pool: &PgPool, created_at: chrono::DateTime<Utc>) {
    sqlx::query("INSERT INTO audit_records(id,actor,action,reason,request_id,created_at) VALUES($1,'operator:x','settings.update','r',$2,$3)")
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO operational_events(id,category,severity,correlation_id,created_at) VALUES($1,'startup','info',$2,$3)")
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn retention_deletes_at_exactly_thirty_days_in_every_state() {
    let f = Fixture::setup().await;
    let pool = &f.pool;
    let now = store::now();
    let exact = now - Duration::days(30);
    let almost = exact + Duration::microseconds(1);
    let old = exact - Duration::days(5);

    // Requests in every state: processing, completed and unknown.
    for state in [1, 2, 3] {
        insert_request(pool, state, exact).await;
        insert_request(pool, state, old).await;
    }
    let kept_request = insert_request(pool, 1, almost).await;
    // Attempts: unresolved (usage unknown), undelivered (usage known, retried) and delivered.
    let owner = insert_request(pool, 2, now).await;
    for (known, delivered) in [(false, false), (true, false), (true, true)] {
        insert_attempt(pool, owner, exact, known, delivered).await;
        insert_attempt(pool, owner, old, known, delivered).await;
    }
    let kept_attempt = insert_attempt(pool, owner, almost, false, false).await;
    for state in [1, 2, 3] {
        insert_idempotency(pool, state, exact).await;
    }
    let kept_idempotency = insert_idempotency(pool, 1, almost).await;
    insert_audit_and_event(pool, exact).await;
    insert_audit_and_event(pool, almost).await;
    // API keys never expire with request records.
    let key = delitools_api::keys::generate();
    store::keys::insert(
        pool,
        store::keys::NewKey {
            id: key.id,
            tenant_id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            kind: delino_protocol::delitools::v1::KeyKind::KEY_KIND_SERVICE,
            capabilities: delitools_api::keys::Capabilities::ALL,
            secret_hash: &key.hash,
            expires_at: None,
        },
        old,
    )
    .await
    .unwrap();

    let report = retention::sweep(pool, now).await.unwrap();
    assert_eq!(report.total(), 6 + 6 + 3 + 1 + 1);
    assert_eq!(count(pool, "requests").await, 2, "kept + owner");
    assert_eq!(count(pool, "attempts").await, 1);
    assert_eq!(count(pool, "idempotency_records").await, 1);
    assert_eq!(count(pool, "audit_records").await, 1);
    assert_eq!(count(pool, "operational_events").await, 1);
    assert_eq!(count(pool, "api_keys").await, 1);
    let remaining: Uuid = sqlx::query_scalar("SELECT id FROM attempts")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(remaining, kept_attempt);
    let _ = (kept_request, kept_idempotency);

    // The record that survived by one microsecond is gone one microsecond later.
    let later = retention::sweep(pool, now + Duration::microseconds(1))
        .await
        .unwrap();
    assert_eq!(later.total(), 1 + 1 + 1 + 1 + 1);
    assert_eq!(count(pool, "attempts").await, 0);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn state_changes_and_retries_never_extend_retention() {
    let f = Fixture::setup().await;
    let pool = &f.pool;
    let now = store::now();
    let created = now - Duration::days(30);
    let request = insert_request(pool, 1, created).await;
    let attempt = insert_attempt(pool, request, created, false, false).await;
    let (tenant, key) = insert_idempotency(pool, 1, created).await;

    // Every ordinary state transition keeps working and leaves created_at alone.
    sqlx::query("UPDATE requests SET state=3,updated_at=$2 WHERE id=$1")
        .bind(request)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE attempts SET delivery_failures=delivery_failures+1,retry_at=$2,updated_at=$2 WHERE id=$1")
        .bind(attempt)
        .bind(now + Duration::days(1))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE idempotency_records SET state=3,updated_at=$3 WHERE tenant_id=$1 AND idempotency_key=$2")
        .bind(tenant)
        .bind(&key)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    // Moving the creation time is refused by the database for every retained table.
    for (table, filter) in [
        ("requests", format!("id='{request}'")),
        ("attempts", format!("id='{attempt}'")),
        ("idempotency_records", format!("idempotency_key='{key}'")),
        ("audit_records", "true".to_owned()),
        ("operational_events", "true".to_owned()),
    ] {
        if table == "audit_records" || table == "operational_events" {
            insert_audit_and_event(pool, created).await;
        }
        let result = sqlx::query(&format!(
            "UPDATE {table} SET created_at=created_at+interval '1 day' WHERE {filter}"
        ))
        .execute(pool)
        .await;
        assert!(result.is_err(), "{table} created_at must be immutable");
    }
    let report = retention::sweep(pool, now).await.unwrap();
    assert!(report.total() >= 3);
    assert_eq!(count(pool, "requests").await, 0);
    assert_eq!(count(pool, "attempts").await, 0);
    assert_eq!(count(pool, "idempotency_records").await, 0);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn restart_marks_processing_work_unknown_without_touching_completed_work() {
    let f = Fixture::setup().await;
    let pool = &f.pool;
    let now = store::now();
    let processing = insert_request(pool, 1, now).await;
    let completed = insert_request(pool, 2, now).await;
    insert_idempotency(pool, 1, now).await;
    insert_idempotency(pool, 2, now).await;
    assert_eq!(
        recovery::mark_interrupted_unknown(pool, now).await.unwrap(),
        1
    );
    let states: Vec<(Uuid, i16)> = sqlx::query("SELECT id,state FROM requests")
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get("id"), r.get("state")))
        .collect();
    assert!(states.contains(&(processing, 3)));
    assert!(states.contains(&(completed, 2)));
    let idempotency: Vec<i16> =
        sqlx::query_scalar("SELECT state FROM idempotency_records ORDER BY state")
            .fetch_all(pool)
            .await
            .unwrap();
    assert_eq!(idempotency, vec![2, 3]);
    assert_eq!(
        recovery::mark_interrupted_unknown(pool, now).await.unwrap(),
        0
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn backlog_counts_follow_usage_and_delivery_state() {
    let f = Fixture::setup().await;
    let pool = &f.pool;
    let now = store::now();
    let request = insert_request(pool, 3, now).await;
    insert_attempt(pool, request, now, false, false).await;
    insert_attempt(pool, request, now, true, false).await;
    insert_attempt(pool, request, now, true, true).await;
    // Created 29 days ago and undelivered: inside a two-day alert window.
    insert_attempt(pool, request, now - Duration::days(29), true, false).await;
    let backlog = recovery::backlog(pool, now, 2 * 86_400).await.unwrap();
    assert_eq!(backlog.unknown_usage, 1);
    assert_eq!(backlog.pending_delivery, 2);
    assert_eq!(backlog.unknown_requests, 1);
    assert_eq!(backlog.nearing_expiry, 1);
    assert!(!backlog.price_available);
    let narrow = recovery::backlog(pool, now, 3600).await.unwrap();
    assert_eq!(narrow.nearing_expiry, 0);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn settings_are_seeded_once_and_then_owned_by_the_database() {
    let f = Fixture::setup().await;
    let stored = store::settings::load(&f.pool).await.unwrap();
    let mut other: Settings = stored.clone();
    other.tenant_concurrency += 7;
    other.translation_models = vec!["other/model".into()];
    let reseeded = store::settings::seed(&f.pool, &other, store::now())
        .await
        .unwrap();
    assert!(
        !reseeded,
        "a second seed must not overwrite stored settings"
    );
    assert_eq!(store::settings::load(&f.pool).await.unwrap(), stored);
    f.finish().await;
}
