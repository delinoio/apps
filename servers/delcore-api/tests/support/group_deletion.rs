// SPDX-License-Identifier: FSL-1.1-ALv2
//! Final tenant deletion with permuted group names, and the group-name constraint behind it.
use super::{Fixture, key};
use chrono::Utc;
use serde_json::{Value, json};
use sha2::Digest;
use sqlx::Row;
use uuid::Uuid;

fn uid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

/// Creates an organization owned by `owner` and returns its ID.
async fn organization(f: &Fixture, name: &str) -> String {
    f.rpc(
        "owner",
        "CreateTenant",
        json!({"requestKey":key(),"name":name}),
    )
    .await["tenant"]["id"]
        .as_str()
        .unwrap()
        .into()
}

/// Creates a group with the owner as member and one permission; returns the group ID.
async fn group(f: &Fixture, tenant: &str, service: &str, owner: &Value, name: &str) -> String {
    f.rpc("owner","SetGroup",json!({"requestKey":key(),"tenantId":tenant,"group":{"name":name,"userIds":[owner],"permissions":[{"serviceId":service,"capability":"execute"}]}})).await["group"]["id"].as_str().unwrap().into()
}

/// Renames a group without touching its members or permissions.
async fn rename(f: &Fixture, tenant: &str, id: &str, name: &str, service: &str, owner: &Value) {
    f.rpc("owner","SetGroup",json!({"requestKey":key(),"tenantId":tenant,"group":{"id":id,"name":name,"userIds":[owner],"permissions":[{"serviceId":service,"capability":"execute"}]}})).await;
}

async fn names(f: &Fixture, tenant: &str) -> Vec<(String, String)> {
    sqlx::query("SELECT id::text,name FROM groups WHERE tenant_id=$1 ORDER BY id")
        .bind(uid(tenant))
        .fetch_all(&f.db)
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect()
}

async fn count(f: &Fixture, sql: &str, tenant: &str) -> i64 {
    sqlx::query_scalar(sql)
        .bind(uid(tenant))
        .fetch_one(&f.db)
        .await
        .unwrap()
}

/// Requests deletion through the API, then moves the request back past both grace periods.
async fn request_deletion_and_expire_grace(f: &Fixture, tenant: &str) {
    f.rpc(
        "owner",
        "DeleteTenant",
        json!({"requestKey":key(),"tenantId":tenant}),
    )
    .await;
    let grace = f.config.reporting_grace_seconds + f.config.deletion_grace_seconds + 86_400;
    sqlx::query("UPDATE tenants SET deleting_at=now()-make_interval(secs=>$2::double precision) WHERE id=$1")
        .bind(uid(tenant))
        .bind(grace as f64)
        .execute(&f.db)
        .await
        .unwrap();
}

struct World {
    f: Fixture,
    service: String,
    owner: Value,
}

async fn world() -> World {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let service = f.service().await;
    let owner = f.rpc("owner", "GetCurrentUser", json!({})).await["userId"].clone();
    World { f, service, owner }
}

