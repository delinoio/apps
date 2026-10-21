// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Fixture, key};
use serde_json::json;

async fn tenant(f: &Fixture, name: &str) -> uuid::Uuid {
    let response = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":name}),
        )
        .await;
    response["tenant"]["id"].as_str().unwrap().parse().unwrap()
}

async fn job(
    f: &Fixture,
    tenant: uuid::Uuid,
    kind: &str,
    state: &str,
    attempts: i32,
    external: &str,
    category: &str,
) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,state,attempts,external_id,error_category,available_at,created_at,updated_at) VALUES($1,$2,$3,$1::uuid::text,'{\"amount\":\"10\",\"minor_units\":\"1000\",\"currency\":\"USD\"}',$4,$5,$6,$7,now(),now(),now())")
        .bind(id).bind(tenant).bind(kind).bind(state).bind(attempts).bind(external).bind(category).execute(&f.db).await.unwrap();
    id
}

async fn state(f: &Fixture, id: uuid::Uuid) -> (String, String) {
    sqlx::query_as("SELECT state,error_category FROM jobs WHERE id=$1")
        .bind(id)
        .fetch_one(&f.db)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn disabling_auto_topup_stops_only_unattempted_tenant_charges() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let target = tenant(&f, "Target").await;
    let other = tenant(&f, "Other").await;
    sqlx::query("UPDATE tenants SET auto_threshold=5,auto_amount=10,auto_cap=10 WHERE id=$1")
        .bind(target)
        .execute(&f.db)
        .await
        .unwrap();
    let queued = job(&f, target, "auto_topup", "pending", 0, "", "").await;
    let manual = job(&f, target, "topup", "pending", 0, "", "").await;
    let unrelated = job(&f, other, "auto_topup", "pending", 0, "", "").await;
    let disable = json!({"requestKey":key(),"tenantId":target,"enabled":false});
    f.rpc("owner", "SetAutoTopUp", disable.clone()).await;
    f.rpc("owner", "SetAutoTopUp", disable).await;
    f.rpc(
        "owner",
        "SetAutoTopUp",
        json!({"requestKey":key(),"tenantId":target,"enabled":false}),
    )
    .await;
    let deferred = job(
        &f,
        target,
        "auto_topup",
        "pending",
        0,
        "",
        "provider_billing_window",
    )
    .await;
    f.rpc(
        "owner",
        "SetAutoTopUp",
        json!({"requestKey":key(),"tenantId":target,"enabled":false}),
    )
    .await;
    for id in [queued, deferred] {
        assert_eq!(
            state(&f, id).await,
            ("done".into(), "auto_topup_disabled".into())
        );
    }
    for id in [manual, unrelated] {
        assert_eq!(state(&f, id).await, ("pending".into(), String::new()));
    }
    let cleared: bool = sqlx::query_scalar("SELECT auto_threshold IS NULL AND auto_amount IS NULL AND auto_cap IS NULL FROM tenants WHERE id=$1").bind(target).fetch_one(&f.db).await.unwrap();
    assert!(cleared);
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn disabling_auto_topup_preserves_attempted_and_operator_retried_charges() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let mut preserved = Vec::new();
    for (initial, attempts, external) in [
        ("running", 1, ""),
        ("reconcile", 1, ""),
        ("pending", 1, ""),
        ("pending", 0, "txn_known"),
    ] {
        let target = tenant(&f, "Attempted charge").await;
        let id = job(&f, target, "auto_topup", initial, attempts, external, "").await;
        f.rpc(
            "owner",
            "SetAutoTopUp",
            json!({"requestKey":key(),"tenantId":target,"enabled":false}),
        )
        .await;
        preserved.push((id, initial));
    }
    let target = tenant(&f, "Retried charge").await;
    let retried = job(
        &f,
        target,
        "auto_topup",
        "failed",
        3,
        "",
        "provider_rejected",
    )
    .await;
    f.rpc(
        "owner",
        "RetryJob",
        json!({"requestKey":key(),"jobId":retried,"reason":"provider recovered"}),
    )
    .await;
    let attempts: i32 = sqlx::query_scalar("SELECT attempts FROM jobs WHERE id=$1")
        .bind(retried)
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(attempts, 0);
    f.rpc(
        "owner",
        "SetAutoTopUp",
        json!({"requestKey":key(),"tenantId":target,"enabled":false}),
    )
    .await;
    for (id, initial) in preserved {
        assert_eq!(state(&f, id).await, (initial.into(), String::new()));
    }
    assert_eq!(
        state(&f, retried).await,
        ("pending".into(), "provider_rejected".into())
    );
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn free_postpaid_plans_are_rejected_without_catalog_or_audit_changes() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    async fn snapshot(f: &Fixture) -> (String, i64, i64) {
        sqlx::query_as("SELECT (SELECT id::text FROM plans WHERE is_default),(SELECT count(*) FROM plans),(SELECT count(*) FROM audit WHERE action='catalog.plan')").fetch_one(&f.db).await.unwrap()
    }
    let before = snapshot(&f).await;
    for default_free in [false, true] {
        let error = f.error("owner","PutPlan",json!({"requestKey":key(),"reason":"reject free postpaid","plan":{"name":"free postpaid","rank":1,"free":true,"defaultFree":default_free,"postpaid":true}}),"invalid_argument").await;
        assert!(error["message"].as_str().unwrap().contains("postpaid"));
        assert_eq!(snapshot(&f).await, before);
    }
    for (name, rank, free, postpaid) in [
        ("free prepaid", 1, true, false),
        ("paid postpaid", 2, false, true),
        ("paid prepaid", 3, false, false),
    ] {
        f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"accept valid billing policy","plan":{"name":name,"rank":rank,"free":free,"postpaid":postpaid}})).await;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM plans WHERE postpaid AND NOT free")
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}
