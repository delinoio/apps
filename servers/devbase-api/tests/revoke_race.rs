// SPDX-License-Identifier: FSL-1.1-ALv2
//! Repeated `RevokeApiKey` of an already-revoked key racing project deletion and tenant cleanup.
//!
//! The cases are ignored and need a disposable `TEST_DATABASE_URL`; each creates and drops its own
//! random schema. A `BEFORE INSERT` trigger on `replays`, restricted to one fresh idempotency key,
//! waits on a session advisory lock that the test holds. That parks the revocation after its key
//! lookup and before its replay insert without adding any lock of its own, so the deletion side can
//! be started while the revocation is in flight.
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use devbase_api::{Server, config::Config, store};
use serde_json::{Value, json};
use server_common::authority::Authority;
use sqlx::Row;
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Notify, task::JoinHandle, time::timeout};

const ACTIVE: &str = "TENANT_STATE_ACTIVE";
const DELETED: &str = "TENANT_STATE_DELETED";
const FRESH: &str = "revoke-fresh";
const BOUND: Duration = Duration::from_secs(20);

/// Parks any Delcore membership check made with the bearer token `paused`.
#[derive(Default)]
struct Gate {
    arrived: Notify,
    release: Notify,
}

struct Fixture {
    server: Arc<Server>,
    state: Arc<Mutex<String>>,
    gate: Arc<Gate>,
    admin: sqlx::PgPool,
    db: sqlx::PgPool,
    schema: String,
    base: String,
    tenant: String,
    http: reqwest::Client,
    tasks: Vec<JoinHandle<()>>,
}

