// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Fixture, key};
use serde_json::json;
use uuid::Uuid;

async fn tenant(f: &Fixture) -> String {
    f.bootstrap().await;
    f.rpc(
        "owner",
        "CreateTenant",
        json!({"requestKey":key(),"name":"Billing fixture"}),
    )
    .await["tenant"]["id"]
        .as_str()
        .unwrap()
        .into()
}
async fn job(f: &Fixture, tenant: &str, kind: &str, category: &str) -> String {
    let id = key();
    sqlx::query("INSERT INTO jobs(id,tenant_id,kind,state,attempts,error_category,dedup_key,payload,available_at,created_at,updated_at) VALUES($1,$2,$3,'failed',3,$4,$5,'{}',now(),now(),now())")
        .bind(Uuid::parse_str(&id).unwrap()).bind(Uuid::parse_str(tenant).unwrap()).bind(kind).bind(category).bind(format!("fixture:{id}")).execute(&f.db).await.unwrap();
    id
}
async fn state(f: &Fixture, job: &str) -> String {
    sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
        .bind(Uuid::parse_str(job).unwrap())
        .fetch_one(&f.db)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn unknown_payments_reconcile_on_retry_and_block_deletion() {
    let f = Fixture::setup().await;
    let tenant = tenant(&f).await;
    let uncertain = job(&f, &tenant, "subscription", "unknown_payment_outcome").await;
    f.error(
        "owner",
        "DeleteTenant",
        json!({"requestKey":key(),"tenantId":tenant}),
        "failed_precondition",
    )
    .await;
    f.rpc(
        "owner",
        "RetryJob",
        json!({"requestKey":key(),"jobId":uncertain,"reason":"reconcile unknown payment"}),
    )
    .await;
    assert_eq!(state(&f, &uncertain).await, "reconcile");
    let rejected = job(&f, &tenant, "topup", "provider_rejected").await;
    f.rpc(
        "owner",
        "RetryJob",
        json!({"requestKey":key(),"jobId":rejected,"reason":"retry confirmed rejection"}),
    )
    .await;
    assert_eq!(state(&f, &rejected).await, "pending");
    f.error(
        "owner",
        "RetryJob",
        json!({"requestKey":key(),"jobId":rejected,"reason":"duplicate operator retry"}),
        "failed_precondition",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn topups_require_exact_minor_units_and_preserve_deletion_debt_gate() {
    let f = Fixture::setup().await;
    let tenant = tenant(&f).await;
    for amount in ["0", "-1", "0.001", "not-decimal"] {
        f.error("owner", "RequestTopUp", json!({"requestKey":key(),"tenantId":tenant,"amount":amount,"returnUrl":f.config.return_urls[0]}), "invalid_argument").await;
    }
    let request = json!({"requestKey":key(),"tenantId":tenant,"amount":"10.00","returnUrl":f.config.return_urls[0]});
    let response = f.rpc("owner", "RequestTopUp", request.clone()).await;
    let replay = f.rpc("owner", "RequestTopUp", request).await;
    assert_eq!(response["jobId"], replay["jobId"]);
    assert_ne!(
        response["metadata"]["correlationId"],
        replay["metadata"]["correlationId"]
    );
    let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM jobs WHERE id=$1")
        .bind(Uuid::parse_str(response["jobId"].as_str().unwrap()).unwrap())
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(payload["amount"], "10");
    assert_eq!(payload["minor_units"], "1000");
    sqlx::query("UPDATE tenants SET deleting_at=now() WHERE id=$1")
        .bind(Uuid::parse_str(&tenant).unwrap())
        .execute(&f.db)
        .await
        .unwrap();
    f.error("owner", "RequestTopUp", json!({"requestKey":key(),"tenantId":tenant,"amount":"10","returnUrl":f.config.return_urls[0]}), "failed_precondition").await;
}