/// Builds an organization whose `n` groups have their names permuted cyclically to each
/// other's IDs through legal API calls, and returns the tenant and group IDs in cycle order.
async fn cyclic_organization(w: &World, label: &str, n: usize) -> (String, Vec<String>) {
    let f = &w.f;
    let tenant = organization(f, label).await;
    let mut ids = Vec::new();
    for i in 0..n {
        ids.push(group(f, &tenant, &w.service, &w.owner, &format!("{label}-{i}")).await);
    }
    // Group i takes the ID of group i+1. Every intermediate state keeps the names unique.
    for i in 0..n {
        rename(f, &tenant, &ids[i], &ids[(i + 1) % n], &w.service, &w.owner).await;
    }
    for i in 0..n {
        assert!(
            names(f, &tenant)
                .await
                .contains(&(ids[i].clone(), ids[(i + 1) % n].clone()))
        );
    }
    (tenant, ids)
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn final_deletion_anonymizes_crossed_and_cyclic_group_names_in_one_transaction() {
    let w = world().await;
    let f = &w.f;
    for (label, n) in [("pair", 2), ("cycle", 3), ("single", 1), ("none", 0)] {
        let (tenant, ids) = cyclic_organization(&w, label, n).await;
        let permissions = count(
            f,
            "SELECT count(*) FROM group_permissions WHERE group_id IN (SELECT id FROM groups WHERE tenant_id=$1)",
            &tenant,
        )
        .await;
        assert_eq!(permissions, n as i64);
        assert_eq!(
            count(
                f,
                "SELECT count(*) FROM group_members WHERE tenant_id=$1",
                &tenant
            )
            .await,
            n as i64
        );
        request_deletion_and_expire_grace(f, &tenant).await;
        // The worker step runs maintenance first, as production does.
        f.worker.step().await.unwrap();
        let after = names(f, &tenant).await;
        assert_eq!(after.len(), n);
        for (id, name) in &after {
            assert_eq!(id, name, "{label}: final name must equal the group ID");
        }
        let mut expected = ids.clone();
        expected.sort();
        assert_eq!(
            after.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            count(
                f,
                "SELECT count(*) FROM tenants WHERE id=$1 AND deleted_at IS NOT NULL",
                &tenant
            )
            .await,
            1,
            "{label}"
        );
        assert_eq!(
            count(
                f,
                "SELECT count(*) FROM group_members WHERE tenant_id=$1",
                &tenant
            )
            .await,
            0
        );
        assert_eq!(
            count(
                f,
                "SELECT count(*) FROM members WHERE tenant_id=$1",
                &tenant
            )
            .await,
            0
        );
        assert_eq!(
            count(
                f,
                "SELECT count(*) FROM group_permissions WHERE group_id IN (SELECT id FROM groups WHERE tenant_id=$1)",
                &tenant
            )
            .await,
            permissions,
            "{label}: permissions must be retained"
        );
        // Another pass after success is harmless and leaves the same result.
        f.worker.step().await.unwrap();
        assert_eq!(names(f, &tenant).await, after, "{label}");
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn final_deletion_removes_api_keys_and_keeps_other_tenants_progressing() {
    let w = world().await;
    let f = &w.f;
    let (doomed, _) = cyclic_organization(&w, "doomed", 2).await;
    let created = f.rpc("owner","CreateApiKey",json!({"requestKey":key(),"tenantId":doomed,"ownerKind":"API_KEY_OWNER_KIND_TENANT","name":"k","grants":[{"serviceId":w.service,"capabilities":["execute"]}]})).await;
    assert!(created["secret"].is_string());
    // A second tenant has an accepted metering event that still needs processing.
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let other = me["tenants"][0]["id"].clone();
    f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"declare meter","meter":{"serviceId":w.service,"key":"requests","name":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT","valueAttribute":"","groupBy":[]}})).await;
    f.rpc("service","IngestEvent",json!({"requestKey":key(),"event":{"tenantId":other,"serviceId":w.service,"eventKey":"progress","eventType":"request","occurredAt":Utc::now().to_rfc3339(),"attributes":{}}})).await;
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM metering_events WHERE applied_revision<latest_revision",
    )
    .fetch_one(&f.db)
    .await
    .unwrap();
    assert_eq!(pending, 1);
    request_deletion_and_expire_grace(f, &doomed).await;
    // One step must finish the deletion and still reach metering for the other tenant.
    f.worker.step().await.unwrap();
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM metering_events WHERE applied_revision<latest_revision",
    )
    .fetch_one(&f.db)
    .await
    .unwrap();
    assert_eq!(pending, 0, "metering must progress in the same step");
    assert_eq!(
        count(
            f,
            "SELECT count(*) FROM tenants WHERE id=$1 AND deleted_at IS NOT NULL",
            &doomed
        )
        .await,
        1
    );
    assert_eq!(
        count(
            f,
            "SELECT count(*) FROM api_keys WHERE tenant_id=$1",
            &doomed
        )
        .await,
        0
    );
    let orphan_grants: i64 = sqlx::query_scalar("SELECT count(*) FROM api_key_grants g WHERE NOT EXISTS(SELECT 1 FROM api_keys k WHERE k.id=g.key_id)")
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(orphan_grants, 0);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn independent_holds_still_prevent_finalization_of_permuted_groups() {
    let w = world().await;
    let f = &w.f;
    let holds = [
        "INSERT INTO disputes(external_id,tenant_id,active) VALUES(gen_random_uuid()::text,$1,true)",
        "UPDATE tenants SET external_subscription_id='sub_hold' WHERE id=$1",
        "INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES(gen_random_uuid(),$1,'topup',gen_random_uuid()::text,'{}',now(),now(),now())",
    ];
    for hold in holds {
        let (tenant, ids) = cyclic_organization(&w, "held", 2).await;
        request_deletion_and_expire_grace(f, &tenant).await;
        let before = names(f, &tenant).await;
        if let Err(error) = sqlx::query(hold).bind(uid(&tenant)).execute(&f.db).await {
            // Keep this test honest if the hold fixture's columns drift from the schema.
            panic!("hold fixture failed: {hold}: {error}");
        }
        f.worker.step().await.unwrap();
        assert_eq!(names(f, &tenant).await, before, "{hold}");
        assert_eq!(
            count(
                f,
                "SELECT count(*) FROM tenants WHERE id=$1 AND deleted_at IS NULL",
                &tenant
            )
            .await,
            1,
            "{hold}"
        );
        assert_eq!(before.len(), ids.len());
    }
    // An unelapsed grace boundary also holds.
    let (tenant, _) = cyclic_organization(&w, "fresh", 2).await;
    f.rpc(
        "owner",
        "DeleteTenant",
        json!({"requestKey":key(),"tenantId":tenant}),
    )
    .await;
    let before = names(f, &tenant).await;
    f.worker.step().await.unwrap();
    assert_eq!(names(f, &tenant).await, before);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn active_group_names_stay_unique_within_an_organization_only() {
    let w = world().await;
    let f = &w.f;
    let first = organization(&w.f, "first").await;
    let second = organization(&w.f, "second").await;
    let a = group(f, &first, &w.service, &w.owner, "shared").await;
    let b = group(f, &first, &w.service, &w.owner, "other").await;
    // Duplicate create and duplicate rename are rejected without a committed change.
    f.error("owner","SetGroup",json!({"requestKey":key(),"tenantId":first,"group":{"name":"shared","userIds":[],"permissions":[]}}),"already_exists").await;
    f.error("owner","SetGroup",json!({"requestKey":key(),"tenantId":first,"group":{"id":b,"name":"shared","userIds":[],"permissions":[]}}),"already_exists").await;
    assert_eq!(
        names(f, &first)
            .await
            .iter()
            .map(|(_, n)| n.as_str())
            .filter(|n| *n == "shared")
            .count(),
        1
    );
    assert!(
        names(f, &first)
            .await
            .contains(&(b.clone(), "other".into()))
    );
    // The same name in a different organization is allowed.
    group(f, &second, &w.service, &w.owner, "shared").await;
    assert!(names(f, &first).await.contains(&(a, "shared".into())));
    // Group permissions still authorize members before deletion.
    let allowed = f
        .rpc(
            "service",
            "Authorize",
            json!({"tenantId":first,"userId":w.owner,"capability":"execute","paid":false}),
        )
        .await;
    assert_eq!(allowed["allowed"], true);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn deferrable_name_constraint_is_checked_at_statement_end_and_rolls_back() {
    let w = world().await;
    let f = &w.f;
    let (tenant, ids) = cyclic_organization(&w, "sql", 3).await;
    let state: (bool, bool) = sqlx::query_as("SELECT condeferrable,condeferred FROM pg_constraint WHERE conname='groups_tenant_id_name_key' AND conrelid='groups'::regclass")
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(state, (true, false), "deferrable but initially immediate");
    let strict: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_constraint WHERE conrelid='groups'::regclass AND contype IN ('p','u') AND NOT condeferrable")
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(
        strict, 2,
        "primary key and (tenant_id,id) key stay immediate"
    );
    // A statement that makes two final names equal is rejected at statement end, and the
    // enclosing transaction (including an earlier marker write) rolls back.
    let before = names(f, &tenant).await;
    let mut tx = f.db.begin().await.unwrap();
    sqlx::query("UPDATE tenants SET name='marker' WHERE id=$1")
        .bind(uid(&tenant))
        .execute(&mut *tx)
        .await
        .unwrap();
    let error = sqlx::query("UPDATE groups SET name='same' WHERE tenant_id=$1")
        .bind(uid(&tenant))
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23505")
    );
    drop(tx);
    assert_eq!(names(f, &tenant).await, before);
    let marker = count(
        f,
        "SELECT count(*) FROM tenants WHERE id=$1 AND name='marker'",
        &tenant,
    )
    .await;
    assert_eq!(
        marker, 0,
        "the marker write must roll back with the failed statement"
    );
    // The anonymizing statement succeeds for every row order and commits atomically.
    let mut tx = f.db.begin().await.unwrap();
    sqlx::query("UPDATE tenants SET name='marker' WHERE id=$1")
        .bind(uid(&tenant))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE groups SET name=id::text WHERE tenant_id=$1")
        .bind(uid(&tenant))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for (id, name) in names(f, &tenant).await {
        assert_eq!(id, name);
    }
    assert_eq!(names(f, &tenant).await.len(), ids.len());
    assert_eq!(
        count(
            f,
            "SELECT count(*) FROM tenants WHERE id=$1 AND name='marker'",
            &tenant
        )
        .await,
        1
    );
}

/// Applies migrations 1-13 to an empty schema by hand-built history, so the upgrade path from
/// the previous released schema is exercised with populated, crossed rows.
#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn schema_13_database_with_crossed_names_upgrades_in_place() {
    let url = std::env::var("TEST_DATABASE_URL").expect("requires disposable TEST_DATABASE_URL");
    let schema = format!("delcore_{}", Uuid::now_v7().simple());
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
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
    let migrations = [
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
    ];
    sqlx::query("CREATE TABLE delcore_schema(version integer PRIMARY KEY,digest text NOT NULL,applied_at timestamptz NOT NULL)").execute(&db).await.unwrap();
    for (index, sql) in migrations.iter().enumerate() {
        sqlx::raw_sql(sql).execute(&db).await.unwrap();
        let digest = hex::encode(sha2::Sha256::digest(sql.as_bytes()));
        sqlx::query("INSERT INTO delcore_schema VALUES($1,$2,now())")
            .bind(index as i32 + 1)
            .bind(digest)
            .execute(&db)
            .await
            .unwrap();
    }
    let old: (bool,) = sqlx::query_as("SELECT condeferrable FROM pg_constraint WHERE conname='groups_tenant_id_name_key' AND conrelid='groups'::regclass")
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(!old.0, "schema 13 constraint is immediate");
    assert!(delcore_api::postgres::check_schema(&db).await.is_err());
    // Populate: an organization with a crossed pair, a member, a group member and a permission.
    let (user, plan, tenant, service) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    for (sql, id) in [
        (
            "INSERT INTO users(id,issuer,subject_hash,email) VALUES($1,'i','s','u@example.test')",
            user,
        ),
        (
            "INSERT INTO plans(id,name,rank,free,postpaid) VALUES($1,'free',1,true,false)",
            plan,
        ),
        (
            "INSERT INTO services(id,name,client_id,capabilities,scopes) VALUES($1,'svc','svc',ARRAY['execute'],ARRAY['authorize'])",
            service,
        ),
    ] {
        sqlx::query(sql).bind(id).execute(&db).await.unwrap();
    }
    sqlx::query("INSERT INTO tenants(id,name,personal,owner_id,plan_id,period_start,period_end) VALUES($1,'t',false,$2,$3,now(),now()+interval '1 month')")
        .bind(tenant).bind(user).bind(plan).execute(&db).await.unwrap();
    sqlx::query("INSERT INTO members(tenant_id,user_id,roles) VALUES($1,$2,ARRAY[1])")
        .bind(tenant)
        .bind(user)
        .execute(&db)
        .await
        .unwrap();
    for (id, name) in [(a, b), (b, a)] {
        sqlx::query("INSERT INTO groups VALUES($1,$2,$3::text)")
            .bind(id)
            .bind(tenant)
            .bind(name.to_string())
            .execute(&db)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO group_members VALUES($1,$2,$3)")
        .bind(tenant)
        .bind(a)
        .bind(user)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO group_permissions VALUES($1,$2,'execute')")
        .bind(a)
        .bind(service)
        .execute(&db)
        .await
        .unwrap();
    let crossed = sqlx::query("UPDATE groups SET name=id::text WHERE tenant_id=$1")
        .bind(tenant)
        .execute(&db)
        .await
        .unwrap_err();
    assert_eq!(
        crossed.as_database_error().unwrap().code().as_deref(),
        Some("23505"),
        "schema 13 rejects the permutation"
    );
    delcore_api::postgres::migrate(&db).await.unwrap();
    delcore_api::postgres::migrate(&db).await.unwrap();
    delcore_api::postgres::check_schema(&db).await.unwrap();
    let versions: Vec<i32> =
        sqlx::query_scalar("SELECT version FROM delcore_schema ORDER BY version")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(versions, (1..=14).collect::<Vec<_>>());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM groups WHERE tenant_id=$1")
        .bind(tenant)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(rows, 2, "existing rows survive");
    let relations: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM group_members)+(SELECT count(*) FROM group_permissions)",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(relations, 2, "references survive");
    sqlx::query("UPDATE groups SET name=id::text WHERE tenant_id=$1")
        .bind(tenant)
        .execute(&db)
        .await
        .unwrap();
    db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