type Reply = (StatusCode, Value);

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
        let actor = uuid::Uuid::now_v7().to_string();
        let state = Arc::new(Mutex::new(ACTIVE.to_owned()));
        let gate = Arc::new(Gate::default());
        let (state_mock, gate_mock) = (state.clone(), gate.clone());
        let mock = Router::new()
            .route(
                "/token",
                post(|| async { Json(json!({"access_token":"service","expires_in":3600})) }),
            )
            .route(
                "/delcore.v1.DelcoreService/GetCurrentUser",
                post(move |h: HeaderMap| {
                    let actor = actor.clone();
                    async move {
                        match bearer(&h).as_str() {
                            "manager" | "paused" => (StatusCode::OK, Json(json!({"userId":actor}))),
                            _ => (
                                StatusCode::UNAUTHORIZED,
                                Json(json!({"code":"unauthenticated"})),
                            ),
                        }
                    }
                }),
            )
            .route(
                "/delcore.v1.DelcoreService/ListMembers",
                post(move |h: HeaderMap| {
                    let gate = gate_mock.clone();
                    async move {
                        let token = bearer(&h);
                        if token == "paused" {
                            gate.arrived.notify_one();
                            gate.release.notified().await;
                        }
                        if matches!(token.as_str(), "manager" | "paused") {
                            (StatusCode::OK, Json(json!({})))
                        } else {
                            (
                                StatusCode::FORBIDDEN,
                                Json(json!({"code":"permission_denied"})),
                            )
                        }
                    }
                }),
            )
            .route(
                "/delcore.v1.DelcoreService/GetTenantStates",
                post(move |Json(q): Json<Value>| {
                    let state = state_mock.lock().unwrap().clone();
                    async move {
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
        let mock_base = format!("http://{}", listener.local_addr().unwrap());
        let mock_task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
        let http = reqwest::Client::new();
        let authority = Authority::new(
            http.clone(),
            mock_base.clone(),
            format!("{mock_base}/token"),
            "resource".into(),
            "client".into(),
            "fixture".into(),
        );
        let config = Config {
            projects: 100,
            users: 100,
            keys: 20,
            user_rate: 10000.0,
            user_burst: 20000,
            management_rate: 10000.0,
            management_burst: 20000,
            // The paused authorization must outlast the competing deletion.
            dependency_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(60),
            cleanup_interval: Duration::from_secs(300),
            origin: "https://console.example".into(),
            listen: "127.0.0.1:0".into(),
        };
        let server = Arc::new(Server::new(db.clone(), authority, config));
        let router = devbase_api::http::router(server.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!(
            "http://{}/devbase.v1.DevBaseService",
            listener.local_addr().unwrap()
        );
        let router_task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            server,
            state,
            gate,
            admin,
            db,
            schema,
            base,
            tenant: uuid::Uuid::now_v7().to_string(),
            http,
            tasks: vec![mock_task, router_task],
        }
    }

    fn call(&self, method: &str, token: &str, q: Value) -> impl Future<Output = Reply> + use<> {
        let (http, url, token, method) = (
            self.http.clone(),
            format!("{}/{method}", self.base),
            token.to_owned(),
            method.to_owned(),
        );
        async move {
            let r = http
                .post(url)
                .bearer_auth(token)
                .header("Connect-Protocol-Version", "1")
                .json(&q)
                .send()
                .await
                .unwrap_or_else(|e| panic!("{method}: {e}"));
            let status = r.status();
            (status, r.json().await.unwrap())
        }
    }

    fn spawn(&self, method: &str, token: &str, q: Value) -> JoinHandle<Reply> {
        tokio::spawn(self.call(method, token, q))
    }

    /// Creates a project with one key and revokes it. Returns the project, key and revoke time.
    async fn revoked_key(&self) -> (String, String, String) {
        let (status, p) = self
            .call(
                "CreateProject",
                "manager",
                json!({"tenantId":self.tenant,"name":"Race","idempotencyKey":"project-create"}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{p}");
        let pid = p["project"]["id"].as_str().unwrap().to_owned();
        let (status, k) = self
            .call(
                "IssueApiKey",
                "manager",
                json!({"projectId":pid,"permission":"API_KEY_PERMISSION_CRUD","idempotencyKey":"key-issue"}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{k}");
        let key = k["apiKey"]["id"].as_str().unwrap().to_owned();
        let (status, r) = self.revoke("manager", &pid, &key, "revoke-first").await;
        assert_eq!(status, StatusCode::OK, "{r}");
        let time = r["apiKey"]["revokeTime"].as_str().unwrap().to_owned();
        (pid, key, time)
    }

    async fn revoke(&self, token: &str, pid: &str, key: &str, idem: &str) -> Reply {
        self.call(
            "RevokeApiKey",
            token,
            json!({"projectId":pid,"apiKeyId":key,"idempotencyKey":idem}),
        )
        .await
    }

    /// Parks the replay insert of the fresh revocation behind an advisory lock held by the
    /// returned connection. Returns the lock key and that connection.
    async fn install_barrier(&self) -> (i64, sqlx::pool::PoolConnection<sqlx::Postgres>) {
        let lock = i64::from(rand::random::<u32>() >> 1);
        let schema = &self.schema;
        sqlx::raw_sql(&format!(
            "CREATE FUNCTION {schema}.replay_barrier() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN \
               IF NEW.operation = 'RevokeApiKey' AND NEW.idempotency_key = '{FRESH}' THEN \
                 PERFORM pg_advisory_lock({lock}); \
                 PERFORM pg_advisory_unlock({lock}); \
               END IF; \
               RETURN NEW; \
             END $$; \
             CREATE TRIGGER replay_barrier BEFORE INSERT ON {schema}.replays \
               FOR EACH ROW EXECUTE FUNCTION {schema}.replay_barrier();"
        ))
        .execute(&self.admin)
        .await
        .unwrap();
        let mut held = self.admin.acquire().await.unwrap();
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(lock)
            .execute(&mut *held)
            .await
            .unwrap();
        (lock, held)
    }

    async fn wait_until<F, Fut>(&self, what: &str, mut ready: F)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = bool>,
    {
        timeout(BOUND, async {
            while !ready().await {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    /// Waits until the revocation backend is parked at the replay barrier.
    async fn wait_at_barrier(&self, lock: i64) {
        self.wait_until("the revocation at the replay barrier", || async {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND NOT granted AND classid=0 AND objid::bigint=$1")
                .bind(lock)
                .fetch_one(&self.admin)
                .await
                .unwrap()
                > 0
        })
        .await;
    }

    /// Waits until the project delete statement is blocked on a row lock.
    async fn wait_deletion_blocked(&self) {
        self.wait_until("the project deletion blocked on the key lock", || async {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'DELETE FROM projects%'")
                .fetch_one(&self.admin)
                .await
                .unwrap()
                > 0
        })
        .await;
    }

    /// Releases the barrier and returns its connection to the pool, which `finish` waits for.
    async fn release(&self, lock: i64, mut held: sqlx::pool::PoolConnection<sqlx::Postgres>) {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(lock)
            .execute(&mut *held)
            .await
            .unwrap();
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.db).await.unwrap()
    }

    /// The fresh revocation record as `(scrubbed, has response, fingerprint length)`.
    async fn fresh_record(&self) -> Option<(bool, bool, i32)> {
        sqlx::query("SELECT scrubbed,response IS NOT NULL AS has_response,octet_length(fingerprint) AS len FROM replays WHERE idempotency_key=$1")
            .bind(FRESH)
            .fetch_optional(&self.db)
            .await
            .unwrap()
            .map(|r| (r.get("scrubbed"), r.get("has_response"), r.get("len")))
    }

    async fn finish(self) {
        for task in &self.tasks {
            task.abort();
        }
        self.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

fn bearer(h: &HeaderMap) -> String {
    h.get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_owned()
}

async fn bounded<T>(what: &str, task: JoinHandle<T>) -> T {
    timeout(BOUND, task)
        .await
        .unwrap_or_else(|_| panic!("{what} did not finish"))
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn repeated_revocation_is_an_ordinary_recorded_read_of_the_key() {
    let f = Fixture::new().await;
    let (pid, key, revoked_at) = f.revoked_key().await;
    let audit = f
        .count("SELECT count(*) FROM audit_events WHERE action='api_key.revoke'")
        .await;
    assert_eq!(audit, 1);
    let (status, again) = f.revoke("manager", &pid, &key, FRESH).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["apiKey"]["id"], key.as_str());
    assert_eq!(again["apiKey"]["revokeTime"], revoked_at.as_str());
    assert!(again.get("secret").is_none());
    assert_eq!(
        f.count("SELECT count(*) FROM audit_events WHERE action='api_key.revoke'")
            .await,
        audit,
        "an already-revoked key adds no revoke audit event"
    );
    let record = f.fresh_record().await.unwrap();
    assert_eq!((record.0, record.1), (false, true));
    assert!(record.2 > 0);
    // A matching retry replays the stored answer and GetApiKey stays a plain read.
    let (status, retry) = f.revoke("manager", &pid, &key, FRESH).await;
    assert_eq!(status, StatusCode::OK, "{retry}");
    assert_eq!(retry["apiKey"], again["apiKey"]);
    let (status, got) = f
        .call(
            "GetApiKey",
            "manager",
            json!({"projectId":pid,"apiKeyId":key}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["apiKey"]["revokeTime"], revoked_at.as_str());
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn project_deletion_waits_for_repeated_revocation_and_scrubs_its_replay() {
    let f = Fixture::new().await;
    let (pid, key, revoked_at) = f.revoked_key().await;
    let (lock, held) = f.install_barrier().await;
    let revoke = f.spawn(
        "RevokeApiKey",
        "manager",
        json!({"projectId":pid,"apiKeyId":key,"idempotencyKey":FRESH}),
    );
    f.wait_at_barrier(lock).await;
    // The parked revocation does not block readers of the key.
    let (status, got) = f
        .call(
            "GetApiKey",
            "manager",
            json!({"projectId":pid,"apiKeyId":key}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let delete = f.spawn(
        "DeleteProject",
        "manager",
        json!({"projectId":pid,"version":"1","idempotencyKey":"project-delete"}),
    );
    // The deletion must wait on the revocation's key lock instead of completing first.
    f.wait_deletion_blocked().await;
    assert!(
        !delete.is_finished(),
        "deletion finished before the revocation committed"
    );
    f.release(lock, held).await;
    let (status, revoked) = bounded("revocation", revoke).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["apiKey"]["revokeTime"], revoked_at.as_str());
    let (status, deleted) = bounded("deletion", delete).await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(f.count("SELECT count(*) FROM projects").await, 0);
    assert_eq!(f.count("SELECT count(*) FROM api_keys").await, 0);
    assert_eq!(
        f.fresh_record().await,
        Some((true, false, 0)),
        "the deletion must scrub the replay the revocation committed"
    );
    assert_eq!(
        f.count("SELECT count(*) FROM audit_events WHERE action='api_key.revoke'")
            .await,
        1
    );
    // A matching retry by the same authorized login user is `not_found`.
    let (status, retry) = f.revoke("manager", &pid, &key, FRESH).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{retry}");
    assert_eq!(retry["code"], "not_found");
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn tenant_cleanup_waits_for_repeated_revocation_and_scrubs_its_replay() {
    let f = Fixture::new().await;
    let (pid, key, _) = f.revoked_key().await;
    let (lock, held) = f.install_barrier().await;
    let revoke = f.spawn(
        "RevokeApiKey",
        "manager",
        json!({"projectId":pid,"apiKeyId":key,"idempotencyKey":FRESH}),
    );
    f.wait_at_barrier(lock).await;
    // The revocation already passed authorization; the tenant is deleted while it is in flight.
    *f.state.lock().unwrap() = DELETED.to_owned();
    let server = f.server.clone();
    let cleanup = tokio::spawn(async move { server.maintain().await.unwrap() });
    f.wait_deletion_blocked().await;
    assert!(
        !cleanup.is_finished(),
        "cleanup finished before the revocation committed"
    );
    f.release(lock, held).await;
    let (status, revoked) = bounded("revocation", revoke).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    bounded("cleanup", cleanup).await;
    assert_eq!(f.count("SELECT count(*) FROM projects").await, 0);
    assert_eq!(f.count("SELECT count(*) FROM api_keys").await, 0);
    assert_eq!(f.count("SELECT count(*) FROM deleted_tenants").await, 1);
    assert_eq!(
        f.fresh_record().await,
        Some((true, false, 0)),
        "cleanup must scrub the replay the revocation committed"
    );
    // The deleted tenant denies the matching retry before any replay is read.
    let (status, retry) = f.revoke("manager", &pid, &key, FRESH).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{retry}");
    assert_eq!(retry["code"], "permission_denied");
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn committed_deletion_wins_over_paused_repeated_revocation() {
    let f = Fixture::new().await;
    let (pid, key, _) = f.revoked_key().await;
    let replays = f.count("SELECT count(*) FROM replays").await;
    let revoke = f.spawn(
        "RevokeApiKey",
        "paused",
        json!({"projectId":pid,"apiKeyId":key,"idempotencyKey":FRESH}),
    );
    // The revocation resolved its project; its authorization is parked at Delcore.
    timeout(BOUND, f.gate.arrived.notified())
        .await
        .expect("the revocation did not reach authorization");
    let (status, deleted) = f
        .call(
            "DeleteProject",
            "manager",
            json!({"projectId":pid,"version":"1","idempotencyKey":"project-delete"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    let replays_after_delete = f.count("SELECT count(*) FROM replays").await;
    f.gate.release.notify_one();
    let (status, late) = bounded("revocation", revoke).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{late}");
    assert_eq!(late["code"], "not_found");
    assert_eq!(f.fresh_record().await, None, "the lost race wrote a replay");
    assert_eq!(
        f.count("SELECT count(*) FROM replays").await,
        replays_after_delete
    );
    // The deletion added its own delete record and nothing else.
    assert_eq!(replays_after_delete, replays + 1);
    f.finish().await;
}
