// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use buffa::Message;
use devbase_api::{Server, config::Config, store};
use serde_json::{Value, json};
use server_common::authority::Authority;
use sqlx::Row;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn postgres_wire_authorization_replays_and_deletion() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("devbase_{}", uuid::Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
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
    store::migrate(&db).await.unwrap();
    store::migrate(&db).await.unwrap();
    store::check(&db).await.unwrap();
    let tenant = uuid::Uuid::now_v7().to_string();
    let actor = uuid::Uuid::now_v7().to_string();
    let state = Arc::new(Mutex::new("TENANT_STATE_ACTIVE".to_owned()));
    let state_mock = state.clone();
    let actor_mock = actor.clone();
    let mock = Router::new()
        .route(
            "/token",
            post(|| async { Json(json!({"access_token":"service","expires_in":3600})) }),
        )
        .route(
            "/delcore.v1.DelcoreService/GetCurrentUser",
            post(move |h: HeaderMap| {
                let actor = actor_mock.clone();
                async move {
                    if h.get("authorization").and_then(|h| h.to_str().ok())
                        != Some("Bearer manager")
                    {
                        return (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({"code":"unauthenticated"})),
                        );
                    }
                    (StatusCode::OK, Json(json!({"userId":actor})))
                }
            }),
        )
        .route(
            "/delcore.v1.DelcoreService/ListMembers",
            post(|h: HeaderMap| async move {
                if h.get("authorization").and_then(|h| h.to_str().ok()) == Some("Bearer manager") {
                    (StatusCode::OK, Json(json!({})))
                } else {
                    (
                        StatusCode::FORBIDDEN,
                        Json(json!({"code":"permission_denied"})),
                    )
                }
            }),
        )
        .route(
            "/delcore.v1.DelcoreService/GetTenantStates",
            post(move |Json(q): Json<Value>| {
                let state = state_mock.clone();
                async move {
                    if *state.lock().unwrap() == "unavailable" {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({"code":"unavailable"})),
                        );
                    }
                    let entries: Vec<_> = q["tenantIds"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|id| json!({"tenantId":id,"state":*state.lock().unwrap()}))
                        .collect();
                    (StatusCode::OK, Json(json!({"states":entries})))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mock_task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
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
    let http = reqwest::Client::new();
    let authority = Authority::new(
        http.clone(),
        base.clone(),
        format!("{base}/token"),
        "resource".into(),
        "client".into(),
        "fixture".into(),
    );
    let server = Arc::new(Server::new(db.clone(), authority, config));
    let router = devbase_api::http::router(server.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!(
        "http://{}/devbase.v1.DevBaseService",
        listener.local_addr().unwrap()
    );
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let root = base.strip_suffix("/devbase.v1.DevBaseService").unwrap();
    for path in ["/healthz", "/readyz", "/metrics"] {
        for origin in ["https://console.example", "https://other.example"] {
            let response = http
                .get(format!("{root}{path}"))
                .header("Origin", origin)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                !response
                    .headers()
                    .contains_key("access-control-allow-origin")
            );
        }
    }
    let preflight = http
        .request(reqwest::Method::OPTIONS, format!("{base}/GetProject"))
        .header("Origin", "https://console.example")
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        preflight.headers()["access-control-allow-origin"],
        "https://console.example"
    );
    let rejected = http
        .post(format!("{base}/GetProject"))
        .header("Origin", "https://other.example")
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    async fn call(
        http: &reqwest::Client,
        base: &str,
        method: &str,
        token: &str,
        q: Value,
    ) -> (StatusCode, Value) {
        let r = http
            .post(format!("{base}/{method}"))
            .bearer_auth(token)
            .header("Connect-Protocol-Version", "1")
            .json(&q)
            .send()
            .await
            .unwrap();
        let status = r.status();
        let body: Value = r.json().await.unwrap();
        (status, body)
    }
    // Returns the status, the trace header and the body so typed error details can be asserted.
    async fn call_traced(
        http: &reqwest::Client,
        base: &str,
        method: &str,
        token: &str,
        q: Value,
    ) -> (StatusCode, Option<String>, Value) {
        let r = http
            .post(format!("{base}/{method}"))
            .bearer_auth(token)
            .header("Connect-Protocol-Version", "1")
            .json(&q)
            .send()
            .await
            .unwrap();
        let status = r.status();
        let trace = r
            .headers()
            .get("X-Devbase-Trace-Id")
            .map(|v| v.to_str().unwrap().to_owned());
        (status, trace, r.json().await.unwrap())
    }
    // Counts the rows a rejected request must not change.
    async fn counts(db: &sqlx::PgPool) -> (i64, i64, i64, i64) {
        let mut out = [0i64; 4];
        for (slot, table) in
            out.iter_mut()
                .zip(["replays", "audit_events", "end_users", "projects"])
        {
            *slot = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(db)
                .await
                .unwrap();
        }
        (out[0], out[1], out[2], out[3])
    }
    // Asserts that every request is answered with the expected code, a typed ErrorDetail and the
    // trace header, and that none of them changes stored rows.
    async fn expect_all(
        http: &reqwest::Client,
        base: &str,
        db: &sqlx::PgPool,
        method: &str,
        token: &str,
        code: &str,
        requests: Vec<Value>,
    ) {
        let before = counts(db).await;
        for q in requests {
            let (status, trace, body) = call_traced(http, base, method, token, q.clone()).await;
            assert_eq!(body["code"], code, "{method} {q} -> {status} {body}");
            assert!(
                !body["details"].as_array().unwrap().is_empty(),
                "{method} {q} -> {body}"
            );
            assert!(trace.is_some(), "{method} {q} lacks the trace header");
        }
        assert_eq!(
            counts(db).await,
            before,
            "{method} {code} changed stored rows"
        );
    }
    let create = json!({"tenantId":tenant,"name":"Project","idempotencyKey":"project-create"});
    let (status, p) = call(&http, &base, "CreateProject", "manager", create.clone()).await;
    assert_eq!(status, StatusCode::OK, "{p}");
    let pid = p["project"]["id"].as_str().unwrap().to_owned();
    assert!(server_common::canonical_id(&pid));
    let (_, replay) = call(&http, &base, "CreateProject", "manager", create).await;
    assert_eq!(p["project"], replay["project"]);
    let (_, conflict) = call(
        &http,
        &base,
        "CreateProject",
        "manager",
        json!({"tenantId":tenant,"name":"Other","idempotencyKey":"project-create"}),
    )
    .await;
    assert_eq!(conflict["code"], "already_exists");
    assert!(!conflict["details"].as_array().unwrap().is_empty());
    let (_,keys)=call(&http,&base,"IssueApiKey","manager",json!({"projectId":pid,"permission":"API_KEY_PERMISSION_CRUD","idempotencyKey":"key-issue"})).await;
    let secret = keys["secret"].as_str().unwrap().to_owned();
    let key_id = keys["apiKey"]["id"].as_str().unwrap().to_owned();
    let (_,again)=call(&http,&base,"IssueApiKey","manager",json!({"projectId":pid,"permission":"API_KEY_PERMISSION_CRUD","idempotencyKey":"key-issue"})).await;
    assert!(again.get("secret").is_none());
    assert_eq!(again["apiKey"], keys["apiKey"]);
    let create = json!({"projectId":pid,"externalId":"external","metadata":{"z":1,"a":{"y":2,"b":[null,true,3]}},"idempotencyKey":"user-create"});
    let (a, b) = tokio::join!(
        call(&http, &base, "CreateEndUser", &secret, create.clone()),
        call(&http, &base, "CreateEndUser", &secret, create.clone())
    );
    assert_eq!(a.0, StatusCode::OK, "{}", a.1);
    assert_eq!(a.1["endUser"], b.1["endUser"]);
    let uid = a.1["endUser"]["id"].as_str().unwrap().to_owned();
    // Replays must remain stable across randomized protobuf map iteration.
    for _ in 0..10 {
        let (status, v) = call(&http, &base, "CreateEndUser", &secret, create.clone()).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["endUser"]["id"], uid);
    }
    let (_, lookup) = call(
        &http,
        &base,
        "LookupEndUser",
        &secret,
        json!({"projectId":pid,"externalId":"external"}),
    )
    .await;
    assert_eq!(lookup["endUser"]["id"], uid);
    let (_,updated)=call(&http,&base,"UpdateEndUser",&secret,json!({"projectId":pid,"endUserId":uid,"name":"Updated","updateMask":"name","version":"1","idempotencyKey":"user-update"})).await;
    assert_eq!(updated["endUser"]["version"], "2");
    // An active (not scrubbed) replay keeps payload validation and ordinary conflict handling.
    let long_name = "a".repeat(257);
    let large_metadata = json!({"k":"v".repeat(20_000)});
    expect_all(&http, &base, &db, "CreateEndUser", &secret, "invalid_argument", vec![
        json!({"projectId":pid,"externalId":"","idempotencyKey":"user-create"}),
        json!({"projectId":pid,"externalId":"external","name":long_name,"idempotencyKey":"user-create"}),
        json!({"projectId":pid,"externalId":"external","metadata":large_metadata,"idempotencyKey":"user-create"}),
        // A fresh key with an invalid payload never reaches storage.
        json!({"projectId":pid,"externalId":"","idempotencyKey":"user-fresh-invalid"}),
        json!({"projectId":pid,"externalId":"external","idempotencyKey":"short"}),
    ]).await;
    expect_all(
        &http,
        &base,
        &db,
        "CreateEndUser",
        &secret,
        "already_exists",
        vec![json!({"projectId":pid,"externalId":"changed","idempotencyKey":"user-create"})],
    )
    .await;
    expect_all(&http, &base, &db, "UpdateEndUser", &secret, "invalid_argument", vec![
        json!({"projectId":pid,"endUserId":uid,"name":"Updated","updateMask":"","version":"1","idempotencyKey":"user-update"}),
        json!({"projectId":pid,"endUserId":uid,"name":"Updated","updateMask":"unknown","version":"1","idempotencyKey":"user-update"}),
    ]).await;
    let (_, active_update) = call(&http, &base, "UpdateEndUser", &secret, json!({"projectId":pid,"endUserId":uid,"name":"Updated","updateMask":"name","version":"1","idempotencyKey":"user-update"})).await;
    assert_eq!(active_update["endUser"], updated["endUser"]);
    // Metadata that PostgreSQL jsonb cannot store is invalid input. It must be
    // rejected before any mutation and leave counters, replays and audits alone.
    async fn snapshot(db: &sqlx::PgPool, pid: &str, uid: &str) -> (i64, i64, i64, i64, i64, Value) {
        let pid = uuid::Uuid::parse_str(pid).unwrap();
        let uid = uuid::Uuid::parse_str(uid).unwrap();
        let count = |sql: &'static str| async move {
            sqlx::query_scalar::<_, i64>(sql)
                .fetch_one(db)
                .await
                .unwrap()
        };
        let user_count: i64 = sqlx::query_scalar("SELECT user_count FROM projects WHERE id=$1")
            .bind(pid)
            .fetch_one(db)
            .await
            .unwrap();
        let (version, metadata): (i64, Value) =
            sqlx::query_as("SELECT version,metadata FROM end_users WHERE id=$1")
                .bind(uid)
                .fetch_one(db)
                .await
                .unwrap();
        (
            count("SELECT count(*) FROM end_users").await,
            user_count,
            count("SELECT count(*) FROM replays").await,
            count("SELECT count(*) FROM audit_events").await,
            version,
            metadata,
        )
    }
    let nul_fixtures = [
        json!({"value":"\u{0}"}),
        json!({"\u{0}":"value"}),
        json!({"nested":{"value":"a\u{0}b"}}),
        json!({"nested":{"a\u{0}b":true}}),
        json!({"items":[0,{"value":"\u{0}"}]}),
        json!({"items":[{"\u{0}":null}]}),
    ];
    let before = snapshot(&db, &pid, &uid).await;
    for (n, fixture) in nul_fixtures.iter().enumerate() {
        let create_nul = json!({"projectId":pid,"externalId":format!("nul-{n}"),"metadata":fixture,"idempotencyKey":format!("nul-create-{n}")});
        let (status, body) = call(&http, &base, "CreateEndUser", &secret, create_nul).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{fixture}: {body}");
        assert_eq!(body["code"], "invalid_argument", "{fixture}: {body}");
        assert!(!body["details"].as_array().unwrap().is_empty());
        assert_eq!(body["message"], "metadata must not contain U+0000");
        let update_nul = json!({"projectId":pid,"endUserId":uid,"metadata":fixture,"updateMask":"metadata","version":"2","idempotencyKey":format!("nul-update-{n}")});
        let (status, body) = call(&http, &base, "UpdateEndUser", &secret, update_nul).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{fixture}: {body}");
        assert_eq!(body["code"], "invalid_argument", "{fixture}: {body}");
        assert!(!body["details"].as_array().unwrap().is_empty());
        assert_eq!(body["message"], "metadata must not contain U+0000");
        assert_eq!(snapshot(&db, &pid, &uid).await, before, "{fixture}");
    }
    let trace = http
        .post(format!("{base}/CreateEndUser"))
        .bearer_auth(&secret)
        .header("Connect-Protocol-Version", "1")
        .json(&json!({"projectId":pid,"externalId":"nul-trace","metadata":nul_fixtures[0],"idempotencyKey":"nul-trace"}))
        .send()
        .await
        .unwrap();
    assert_eq!(trace.status(), StatusCode::BAD_REQUEST);
    assert!(trace.headers().contains_key("x-devbase-trace-id"));
    // Unmasked metadata is not validated: a name-only update ignores it.
    let (status, name_only) = call(&http, &base, "UpdateEndUser", &secret, json!({"projectId":pid,"endUserId":uid,"name":"Ignored NUL","metadata":nul_fixtures[0],"updateMask":"name","version":"2","idempotencyKey":"nul-name-only"})).await;
    assert_eq!(status, StatusCode::OK, "{name_only}");
    assert_eq!(name_only["endUser"]["version"], "3");
    assert_eq!(name_only["endUser"]["metadata"], a.1["endUser"]["metadata"]);
    // Corrected retries succeed with the rejected key and with a fresh key.
    let unicode =
        json!({"value":"한글 😀","literal":"\\u0000","items":[null,true,3.0,{"key":"value"}]});
    let (status, retry) = call(&http, &base, "CreateEndUser", &secret, json!({"projectId":pid,"externalId":"nul-0","metadata":unicode,"idempotencyKey":"nul-create-0"})).await;
    assert_eq!(status, StatusCode::OK, "{retry}");
    assert_eq!(retry["endUser"]["metadata"], unicode);
    let (status, retry) = call(&http, &base, "UpdateEndUser", &secret, json!({"projectId":pid,"endUserId":uid,"metadata":unicode,"updateMask":"metadata","version":"3","idempotencyKey":"nul-update-0"})).await;
    assert_eq!(status, StatusCode::OK, "{retry}");
    assert_eq!(retry["endUser"]["metadata"], unicode);
    let (status, retry) = call(&http, &base, "UpdateEndUser", &secret, json!({"projectId":pid,"endUserId":uid,"metadata":{},"updateMask":"metadata","version":"4","idempotencyKey":"nul-clear"})).await;
    assert_eq!(status, StatusCode::OK, "{retry}");
    assert_eq!(retry["endUser"]["metadata"], json!({}));
    let (status, retry) = call(&http, &base, "UpdateEndUser", &secret, json!({"projectId":pid,"endUserId":uid,"metadata":{"k":"v"},"updateMask":"metadata","version":"5","idempotencyKey":"nul-restore"})).await;
    assert_eq!(status, StatusCode::OK, "{retry}");
    let (_, stale) = call(
        &http,
        &base,
        "DeleteEndUser",
        &secret,
        json!({"projectId":pid,"endUserId":uid,"version":"1","idempotencyKey":"user-stale"}),
    )
    .await;
    assert_eq!(stale["code"], "aborted");
    let (_, deleted) = call(
        &http,
        &base,
        "DeleteEndUser",
        &secret,
        json!({"projectId":pid,"endUserId":uid,"version":"6","idempotencyKey":"user-delete"}),
    )
    .await;
    assert!(deleted.get("code").is_none(), "{deleted}");
    let (_, scrubbed) = call(&http, &base, "CreateEndUser", &secret, create.clone()).await;
    assert_eq!(scrubbed["code"], "not_found");
    // A matching scrubbed create or update is `not_found` whatever the payload contains and
    // restores nothing.
    expect_all(&http, &base, &db, "CreateEndUser", &secret, "not_found", vec![
        create.clone(),
        json!({"projectId":pid,"externalId":"changed","idempotencyKey":"user-create"}),
        json!({"projectId":pid,"externalId":"","idempotencyKey":"user-create"}),
        json!({"projectId":pid,"externalId":"external","name":long_name,"idempotencyKey":"user-create"}),
        json!({"projectId":pid,"externalId":"external","metadata":large_metadata,"idempotencyKey":"user-create"}),
        json!({"projectId":pid,"externalId":"external","email":"not an email","idempotencyKey":"user-create"}),
    ]).await;
    expect_all(&http, &base, &db, "UpdateEndUser", &secret, "not_found", vec![
        json!({"projectId":pid,"endUserId":uid,"name":"Updated","updateMask":"name","version":"1","idempotencyKey":"user-update"}),
        json!({"projectId":pid,"endUserId":uid,"name":"Other","updateMask":"name","version":"1","idempotencyKey":"user-update"}),
        json!({"projectId":pid,"endUserId":uid,"updateMask":"","version":"1","idempotencyKey":"user-update"}),
        json!({"projectId":pid,"endUserId":uid,"updateMask":"unknown","version":"1","idempotencyKey":"user-update"}),
        json!({"projectId":pid,"endUserId":uid,"name":long_name,"updateMask":"name","version":"1","idempotencyKey":"user-update"}),
        json!({"projectId":pid,"endUserId":"not-a-uuid","name":"Updated","updateMask":"name","version":"1","idempotencyKey":"user-update"}),
    ]).await;
    // A different operation still conflicts, and malformed payload fields cannot override that.
    expect_all(&http, &base, &db, "UpdateEndUser", &secret, "already_exists", vec![
        json!({"projectId":pid,"endUserId":uid,"name":"Updated","updateMask":"name","version":"1","idempotencyKey":"user-create"}),
        json!({"projectId":pid,"endUserId":"not-a-uuid","updateMask":"","idempotencyKey":"user-create"}),
    ]).await;
    expect_all(
        &http,
        &base,
        &db,
        "CreateEndUser",
        &secret,
        "already_exists",
        vec![json!({"projectId":pid,"externalId":"","idempotencyKey":"user-update"})],
    )
    .await;
    // The scrubbed marker never overrides request-level protections.
    expect_all(
        &http,
        &base,
        &db,
        "CreateEndUser",
        &secret,
        "invalid_argument",
        vec![json!({"projectId":pid,"externalId":"external","idempotencyKey":"short"})],
    )
    .await;
    expect_all(
        &http,
        &base,
        &db,
        "CreateEndUser",
        "dbk_invalid",
        "unauthenticated",
        vec![json!({"projectId":pid,"externalId":"","idempotencyKey":"user-create"})],
    )
    .await;
    let oversized = json!({"projectId":pid,"externalId":"external","metadata":{"k":"v".repeat(70_000)},"idempotencyKey":"user-create"});
    expect_all(
        &http,
        &base,
        &db,
        "CreateEndUser",
        &secret,
        "invalid_argument",
        vec![oversized],
    )
    .await;
    let kept = sqlx::query("SELECT scrubbed,response,fingerprint FROM replays WHERE idempotency_key IN ('user-create','user-update')")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(kept.len(), 2);
    assert!(kept.iter().all(|r| r.get::<bool, _>("scrubbed")
        && r.get::<Option<Vec<u8>>, _>("response").is_none()
        && r.get::<Vec<u8>, _>("fingerprint").is_empty()));
    // After expiry the key is a fresh request again, so an invalid payload is `invalid_argument`.
    sqlx::query("UPDATE replays SET expires_at=now()-interval '1 second' WHERE idempotency_key='user-update'")
        .execute(&db)
        .await
        .unwrap();
    let expired = call(&http, &base, "UpdateEndUser", &secret, json!({"projectId":pid,"endUserId":uid,"updateMask":"","version":"1","idempotencyKey":"user-update"})).await;
    assert_eq!(expired.1["code"], "invalid_argument", "{}", expired.1);
    let rows =
        sqlx::query("SELECT fingerprint,response,scrubbed FROM replays WHERE target_user_id=$1")
            .bind(uuid::Uuid::parse_str(&uid).unwrap())
            .fetch_all(&db)
            .await
            .unwrap();
    assert!(
        rows.iter()
            .all(|r| r.get::<bool, _>("scrubbed")
                && r.get::<Option<Vec<u8>>, _>("response").is_none())
    );
    let (_, revoked) = call(
        &http,
        &base,
        "RevokeApiKey",
        "manager",
        json!({"projectId":pid,"apiKeyId":key_id,"idempotencyKey":"key-revoke"}),
    )
    .await;
    assert!(revoked["apiKey"].get("revokeTime").is_some());
    let (_, denied) = call(
        &http,
        &base,
        "ListEndUsers",
        &secret,
        json!({"projectId":pid}),
    )
    .await;
    assert_eq!(denied["code"], "unauthenticated");
    *state.lock().unwrap() = "TENANT_STATE_BLOCKED".into();
    let (_, blocked) = call(
        &http,
        &base,
        "ListEndUsers",
        "manager",
        json!({"projectId":pid}),
    )
    .await;
    assert_eq!(blocked["code"], "permission_denied");
    let (status, _) = call(
        &http,
        &base,
        "GetProject",
        "manager",
        json!({"projectId":pid}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    *state.lock().unwrap() = "TENANT_STATE_ACTIVE".into();
    // Binary Connect requests and replay bytes use the same public schema.
    let req = delino_protocol::devbase::v1::GetProjectRequest {
        project_id: pid.clone(),
        ..Default::default()
    };
    let response = http
        .post(format!("{base}/GetProject"))
        .bearer_auth("manager")
        .header("Content-Type", "application/proto")
        .header("Connect-Protocol-Version", "1")
        .body(req.encode_to_vec())
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let bytes = response.bytes().await.unwrap();
    let response =
        delino_protocol::devbase::v1::GetProjectResponse::decode_from_slice(&bytes).unwrap();
    assert_eq!(response.project.id, pid);
    let delete = json!({"projectId":pid,"version":"1","idempotencyKey":"project-delete"});
    let (status, _) = call(&http, &base, "DeleteProject", "manager", delete.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, v) = call(&http, &base, "DeleteProject", "manager", delete).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    // Login project creation: the deleted project's create record is scrubbed, so the original
    // caller, key and tenant get `not_found` whatever the payload, after the authority checks.
    let create_again =
        json!({"tenantId":tenant,"name":"Project","idempotencyKey":"project-create"});
    expect_all(
        &http,
        &base,
        &db,
        "CreateProject",
        "manager",
        "not_found",
        vec![
            create_again.clone(),
            json!({"tenantId":tenant,"name":"","idempotencyKey":"project-create"}),
            json!({"tenantId":tenant,"name":"Other","idempotencyKey":"project-create"}),
        ],
    )
    .await;
    // A different tenant or operation still conflicts even with a malformed payload.
    expect_all(&http, &base, &db, "CreateProject", "manager", "already_exists", vec![
        json!({"tenantId":uuid::Uuid::now_v7().to_string(),"name":"","idempotencyKey":"project-create"}),
    ]).await;
    expect_all(
        &http,
        &base,
        &db,
        "IssueApiKey",
        "manager",
        "already_exists",
        vec![json!({"projectId":pid,"permission":"API_KEY_PERMISSION_UNSPECIFIED","idempotencyKey":"project-create"})],
    )
    .await;
    // Authentication still wins over a replay-derived result.
    expect_all(
        &http,
        &base,
        &db,
        "CreateProject",
        "stranger",
        "unauthenticated",
        vec![json!({"tenantId":tenant,"name":"","idempotencyKey":"project-create"})],
    )
    .await;
    // A fresh invalid request keeps `invalid_argument`.
    expect_all(
        &http,
        &base,
        &db,
        "CreateProject",
        "manager",
        "invalid_argument",
        vec![json!({"tenantId":tenant,"name":"","idempotencyKey":"project-fresh-invalid"})],
    )
    .await;
    // A login-token mutation for a deleted or missing project validates the idempotency key before
    // the routing-only replay lookup, so a malformed key is `invalid_argument`, not `not_found`,
    // and no replay row is read or changed.
    for key in ["short", "bad key with spaces", "key/with/slash"] {
        for target in [pid.clone(), uuid::Uuid::now_v7().to_string()] {
            expect_all(
                &http,
                &base,
                &db,
                "DeleteProject",
                "manager",
                "invalid_argument",
                vec![json!({"projectId":target,"version":"1","idempotencyKey":key})],
            )
            .await;
        }
    }
    let (_, new) = call(
        &http,
        &base,
        "CreateProject",
        "manager",
        json!({"tenantId":tenant,"name":"Cleanup","idempotencyKey":"cleanup-create"}),
    )
    .await;
    assert!(new.get("project").is_some());
    let failures = server.metrics.cleanup_failures.get();
    *state.lock().unwrap() = "unavailable".into();
    server.maintain().await.unwrap();
    assert_eq!(server.metrics.cleanup_failures.get(), failures + 1);
    let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM projects")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(retained, 1);
    *state.lock().unwrap() = "TENANT_STATE_DELETED".into();
    server.maintain().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM projects")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    // Handled requests keep their own observations and never add outer-timeout counts.
    let metrics = server.metrics.encode();
    assert!(metrics.contains(r#"code="ok""#), "{metrics}");
    assert!(!metrics.contains("deadline_exceeded"), "{metrics}");
    task.abort();
    mock_task.abort();
    drop(server);
    db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
