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

/// Starts a counting provider mock and returns a worker bound to it, so cancellation tests assert
/// on the provider requests actually sent instead of reaching the sandbox URL from `.env.example`.
async fn mock_worker(
    f: &Fixture,
) -> (
    delcore_api::worker::Worker,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
        let recorded = recorded.clone();
        async move {
            recorded.lock().unwrap().push(uri.path().to_string());
            axum::Json(json!({"data":{"id":"sub_fixture","status":"active"}}))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let worker = delcore_api::worker::Worker::new(
        f.db.clone(),
        delcore_api::config::Config {
            paddle_url: url.clone(),
            email_url: url,
            ..f.config.clone()
        },
    )
    .unwrap();
    (worker, seen, task)
}

/// Creates a deleting tenant that still owns a provider subscription, past reporting grace, with a
/// queued deletion cancel job.
async fn deleting_tenant(f: &Fixture, name: &str) -> (uuid::Uuid, uuid::Uuid) {
    let tenant = tenant(f, name).await;
    sqlx::query("UPDATE tenants SET external_subscription_id='sub_fixture',deleting_at=now()-interval '8 days' WHERE id=$1")
        .bind(tenant).execute(&f.db).await.unwrap();
    let cancel = job(f, tenant, "cancel", "pending", 0, "", "").await;
    (tenant, cancel)
}

async fn metering_job(f: &Fixture, tenant: uuid::Uuid, state: &str, future: bool) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,state,attempts,error_category,available_at,created_at,updated_at) VALUES($1,$2,'metering',$1::text,'{}',$3,0,$4,CASE WHEN $5 THEN now()+interval '7 days' ELSE now() END,now(),now())")
        .bind(id).bind(tenant).bind(state).bind(if state == "failed" { "retry_exhausted" } else { "" }).bind(future).execute(&f.db).await.unwrap();
    id
}

async fn attempts(f: &Fixture, id: uuid::Uuid) -> i32 {
    sqlx::query_scalar("SELECT attempts FROM jobs WHERE id=$1")
        .bind(id)
        .fetch_one(&f.db)
        .await
        .unwrap()
}

fn cancel_requests(seen: &std::sync::Mutex<Vec<String>>) -> usize {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|path| path.ends_with("/cancel"))
        .count()
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn deleting_cancellation_waits_for_unfinished_metering_work() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let (worker, seen, mock) = mock_worker(&f).await;
    for (kind, future) in [("failed", false), ("pending", true)] {
        let (tenant, cancel) = deleting_tenant(&f, "Deferred cancel").await;
        let metering = metering_job(&f, tenant, kind, future).await;
        for _ in 0..3 {
            worker.step().await.unwrap();
        }
        assert_eq!(
            state(&f, cancel).await,
            ("pending".into(), "final_settlement_pending".into()),
            "{kind} metering must defer cancellation"
        );
        assert_eq!(
            attempts(&f, cancel).await,
            0,
            "deferral keeps the retry budget"
        );
        assert_eq!(
            state(&f, metering).await.0,
            kind,
            "metering work is untouched"
        );
        assert_eq!(cancel_requests(&seen), 0, "no provider cancel request");
    }
    // Once the metering work is done and nothing else is outstanding, cancellation proceeds.
    let (tenant, cancel) = deleting_tenant(&f, "Released cancel").await;
    let metering = metering_job(&f, tenant, "failed", false).await;
    sqlx::query("UPDATE jobs SET state='done',error_category='' WHERE id=$1")
        .bind(metering)
        .execute(&f.db)
        .await
        .unwrap();
    let other = tenant_other_unfinished(&f).await;
    for _ in 0..3 {
        worker.step().await.unwrap();
    }
    assert_eq!(
        state(&f, other).await.0,
        "failed",
        "other tenant work is untouched"
    );
    assert_eq!(
        state(&f, cancel).await,
        ("reconcile".into(), "awaiting_provider_event".into())
    );
    assert_eq!(cancel_requests(&seen), 1);
    mock.abort();
}

async fn tenant_other_unfinished(f: &Fixture) -> uuid::Uuid {
    let other = tenant(f, "Other tenant").await;
    metering_job(f, other, "failed", false).await
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn deleting_cancellation_still_waits_for_settlement_after_metering() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let (worker, seen, mock) = mock_worker(&f).await;
    let (tenant, cancel) = deleting_tenant(&f, "Settlement wait").await;
    sqlx::query("INSERT INTO ledger(id,tenant_id,kind,amount,period_start,period_end,source_key,created_at) VALUES($1,$2,'postpaid',-10,now()-interval '40 days',now()-interval '10 days','debt-fixture',now())")
        .bind(uuid::Uuid::now_v7()).bind(tenant).execute(&f.db).await.unwrap();
    let metering = metering_job(&f, tenant, "done", false).await;
    for _ in 0..3 {
        worker.step().await.unwrap();
    }
    assert_eq!(
        state(&f, cancel).await,
        ("pending".into(), "final_settlement_pending".into())
    );
    assert_eq!(state(&f, metering).await.0, "done");
    assert_eq!(
        cancel_requests(&seen),
        0,
        "postpaid debt defers cancellation"
    );
    mock.abort();
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn ordinary_cancellation_ignores_unfinished_metering_work() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let (worker, seen, mock) = mock_worker(&f).await;
    let (tenant, cancel) = deleting_tenant(&f, "Ordinary cancel").await;
    sqlx::query("UPDATE tenants SET deleting_at=NULL WHERE id=$1")
        .bind(tenant)
        .execute(&f.db)
        .await
        .unwrap();
    metering_job(&f, tenant, "failed", false).await;
    for _ in 0..3 {
        worker.step().await.unwrap();
    }
    assert_eq!(
        state(&f, cancel).await,
        ("reconcile".into(), "awaiting_provider_event".into())
    );
    assert_eq!(cancel_requests(&seen), 1);
    mock.abort();
}
