// SPDX-License-Identifier: FSL-1.1-ALv2
//! Completed-cycle accounting for `Server::maintain()`.
//!
//! The PostgreSQL cases are ignored and need a disposable `TEST_DATABASE_URL`; each creates and
//! drops its own random schema. The closed-pool case needs no database.
use axum::{Json, Router, http::StatusCode, routing::post};
use devbase_api::{Server, config::Config, store};
use serde_json::{Value, json};
use server_common::authority::Authority;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const ACTIVE: &str = "TENANT_STATE_ACTIVE";
const DELETED: &str = "TENANT_STATE_DELETED";
const UNAVAILABLE: &str = "unavailable";
const RUNS_METRIC: &str = "devbase_cleanup_runs_total";

fn config() -> Config {
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

fn authority(base: &str) -> Authority {
    Authority::new(
        reqwest::Client::new(),
        base.to_owned(),
        format!("{base}/token"),
        "resource".into(),
        "client".into(),
        "fixture".into(),
    )
}

/// A migrated disposable schema, a mocked Delcore tenant-state endpoint and a served router.
struct Fixture {
    server: Arc<Server>,
    state: Arc<Mutex<String>>,
    admin: sqlx::PgPool,
    schema: String,
    root: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Fixture {
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
        let state = Arc::new(Mutex::new(ACTIVE.to_owned()));
        let state_mock = state.clone();
        let mock = Router::new()
            .route(
                "/token",
                post(|| async { Json(json!({"access_token":"service","expires_in":3600})) }),
            )
            .route(
                "/delcore.v1.DelcoreService/GetTenantStates",
                post(move |Json(q): Json<Value>| {
                    let state = state_mock.lock().unwrap().clone();
                    async move {
                        if state == UNAVAILABLE {
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
        let server = Arc::new(Server::new(db, authority(&base), config()));
        let router = devbase_api::http::router(server.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let router_task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            server,
            state,
            admin,
            schema,
            root,
            tasks: vec![mock_task, router_task],
        }
    }

    fn set_state(&self, state: &str) {
        *self.state.lock().unwrap() = state.to_owned();
    }

    fn runs(&self) -> u64 {
        self.server.metrics.cleanup_runs.get()
    }

    fn failures(&self) -> u64 {
        self.server.metrics.cleanup_failures.get()
    }

    async fn insert_projects(&self, tenants: usize) {
        for i in 0..tenants {
            sqlx::query("INSERT INTO projects(id,tenant_id,name,created_at,updated_at) VALUES($1,$2,$3,now(),now())")
                .bind(uuid::Uuid::now_v7())
                .bind(uuid::Uuid::now_v7())
                .bind(format!("Project {i}"))
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

    /// Reads the exported sample so the assertion covers the `/metrics` surface itself.
    async fn scraped_runs(&self) -> u64 {
        let body = reqwest::get(format!("{}/metrics", self.root))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains(&format!(
            "# HELP {RUNS_METRIC} Completed maintenance cycles."
        )));
        assert!(body.contains(&format!("# TYPE {RUNS_METRIC} counter")));
        body.lines()
            .find_map(|line| line.strip_prefix(&format!("{RUNS_METRIC} ")))
            .expect("cleanup runs sample")
            .parse()
            .unwrap()
    }

    async fn finish(self) {
        for task in self.tasks {
            task.abort();
        }
        let db = self.server.db.clone();
        drop(self.server);
        db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn empty_cycles_count_as_completed() {
    let f = Fixture::new().await;
    let (runs, failures) = (f.runs(), f.failures());
    f.server.maintain().await.unwrap();
    f.server.maintain().await.unwrap();
    assert_eq!(f.runs(), runs + 2);
    assert_eq!(f.failures(), failures);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn multi_batch_cycle_counts_once() {
    let f = Fixture::new().await;
    // 101 tenants force a second page after the 100-row batch.
    f.insert_projects(101).await;
    let (runs, failures) = (f.runs(), f.failures());
    f.server.maintain().await.unwrap();
    assert_eq!(f.runs(), runs + 1);
    assert_eq!(f.failures(), failures);
    assert_eq!(f.project_count().await, 101);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn deleted_tenant_cleanup_counts_each_cycle() {
    let f = Fixture::new().await;
    f.insert_projects(1).await;
    f.set_state(DELETED);
    let runs = f.runs();
    f.server.maintain().await.unwrap();
    assert_eq!(f.project_count().await, 0);
    assert_eq!(f.runs(), runs + 1);
    f.server.maintain().await.unwrap();
    assert_eq!(f.runs(), runs + 2);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn recoverable_authority_failure_still_completes_cycle() {
    let f = Fixture::new().await;
    f.insert_projects(1).await;
    f.set_state(UNAVAILABLE);
    let (runs, failures) = (f.runs(), f.failures());
    f.server.maintain().await.unwrap();
    assert_eq!(f.failures(), failures + 1);
    assert_eq!(f.runs(), runs + 1);
    assert_eq!(f.project_count().await, 1);
    f.set_state(DELETED);
    f.server.maintain().await.unwrap();
    assert_eq!(f.failures(), failures + 1);
    assert_eq!(f.runs(), runs + 2);
    assert_eq!(f.project_count().await, 0);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn metrics_endpoint_exports_completed_cycles() {
    let f = Fixture::new().await;
    let before = f.scraped_runs().await;
    f.server.maintain().await.unwrap();
    assert_eq!(f.scraped_runs().await, before + 1);
    f.finish().await;
}

#[tokio::test]
async fn failed_cycle_does_not_count_as_completed() {
    // A lazy pool is closed before use, so the first retention query fails at once with `PoolClosed`
    // instead of waiting for the acquire timeout against an unreachable host.
    let db = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://postgres@127.0.0.1:1/unused")
        .unwrap();
    db.close().await;
    let server = Server::new(db, authority("http://127.0.0.1:1"), config());
    let (runs, failures) = (
        server.metrics.cleanup_runs.get(),
        server.metrics.cleanup_failures.get(),
    );
    assert!(server.maintain().await.is_err());
    assert_eq!(server.metrics.cleanup_runs.get(), runs);
    // `maintain()` leaves failure accounting to its scheduled caller.
    assert_eq!(server.metrics.cleanup_failures.get(), failures);
}
