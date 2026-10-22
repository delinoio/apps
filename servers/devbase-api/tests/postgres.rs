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
        json!({"projectId":pid,"endUserId":uid,"version":"2","idempotencyKey":"user-delete"}),
    )
    .await;
    assert!(deleted.get("code").is_none(), "{deleted}");
    let (_, scrubbed) = call(&http, &base, "CreateEndUser", &secret, create).await;
    assert_eq!(scrubbed["code"], "not_found");
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

const MAINTENANCE_PAGE: usize = 100;

fn maintenance_config() -> Config {
    Config {
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
    }
}

fn maintenance_authority(base: &str) -> Authority {
    Authority::new(
        reqwest::Client::new(),
        base.to_owned(),
        format!("{base}/token"),
        "resource".into(),
        "client".into(),
        "fixture".into(),
    )
}

/// A server on an isolated schema whose mocked Delcore returns one tenant state for every tenant.
struct MaintenanceFixture {
    server: Arc<Server>,
    admin: sqlx::PgPool,
    schema: String,
    state: Arc<Mutex<String>>,
    root: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl MaintenanceFixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("devbase_{}", uuid::Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = schema.clone();
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
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
        let state = Arc::new(Mutex::new("TENANT_STATE_ACTIVE".to_owned()));
        let state_mock = state.clone();
        let mock = Router::new()
            .route(
                "/token",
                post(|| async { Json(json!({"access_token":"service","expires_in":3600})) }),
            )
            .route(
                "/delcore.v1.DelcoreService/GetTenantStates",
                post(move |Json(q): Json<Value>| {
                    let state = state_mock.clone();
                    async move {
                        let state = state.lock().unwrap().clone();
                        if state == "unavailable" {
                            return (
                                StatusCode::SERVICE_UNAVAILABLE,
                                Json(json!({"code":"unavailable"})),
                            );
                        }
                        let entries: Vec<_> = q["tenantIds"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|id| json!({"tenantId":id,"state":state}))
                            .collect();
                        (StatusCode::OK, Json(json!({"states":entries})))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mock_task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
        let server = Arc::new(Server::new(
            db,
            maintenance_authority(&base),
            maintenance_config(),
        ));
        let router = devbase_api::http::router(server.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let api_task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            server,
            admin,
            schema,
            state,
            root,
            tasks: vec![mock_task, api_task],
        }
    }

    async fn seed_tenants(&self, count: usize) {
        for index in 0..count {
            sqlx::query("INSERT INTO projects(id,tenant_id,name,created_at,updated_at) VALUES($1,$2,$3,now(),now())")
                .bind(uuid::Uuid::now_v7())
                .bind(uuid::Uuid::now_v7())
                .bind(format!("Project {index}"))
                .execute(&self.server.db)
                .await
                .unwrap();
        }
    }

    async fn project_count(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM projects")
            .fetch_one(&self.server.db)
            .await
            .unwrap()
    }

    fn set_state(&self, state: &str) {
        *self.state.lock().unwrap() = state.to_owned();
    }

    /// Reads a counter from the scraped `/metrics` endpoint.
    async fn scrape(&self, name: &str) -> u64 {
        let body = reqwest::get(format!("{}/metrics", self.root))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        body.lines()
            .find_map(|line| line.strip_prefix(&format!("{name} ")))
            .unwrap_or_else(|| panic!("metric {name} is exposed"))
            .parse()
            .unwrap()
    }

    async fn close(self) {
        for task in &self.tasks {
            task.abort();
        }
        self.server.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn maintenance_counts_completed_cycles() {
    let fixture = MaintenanceFixture::new().await;
    assert_eq!(fixture.scrape("devbase_cleanup_runs_total").await, 0);

    fixture.server.maintain().await.unwrap();
    assert_eq!(fixture.scrape("devbase_cleanup_runs_total").await, 1);
    assert_eq!(fixture.scrape("devbase_cleanup_failures_total").await, 0);

    fixture.server.maintain().await.unwrap();
    assert_eq!(fixture.scrape("devbase_cleanup_runs_total").await, 2);

    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn maintenance_counts_multi_batch_and_cleanup_cycles() {
    let fixture = MaintenanceFixture::new().await;
    fixture.seed_tenants(MAINTENANCE_PAGE + 1).await;

    fixture.server.maintain().await.unwrap();
    assert_eq!(fixture.scrape("devbase_cleanup_runs_total").await, 1);
    assert_eq!(fixture.project_count().await, 101);

    fixture.set_state("TENANT_STATE_DELETED");
    fixture.server.maintain().await.unwrap();
    assert_eq!(fixture.scrape("devbase_cleanup_runs_total").await, 2);
    assert_eq!(fixture.project_count().await, 0);

    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn maintenance_counts_cycles_with_recoverable_authority_failure() {
    let fixture = MaintenanceFixture::new().await;
    fixture.seed_tenants(MAINTENANCE_PAGE + 1).await;
    fixture.set_state("unavailable");

    fixture.server.maintain().await.unwrap();
    assert_eq!(fixture.scrape("devbase_cleanup_runs_total").await, 1);
    assert_eq!(fixture.scrape("devbase_cleanup_failures_total").await, 2);
    assert_eq!(fixture.project_count().await, 101);

    fixture.close().await;
}

#[tokio::test]
async fn maintenance_does_not_count_failed_cycles() {
    // A closed lazy pool fails the first query immediately and needs no database server.
    let db = sqlx::PgPool::connect_lazy("postgres://postgres@127.0.0.1:1/unused").unwrap();
    db.close().await;
    let server = Server::new(
        db,
        maintenance_authority("http://127.0.0.1:1"),
        maintenance_config(),
    );

    assert!(server.maintain().await.is_err());
    assert_eq!(server.metrics.cleanup_runs.get(), 0);
    assert!(
        server
            .metrics
            .encode()
            .contains("devbase_cleanup_runs_total 0")
    );
}
