// SPDX-License-Identifier: FSL-1.1-ALv2
//! Verifies that a malformed successful Delcore membership reply never authorizes a login
//! token. The tenant-targeted path never reaches the database, so the pool is lazy and
//! points at an unreachable address.
use axum::{Json, Router, http::StatusCode, routing::post};
use connectrpc::ErrorCode;
use devbase_api::{Op, Server, config::Config};
use serde_json::{Value, json};
use server_common::authority::Authority;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const ACTOR: &str = "01900000-0000-7000-8000-000000000001";
const TENANT: &str = "01900000-0000-7000-8000-000000000002";

async fn server(members: Arc<Mutex<String>>) -> Server {
    let mock = Router::new()
        .route(
            "/token",
            post(|| async { Json(json!({"access_token":"service","expires_in":3600})) }),
        )
        .route(
            "/delcore.v1.DelcoreService/GetCurrentUser",
            post(|| async { Json(json!({"userId":ACTOR})) }),
        )
        .route(
            "/delcore.v1.DelcoreService/GetTenantStates",
            post(|| async {
                Json(json!({"states":[{"tenantId":TENANT,"state":"TENANT_STATE_ACTIVE"}]}))
            }),
        )
        .route(
            "/delcore.v1.DelcoreService/ListMembers",
            post(move || {
                let body = members.lock().unwrap().clone();
                async move { (StatusCode::OK, [("content-type", "application/json")], body) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let authority = Authority::new(
        reqwest::Client::new(),
        base.clone(),
        format!("{base}/token"),
        "resource".into(),
        "client".into(),
        "fixture".into(),
    );
    let db = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://postgres@127.0.0.1:1/unreachable")
        .unwrap();
    let config = Config {
        projects: 100,
        users: 100,
        keys: 20,
        user_rate: 10000.0,
        user_burst: 20000,
        management_rate: 10000.0,
        management_burst: 20000,
        dependency_timeout: Duration::from_secs(3),
        request_timeout: Duration::from_secs(10),
        cleanup_interval: Duration::from_secs(300),
        origin: "https://console.example".into(),
        listen: "127.0.0.1:0".into(),
    };
    Server::new(db, authority, config)
}

#[tokio::test]
async fn malformed_listmembers_reply_does_not_authorize_login_tokens() {
    let members = Arc::new(Mutex::new(String::new()));
    let server = server(members.clone()).await;
    let query: Value = json!({"tenantId":TENANT});
    for body in [
        "{}",
        r#"{"members":[]}"#,
        r#"{"members":null}"#,
        r#"{"members":[{"userId":"01900000-0000-7000-8000-000000000001","roles":["ROLE_OWNER"]}]}"#,
    ] {
        *members.lock().unwrap() = body.into();
        let identity = server
            .authenticate("Bearer login", Op::ListProjects, &query)
            .await
            .unwrap_or_else(|e| panic!("{body} must authorize: {e:?}"));
        assert_eq!(identity.actor, format!("user:{ACTOR}"));
        assert_eq!(identity.tenant, TENANT);
    }
    for body in [
        "[]",
        "42",
        "true",
        "null",
        r#"{"members":42}"#,
        r#"{"members":[null]}"#,
        r#"{"members":[42]}"#,
        r#"{"nextAfterId":42}"#,
    ] {
        *members.lock().unwrap() = body.into();
        let Err(fault) = server
            .authenticate("Bearer login", Op::ListProjects, &query)
            .await
        else {
            panic!("{body} must not authorize");
        };
        assert_eq!(fault.code, ErrorCode::Unavailable, "{body}");
        assert_eq!(fault.reason, 9, "{body}");
    }
}
