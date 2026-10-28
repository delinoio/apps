// SPDX-License-Identifier: FSL-1.1-ALv2
//! Device synchronization timestamp persistence through the authenticated production dispatch.
//!
//! These tests run `Server::execute` (and the real Connect router for subscriptions) against a
//! disposable migrated PostgreSQL schema and a mocked Delcore authority that always reports an
//! ACTIVE tenant. They need `LANE_TEST_DATABASE_URL` and are ignored by default.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, HeaderValue, Request},
};
use chrono::{DateTime, Utc};
use lane_api::{
    Result, Server, account_lock, auth::Auth, config::Config, hash, id, migration, push::Push,
    webhooks::WebhookConfig,
};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Once, OnceLock},
    time::Duration,
};
use tower::ServiceExt;

const PUBLIC_URL: &str = "http://127.0.0.1:46999";
const PRIOR: &str = "2020-01-01T00:00:00Z";

/// Minimal Delcore and OIDC mock: every Authorize is allowed and every tenant is ACTIVE.
fn mock_authority() -> &'static str {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let base = url.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let base = base.clone();
                std::thread::spawn(move || serve_mock(stream, &base));
            }
        });
        url
    })
}
fn serve_mock(mut stream: std::net::TcpStream, base: &str) {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head_end, length) = loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        raw.extend_from_slice(&chunk[..n]);
        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..end]).to_lowercase();
            let length = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            break (end + 4, length);
        }
    };
    while raw.len() < head_end + length {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        raw.extend_from_slice(&chunk[..n]);
    }
    let line = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let path = line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let body: Value = serde_json::from_slice(&raw[head_end..head_end + length]).unwrap_or_default();
    let reply = if path.ends_with("/openid-configuration") {
        json!({"issuer":base,"authorization_endpoint":format!("{base}/authorize"),"token_endpoint":format!("{base}/token"),"jwks_uri":format!("{base}/jwks")})
    } else if path == "/token" {
        json!({"access_token":"synthetic-m2m","expires_in":3600})
    } else if path.ends_with("/Authorize") {
        json!({"allowed":true})
    } else if path.ends_with("/GetTenantStates") {
        let states: Vec<Value> = body["tenantIds"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| json!({"tenantId":t,"state":"TENANT_STATE_ACTIVE"}))
            .collect();
        json!({"states":states})
    } else {
        json!({})
    };
    let encoded = reply.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        encoded.len(),
        encoded
    );
}

fn configure_environment() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let mock = mock_authority();
        let clients = json!({"fixture":{"name":"fixture","redirect_uris":[format!("{PUBLIC_URL}/callback")],"origin_kind":"ORIGIN_KIND_CLI","resource":PUBLIC_URL}});
        let returns = json!([format!("{PUBLIC_URL}/callback")]);
        // SAFETY: executed once, before any server component reads these variables.
        unsafe {
            for (key, value) in [
                ("LANE_PUBLIC_URL", PUBLIC_URL.to_owned()),
                ("LOGTO_ISSUER", mock.to_owned()),
                ("LANE_WEB_RETURN_URLS", returns.to_string()),
                ("LANE_OAUTH_CLIENTS", clients.to_string()),
                ("AUTH_REQUESTS_PER_MINUTE", "100000".into()),
                ("LANE_LOGTO_CLIENT_ID", "fixture".into()),
                ("LANE_LOGTO_CLIENT_SECRET", "fixture".into()),
                ("DELCORE_RESOURCE", "fixture".into()),
                ("DELCORE_URL", mock.to_owned()),
                ("LOGTO_TOKEN_URL", format!("{mock}/token")),
                ("CLIENT_ID", "fixture".into()),
                ("CLIENT_SECRET", "fixture".into()),
                ("LANE_APNS_ENABLED", "false".into()),
                ("LANE_FCM_ENABLED", "false".into()),
                ("LANE_WEB_PUSH_ENABLED", "false".into()),
            ] {
                std::env::set_var(key, value);
            }
        }
    });
}

struct Account {
    id: String,
    tenant: String,
    epoch: String,
}
struct Fx {
    server: Arc<Server>,
    pool: PgPool,
    admin: PgPool,
    schema: String,
    a: Account,
    b: Account,
    /// Device D of account A, and an unrelated device of A.
    d: String,
    d2: String,
}
fn token(label: &str) -> String {
    format!("{label:x<43}")
}
fn bearer(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers
}
const TOKEN_D: &str = "device-d";
const TOKEN_D2: &str = "device-d2";
const TOKEN_B: &str = "account-b-device-d";

impl Fx {
    async fn new() -> Self {
        configure_environment();
        let url = std::env::var("LANE_TEST_DATABASE_URL").expect("LANE_TEST_DATABASE_URL");
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("lane_contact_{}", id().replace('-', ""));
        admin
            .execute(format!("CREATE SCHEMA {schema}").as_str())
            .await
            .unwrap();
        let search = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(12)
            .after_connect(move |conn, _| {
                let query = format!("SET search_path TO {search}");
                Box::pin(async move {
                    conn.execute(query.as_str()).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        migration::migrate(&pool).await.unwrap();
        // Disposable fixture hooks: a deferred trigger fires at commit, after the timestamp write.
        pool.execute(
            "CREATE TABLE fixture_fail(x int);
             CREATE TABLE fixture_slow(x int);
             CREATE TABLE fixture_rejection_gate(id int PRIMARY KEY);
             CREATE FUNCTION fixture_rejection_gate_hook() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN
               PERFORM 1 FROM fixture_rejection_gate WHERE id=1 FOR UPDATE;
               RETURN NULL;
             END $$;
             CREATE TRIGGER fixture_rejection_gate_hook AFTER INSERT ON lane_sync_rejections
               FOR EACH ROW EXECUTE FUNCTION fixture_rejection_gate_hook();
             CREATE FUNCTION fixture_commit_hook() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN
               IF EXISTS (SELECT 1 FROM fixture_fail) THEN RAISE EXCEPTION 'injected commit failure'; END IF;
               IF EXISTS (SELECT 1 FROM fixture_slow) THEN PERFORM pg_sleep(1.5); END IF;
               RETURN NULL;
             END $$;
             CREATE CONSTRAINT TRIGGER fixture_commit_hook AFTER UPDATE OF last_sync_at ON lane_devices
               DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fixture_commit_hook();",
        )
        .await
        .unwrap();
        let config = Config {
            listen: "127.0.0.1:0".into(),
            public_url: PUBLIC_URL.into(),
            web_origin: PUBLIC_URL.into(),
            metrics_token: "synthetic-metrics-token-at-least-32-characters".into(),
            max_item_bytes: 65536,
            max_webhook_bytes: 4096,
            requests_per_minute: 100000,
            webhook_requests_per_minute: 100000,
        };
        let auth = Arc::new(Auth::from_env(pool.clone()).await.unwrap());
        let push = Arc::new(Push::from_env(pool.clone()).unwrap());
        let webhooks = Arc::new(
            WebhookConfig::new(
                "CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg",
                PUBLIC_URL,
                4096,
            )
            .unwrap(),
        );
        let server = Arc::new(Server {
            db: pool.clone(),
            auth,
            config: Arc::new(config),
            metrics: Arc::default(),
            push,
            webhooks,
        });
        let account = || Account {
            id: id(),
            tenant: id(),
            epoch: id(),
        };
        let fx = Self {
            server,
            pool,
            admin,
            schema,
            a: account(),
            b: account(),
            d: id(),
            d2: id(),
        };
        for acct in [&fx.a, &fx.b] {
            sqlx::query("INSERT INTO lane_accounts VALUES($1,$2,$3)")
                .bind(&acct.id)
                .bind(&acct.tenant)
                .bind(&acct.epoch)
                .execute(&fx.pool)
                .await
                .unwrap();
        }
        fx.device(&fx.a, &fx.d, TOKEN_D).await;
        fx.device(&fx.a, &fx.d2, TOKEN_D2).await;
        // Account B reuses the same device ID as account A's device D.
        fx.device(&fx.b, &fx.d, TOKEN_B).await;
        fx
    }
    /// Approve a device with an access credential, as the approval flow would, but with null time.
    async fn device(&self, acct: &Account, device: &str, label: &str) {
        sqlx::query("INSERT INTO lane_devices(account_id,id,name) VALUES($1,$2,$3)")
            .bind(&acct.id)
            .bind(device)
            .bind(label)
            .execute(&self.pool)
            .await
            .unwrap();
        self.session(acct, device, label, &acct.epoch).await;
    }
    async fn session(&self, acct: &Account, device: &str, label: &str, epoch: &str) {
        let session = id();
        sqlx::query("INSERT INTO lane_sessions(id,account_id,device_id,client_id,origin_kind,resource,issuer,csrf_hash,redirect_uri,epoch) VALUES($1,$2,$3,'fixture','ORIGIN_KIND_CLI',$4,$4,'fixture',$5,$6)")
            .bind(&session)
            .bind(&acct.id)
            .bind(device)
            .bind(PUBLIC_URL)
            .bind(format!("{PUBLIC_URL}/callback"))
            .bind(epoch)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO lane_credentials VALUES($1,$2,'access',clock_timestamp()+interval '1 hour',NULL)")
            .bind(hash(token(label).as_bytes()))
            .bind(&session)
            .execute(&self.pool)
            .await
            .unwrap();
    }
    async fn call(&self, label: &str, service: &str, method: &str, q: Value) -> Result<Value> {
        self.server
            .execute(&bearer(&token(label)), service, method, q, false)
            .await
    }
    async fn pull(&self, label: &str, extra: Value) -> Result<Value> {
        let mut q = json!({"formatVersion":3,"epoch":self.a.epoch});
        for (k, v) in extra.as_object().into_iter().flatten() {
            q[k] = v.clone();
        }
        self.call(label, "SyncService", "PullChanges", q).await
    }
    async fn push(
        &self,
        label: &str,
        device: &str,
        epoch: &str,
        items: Vec<Value>,
    ) -> Result<Value> {
        self.call(
            label,
            "SyncService",
            "PushChanges",
            json!({"formatVersion":3,"epoch":epoch,"deviceId":device,"mutations":items}),
        )
        .await
    }
    async fn time(&self, acct: &Account, device: &str) -> Option<DateTime<Utc>> {
        sqlx::query_scalar("SELECT last_sync_at FROM lane_devices WHERE account_id=$1 AND id=$2")
            .bind(&acct.id)
            .bind(device)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn seed_prior(&self, acct: &Account, device: &str) -> DateTime<Utc> {
        sqlx::query(
            "UPDATE lane_devices SET last_sync_at=$3::timestamptz WHERE account_id=$1 AND id=$2",
        )
        .bind(&acct.id)
        .bind(device)
        .bind(PRIOR)
        .execute(&self.pool)
        .await
        .unwrap();
        self.time(acct, device).await.unwrap()
    }
    async fn db_now(&self) -> DateTime<Utc> {
        sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn pending_failures(&self) -> i64 {
        let response = lane_api::http::router(self.server.clone())
            .oneshot(
                Request::get("/metrics")
                    .header(
                        "authorization",
                        "Bearer synthetic-metrics-token-at-least-32-characters",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = std::str::from_utf8(&body).unwrap();
        text.lines()
            .find_map(|line| line.strip_prefix("lane_sync_pending_failures "))
            .unwrap()
            .parse()
            .unwrap()
    }
    /// Run an authenticated call and assert D's time was set within the call's database-clock window.
    async fn assert_advances(
        &self,
        device: &str,
        before: Option<DateTime<Utc>>,
        call: impl std::future::Future<Output = Result<Value>>,
    ) -> (Value, DateTime<Utc>) {
        let start = self.db_now().await;
        let out = call.await.unwrap();
        let end = self.db_now().await;
        let after = self.time(&self.a, device).await.expect("time recorded");
        assert!(
            after >= start && after <= end,
            "bounded by database server time"
        );
        if let Some(before) = before {
            assert!(after > before, "time advances");
        }
        (out, after)
    }
    async fn listed(&self, device: &str) -> Option<DateTime<Utc>> {
        let out = self
            .call(TOKEN_D2, "AccountService", "ListDevices", json!({}))
            .await
            .unwrap();
        let item = out["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == device)
            .unwrap();
        item.get("lastSyncAt").map(|v| {
            DateTime::parse_from_rfc3339(v.as_str().unwrap())
                .unwrap()
                .with_timezone(&Utc)
        })
    }
    async fn waiting(&self, query: &str) {
        for _ in 0..200 {
            let n: i64 = sqlx::query_scalar(query)
                .fetch_one(&self.admin)
                .await
                .unwrap();
            if n > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("condition was not reached");
    }
    async fn close(self) {
        drop(self.server);
        self.pool.close().await;
        self.admin
            .execute(format!("DROP SCHEMA {} CASCADE", self.schema).as_str())
            .await
            .unwrap();
        self.admin.close().await;
    }
}

fn repository(
    entity: &str,
    version: &str,
    parents: &[&str],
    url: &str,
    name: &str,
    device: &str,
    format: u64,
) -> Value {
    let v = json!({"formatVersion":format,"entityId":entity,"kind":"ENTITY_KIND_REPOSITORY","versionId":version,"parentVersionIds":parents,"createdAt":"2026-01-01T00:00:00.000000Z","origin":{"kind":"ORIGIN_KIND_CLI","deviceId":device,"deviceName":"fixture","accountId":""},"deleted":false,"repository":{"normalizedUrl":url,"displayName":name}});
    json!({"meta":{"requestId":id()},"kind":if parents.is_empty() {"MUTATION_KIND_CREATE"} else {"MUTATION_KIND_SAVE"},"version":v,"expectedHeadIds":[]})
}
fn create_repo(fx: &Fx, url: &str) -> Value {
    repository(&id(), &id(), &[], url, "n", &fx.d, 3)
}
fn task_item(
    entity: &str,
    version: &str,
    parent: Option<&str>,
    repo: &str,
    device: &str,
    missing: bool,
) -> Value {
    let parents: Vec<&str> = parent.into_iter().collect();
    let mut task = json!({"repositoryId":repo,"title":"Task","description":"","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR"});
    if missing {
        task["parentTaskId"] = json!(id());
    }
    json!({"meta":{"requestId":id()},"kind":if parents.is_empty() {"MUTATION_KIND_CREATE"} else {"MUTATION_KIND_SAVE"},"version":{"formatVersion":3,"entityId":entity,"kind":"ENTITY_KIND_TASK","versionId":version,"parentVersionIds":parents,"createdAt":"2026-01-01T00:00:00.000000Z","origin":{"kind":"ORIGIN_KIND_CLI","deviceId":device,"deviceName":"fixture","accountId":""},"deleted":false,"task":task},"expectedHeadIds":[]})
}
async fn rejection_count(fx: &Fx, request: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM lane_sync_rejections WHERE account_id=$1 AND request_id=$2",
    )
    .bind(&fx.a.id)
    .bind(request)
    .fetch_one(&fx.pool)
    .await
    .unwrap()
}
/// An item the production push path rejects before any write (unsupported version format).
fn rejected(fx: &Fx) -> Value {
    repository(&id(), &id(), &[], "example.com/rejected/x", "n", &fx.d, 9)
}
fn is_rejection(out: &Value, index: usize) -> bool {
    out["results"][index].get("error").is_some()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn corrected_create_and_save_retire_only_covered_failures() {
    let fx = Fx::new().await;
    let repo = create_repo(&fx, "example.com/correction/repo");
    let repo_id = repo["version"]["entityId"].as_str().unwrap();
    assert!(!is_rejection(
        &fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![repo.clone()])
            .await
            .unwrap(),
        0
    ));
    let entity = id();
    let first = task_item(&entity, &id(), None, repo_id, &fx.d, true);
    let first_request = first["meta"]["requestId"].as_str().unwrap().to_owned();
    let second = task_item(&entity, &id(), None, repo_id, &fx.d, true);
    let second_request = second["meta"]["requestId"].as_str().unwrap().to_owned();
    let rejected = fx
        .push(TOKEN_D, &fx.d, &fx.a.epoch, vec![first, second])
        .await
        .unwrap();
    assert!(is_rejection(&rejected, 0) && is_rejection(&rejected, 1));
    assert_eq!(fx.pending_failures().await, 2);
    assert_eq!(fx.count("lane_versions").await, 1);

    let corrected_version = id();
    let corrected = task_item(&entity, &corrected_version, None, repo_id, &fx.d, false);
    let accepted = fx
        .push(TOKEN_D, &fx.d, &fx.a.epoch, vec![corrected])
        .await
        .unwrap();
    assert_eq!(
        accepted["results"][0]["result"]["result"],
        "RESULT_KIND_APPLIED"
    );
    assert_eq!(rejection_count(&fx, &first_request).await, 0);
    assert_eq!(rejection_count(&fx, &second_request).await, 0);
    assert_eq!(fx.pending_failures().await, 0);
    let heads: Vec<String> = sqlx::query_scalar(
        "SELECT version_id FROM lane_heads WHERE account_id=$1 AND entity_id=$2",
    )
    .bind(&fx.a.id)
    .bind(&entity)
    .fetch_all(&fx.pool)
    .await
    .unwrap();
    assert_eq!(heads, vec![corrected_version.clone()]);

    let failed_save = task_item(
        &entity,
        &id(),
        Some(&corrected_version),
        repo_id,
        &fx.d,
        true,
    );
    let failed_request = failed_save["meta"]["requestId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(is_rejection(
        &fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![failed_save])
            .await
            .unwrap(),
        0
    ));
    assert_eq!(fx.pending_failures().await, 1);
    let saved_version = id();
    let saved = task_item(
        &entity,
        &saved_version,
        Some(&corrected_version),
        repo_id,
        &fx.d,
        false,
    );
    assert_eq!(
        fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![saved])
            .await
            .unwrap()["results"][0]["result"]["result"],
        "RESULT_KIND_APPLIED"
    );
    assert_eq!(rejection_count(&fx, &failed_request).await, 0);
    assert_eq!(fx.pending_failures().await, 0);
    assert_eq!(fx.count("lane_versions").await, 3);
    fx.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn correction_keeps_other_accounts_devices_entities_and_old_replay_failures() {
    let fx = Fx::new().await;
    let repo = create_repo(&fx, "example.com/correction/isolation");
    let repo_id = repo["version"]["entityId"].as_str().unwrap();
    fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![repo.clone()])
        .await
        .unwrap();
    let entity = id();
    let a = task_item(&entity, &id(), None, repo_id, &fx.d, true);
    let a_id = a["meta"]["requestId"].as_str().unwrap().to_owned();
    let other_device = task_item(&entity, &id(), None, repo_id, &fx.d2, true);
    let other_device_id = other_device["meta"]["requestId"]
        .as_str()
        .unwrap()
        .to_owned();
    let other_entity = task_item(&id(), &id(), None, repo_id, &fx.d, true);
    let other_entity_id = other_entity["meta"]["requestId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(is_rejection(
        &fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![a]).await.unwrap(),
        0
    ));
    assert!(is_rejection(
        &fx.push(TOKEN_D2, &fx.d2, &fx.a.epoch, vec![other_device])
            .await
            .unwrap(),
        0
    ));
    assert!(is_rejection(
        &fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![other_entity])
            .await
            .unwrap(),
        0
    ));
    let other_account = task_item(&entity, &id(), None, repo_id, &fx.d, true);
    assert!(is_rejection(
        &fx.push(TOKEN_B, &fx.d, &fx.b.epoch, vec![other_account])
            .await
            .unwrap(),
        0
    ));
    let legacy = id();
    sqlx::query("INSERT INTO lane_sync_rejections(account_id,request_id,reason) VALUES($1,$2,19)")
        .bind(&fx.a.id)
        .bind(&legacy)
        .execute(&fx.pool)
        .await
        .unwrap();
    assert_eq!(fx.pending_failures().await, 5);
    let old_receipt = fx
        .push(TOKEN_D, &fx.d, &fx.a.epoch, vec![repo.clone()])
        .await
        .unwrap();
    assert_eq!(
        old_receipt["results"][0]["result"]["result"],
        "RESULT_KIND_REPLAYED"
    );
    assert_eq!(fx.pending_failures().await, 5);
    let corrected = task_item(&entity, &id(), None, repo_id, &fx.d, false);
    assert_eq!(
        fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![corrected])
            .await
            .unwrap()["results"][0]["result"]["result"],
        "RESULT_KIND_APPLIED"
    );
    assert_eq!(rejection_count(&fx, &a_id).await, 0);
    assert_eq!(rejection_count(&fx, &other_device_id).await, 1);
    assert_eq!(rejection_count(&fx, &other_entity_id).await, 1);
    assert_eq!(rejection_count(&fx, &legacy).await, 1);
    assert_eq!(fx.pending_failures().await, 4);
    fx.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn failed_correction_rolls_back_retirement_and_exact_request_retry_still_clears() {
    let fx = Fx::new().await;
    let repo = create_repo(&fx, "example.com/correction/rollback");
    let repo_id = repo["version"]["entityId"].as_str().unwrap();
    fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![repo.clone()])
        .await
        .unwrap();
    let entity = id();
    let first = task_item(&entity, &id(), None, repo_id, &fx.d, true);
    let first_request = first["meta"]["requestId"].as_str().unwrap().to_owned();
    let dependency = first["version"]["task"]["parentTaskId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(is_rejection(
        &fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![first.clone()])
            .await
            .unwrap(),
        0
    ));

    let corrected = task_item(&entity, &id(), None, repo_id, &fx.d, false);
    fx.pool
        .execute("INSERT INTO fixture_fail VALUES(1)")
        .await
        .unwrap();
    assert!(
        fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![corrected.clone()])
            .await
            .is_err()
    );
    assert_eq!(fx.pending_failures().await, 1);
    let task_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM lane_entities WHERE account_id=$1 AND id=$2")
            .bind(&fx.a.id)
            .bind(&entity)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(task_count, 0);
    fx.pool.execute("DELETE FROM fixture_fail").await.unwrap();

    let dependency_item = task_item(&dependency, &id(), None, repo_id, &fx.d, false);
    fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![dependency_item])
        .await
        .unwrap();
    assert_eq!(fx.pending_failures().await, 1);
    let retried = fx
        .push(TOKEN_D, &fx.d, &fx.a.epoch, vec![first])
        .await
        .unwrap();
    assert_eq!(
        retried["results"][0]["result"]["result"],
        "RESULT_KIND_APPLIED"
    );
    assert_eq!(rejection_count(&fx, &first_request).await, 0);
    assert_eq!(fx.pending_failures().await, 0);
    fx.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn old_receipts_ancestor_acks_and_changed_bases_keep_newer_failures() {
    let fx = Fx::new().await;
    let repo = create_repo(&fx, "example.com/correction/replay");
    let repo_id = repo["version"]["entityId"].as_str().unwrap();
    fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![repo.clone()])
        .await
        .unwrap();
    let entity = id();
    let base = id();
    let created = task_item(&entity, &base, None, repo_id, &fx.d, false);
    fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![created.clone()])
        .await
        .unwrap();
    let failed = task_item(&entity, &id(), Some(&base), repo_id, &fx.d, true);
    let failed_request = failed["meta"]["requestId"].as_str().unwrap().to_owned();
    assert!(is_rejection(
        &fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![failed])
            .await
            .unwrap(),
        0
    ));
    let receipt = fx
        .push(TOKEN_D, &fx.d, &fx.a.epoch, vec![created.clone()])
        .await
        .unwrap();
    assert_eq!(
        receipt["results"][0]["result"]["result"],
        "RESULT_KIND_REPLAYED"
    );
    let mut ancestor = created;
    ancestor["meta"]["requestId"] = json!(id());
    let ack = fx
        .push(TOKEN_D, &fx.d, &fx.a.epoch, vec![ancestor])
        .await
        .unwrap();
    assert_eq!(
        ack["results"][0]["result"]["result"],
        "RESULT_KIND_REPLAYED"
    );
    assert_eq!(rejection_count(&fx, &failed_request).await, 1);

    let next = id();
    let independent = task_item(&entity, &next, Some(&base), repo_id, &fx.d2, false);
    fx.push(TOKEN_D2, &fx.d2, &fx.a.epoch, vec![independent])
        .await
        .unwrap();
    assert_eq!(rejection_count(&fx, &failed_request).await, 1);
    let changed_base = task_item(&entity, &id(), Some(&next), repo_id, &fx.d, false);
    fx.push(TOKEN_D, &fx.d, &fx.a.epoch, vec![changed_base])
        .await
        .unwrap();
    assert_eq!(rejection_count(&fx, &failed_request).await, 1);
    fx.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn pulls_record_only_the_authenticated_device_and_list_devices_reports_it() {
    let fx = Fx::new().await;
    // Approval and negotiation alone never record synchronization.
    let info = fx
        .call(TOKEN_D, "SyncService", "GetSyncInfo", json!({}))
        .await
        .unwrap();
    assert_eq!(info["epoch"], fx.a.epoch);
    assert_eq!(fx.time(&fx.a, &fx.d).await, None);
    assert_eq!(fx.listed(&fx.d).await, None);

    // Empty pull.
    let (_, first) = fx
        .assert_advances(&fx.d, None, fx.pull(TOKEN_D, json!({"pageSize":1})))
        .await;
    assert_eq!(
        fx.listed(&fx.d).await,
        Some(first),
        "ListDevices returns the exact instant"
    );
    // Control devices: another device of A and account B's device with the same ID stay null.
    assert_eq!(fx.time(&fx.a, &fx.d2).await, None);
    assert_eq!(fx.time(&fx.b, &fx.d).await, None);

    // Populated, paginated pulls also advance it.
    for n in 0..3 {
        fx.push(
            TOKEN_D,
            &fx.d,
            &fx.a.epoch,
            vec![create_repo(&fx, &format!("example.com/o/r{n}"))],
        )
        .await
        .unwrap();
    }
    let before = fx.seed_prior(&fx.a, &fx.d).await;
    let (page, second) = fx
        .assert_advances(&fx.d, Some(before), fx.pull(TOKEN_D, json!({"pageSize":1})))
        .await;
    assert_eq!(page["hasMore"], true);
    let (_, third) = fx
        .assert_advances(
            &fx.d,
            Some(second),
            fx.pull(TOKEN_D, json!({"pageSize":1,"cursor":page["nextCursor"]})),
        )
        .await;
    assert!(third > second);
    assert_eq!(fx.listed(&fx.d).await, Some(third));
    assert_eq!(fx.time(&fx.a, &fx.d2).await, None);
    assert_eq!(fx.time(&fx.b, &fx.d).await, None);

    // Account B's device with the same ID records only B.
    fx.call(
        TOKEN_B,
        "SyncService",
        "PullChanges",
        json!({"formatVersion":3,"epoch":fx.b.epoch}),
    )
    .await
    .unwrap();
    assert!(fx.time(&fx.b, &fx.d).await.is_some());
    assert_eq!(
        fx.listed(&fx.d).await,
        Some(third),
        "account A's time is unchanged by account B"
    );
    assert_eq!(fx.time(&fx.a, &fx.d2).await, None);
    fx.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn push_commits_qualify_only_when_a_transaction_succeeds() {
    let fx = Fx::new().await;
    let (a, d) = (&fx.a, fx.d.clone());
    // A validated empty batch qualifies.
    let (_, t) = fx
        .assert_advances(&d, None, fx.push(TOKEN_D, &d, &a.epoch, vec![]))
        .await;

    // An accepted item qualifies; an identical receipt replay qualifies again.
    let prior = fx.seed_prior(a, &d).await;
    let item = create_repo(&fx, "example.com/o/accepted");
    let (out, _) = fx
        .assert_advances(
            &d,
            Some(prior),
            fx.push(TOKEN_D, &d, &a.epoch, vec![item.clone()]),
        )
        .await;
    assert!(!is_rejection(&out, 0));
    let prior = fx.seed_prior(a, &d).await;
    let (replay, _) = fx
        .assert_advances(&d, Some(prior), fx.push(TOKEN_D, &d, &a.epoch, vec![item]))
        .await;
    assert_eq!(
        replay["results"][0]["result"]["result"],
        "RESULT_KIND_REPLAYED"
    );
    assert!(t < fx.time(a, &d).await.unwrap());

    // A rejected item and an all-rejected batch do not advance a seeded prior time.
    let prior = fx.seed_prior(a, &d).await;
    let out = fx
        .push(TOKEN_D, &d, &a.epoch, vec![rejected(&fx)])
        .await
        .unwrap();
    assert!(is_rejection(&out, 0));
    let out = fx
        .push(TOKEN_D, &d, &a.epoch, vec![rejected(&fx), rejected(&fx)])
        .await
        .unwrap();
    assert!(is_rejection(&out, 0) && is_rejection(&out, 1));
    assert_eq!(fx.time(a, &d).await, Some(prior));
    assert_eq!(fx.count("lane_sync_rejections").await, 3);

    // A mixed batch keeps independent per-item transactions: the accepted item advances time and
    // the later rejection neither erases it nor changes the per-item results.
    let accepted = create_repo(&fx, "example.com/o/mixed");
    let (mixed, _) = fx
        .assert_advances(
            &d,
            Some(prior),
            fx.push(TOKEN_D, &d, &a.epoch, vec![accepted, rejected(&fx)]),
        )
        .await;
    assert!(!is_rejection(&mixed, 0) && is_rejection(&mixed, 1));
    assert_eq!(
        mixed["results"][0]["result"]["result"],
        "RESULT_KIND_APPLIED"
    );
    assert_eq!(fx.count("lane_sync_rejections").await, 4);

    // A retained conflict result is a successful transaction.
    let (entity, base) = (id(), id());
    fx.push(
        TOKEN_D,
        &d,
        &a.epoch,
        vec![repository(
            &entity,
            &base,
            &[],
            "example.com/o/conflict",
            "base",
            &d,
            3,
        )],
    )
    .await
    .unwrap();
    fx.push(
        TOKEN_D,
        &d,
        &a.epoch,
        vec![repository(
            &entity,
            &id(),
            &[&base],
            "example.com/o/conflict",
            "left",
            &d,
            3,
        )],
    )
    .await
    .unwrap();
    let prior = fx.seed_prior(a, &d).await;
    let right = repository(
        &entity,
        &id(),
        &[&base],
        "example.com/o/conflict",
        "right",
        &d,
        3,
    );
    let (conflict, _) = fx
        .assert_advances(&d, Some(prior), fx.push(TOKEN_D, &d, &a.epoch, vec![right]))
        .await;
    assert_eq!(
        conflict["results"][0]["result"]["result"],
        "RESULT_KIND_CONFLICT"
    );

    // Only the authenticated device recorded contact.
    assert_eq!(fx.time(a, &fx.d2).await, None);
    assert_eq!(fx.time(&fx.b, &fx.d).await, None);
    fx.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn validation_failures_rollbacks_and_non_sync_calls_never_advance_time() {
    let fx = Fx::new().await;
    let (a, d) = (&fx.a, fx.d.clone());
    let foreign = {
        fx.push(
            TOKEN_B,
            &d,
            &fx.b.epoch,
            vec![
                create_repo(&fx, "example.com/b/r1"),
                create_repo(&fx, "example.com/b/r2"),
            ],
        )
        .await
        .unwrap();
        let page = fx
            .call(
                TOKEN_B,
                "SyncService",
                "PullChanges",
                json!({"formatVersion":3,"epoch":fx.b.epoch,"pageSize":1}),
            )
            .await
            .unwrap();
        page["nextCursor"].clone()
    };
    assert!(foreign.as_str().is_some_and(|c| !c.is_empty()));
    let prior = fx.seed_prior(a, &d).await;
    let seeded_b = fx.seed_prior(&fx.b, &d).await;

    // Rejected pulls.
    assert!(fx.pull(TOKEN_D, json!({"formatVersion":99})).await.is_err());
    assert_eq!(
        fx.pull(TOKEN_D, json!({"epoch":id()}))
            .await
            .unwrap_err()
            .reason,
        17
    );
    assert!(fx.pull(TOKEN_D, json!({"cursor":foreign})).await.is_err());
    // Rejected empty and item pushes: wrong format, stale epoch, mismatched device.
    let item = |fx: &Fx| create_repo(fx, "example.com/o/never");
    for items in [vec![], vec![item(&fx)]] {
        let empty = items.is_empty();
        let mismatch = fx.push(TOKEN_D, &fx.d2, &a.epoch, items.clone()).await;
        let stale = fx.push(TOKEN_D, &d, &id(), items.clone()).await;
        if empty {
            assert!(mismatch.is_err() && stale.is_err());
        } else {
            assert!(is_rejection(&mismatch.unwrap(), 0) && is_rejection(&stale.unwrap(), 0));
        }
    }
    let bad_format = fx
        .call(
            TOKEN_D,
            "SyncService",
            "PushChanges",
            json!({"formatVersion":2,"epoch":a.epoch,"deviceId":d,"mutations":[]}),
        )
        .await;
    assert!(bad_format.is_err());
    assert_eq!(fx.time(a, &d).await, Some(prior));
    assert_eq!(
        fx.count("lane_entities").await,
        2,
        "only account B's setup entities exist"
    );

    // Successful non-synchronization calls do not advance it either.
    fx.call(TOKEN_D, "SyncService", "GetSyncInfo", json!({}))
        .await
        .unwrap();
    fx.call(
        TOKEN_D,
        "SyncService",
        "WatchChanges",
        json!({"epoch":a.epoch,"cursor":""}),
    )
    .await
    .unwrap();
    fx.call(TOKEN_D, "LaneService", "ListRepositories", json!({}))
        .await
        .unwrap();
    fx.call(TOKEN_D, "AccountService", "ListDevices", json!({}))
        .await
        .unwrap();
    fx.call(
        TOKEN_D,
        "LaneService",
        "CreateRepository",
        json!({"meta":{"requestId":id()},"id":id(),"url":"https://example.com/management/repo"}),
    )
    .await
    .unwrap();
    assert_eq!(fx.time(a, &d).await, Some(prior));
    assert_eq!(fx.time(&fx.b, &d).await, Some(seeded_b));

    // A storage failure after the timestamp write but before commit rolls everything back.
    sqlx::query("INSERT INTO fixture_fail VALUES(1)")
        .execute(&fx.pool)
        .await
        .unwrap();
    let (entities, cursors, changes) = (
        fx.count("lane_entities").await,
        fx.count("lane_entity_cursors").await,
        fx.count("lane_changes").await,
    );
    assert!(
        fx.push(
            TOKEN_D,
            &d,
            &a.epoch,
            vec![create_repo(&fx, "example.com/o/rolled-back")]
        )
        .await
        .is_err()
    );
    assert!(fx.push(TOKEN_D, &d, &a.epoch, vec![]).await.is_err());
    assert!(fx.pull(TOKEN_D, json!({"pageSize":1})).await.is_err());
    assert_eq!(fx.time(a, &d).await, Some(prior));
    assert_eq!(fx.count("lane_entities").await, entities);
    assert_eq!(fx.count("lane_entity_cursors").await, cursors);
    assert_eq!(fx.count("lane_changes").await, changes);
    sqlx::query("DELETE FROM fixture_fail")
        .execute(&fx.pool)
        .await
        .unwrap();
    // The same operations succeed once the fixture failure is disarmed.
    fx.assert_advances(
        &d,
        Some(prior),
        fx.push(
            TOKEN_D,
            &d,
            &a.epoch,
            vec![create_repo(&fx, "example.com/o/committed")],
        ),
    )
    .await;
    fx.close().await;
}

async fn deletion_request(fx: &Fx, label: &str, mode: &str) -> Value {
    let session: String =
        sqlx::query_scalar("SELECT session_id FROM lane_credentials WHERE digest=$1")
            .bind(hash(token(label).as_bytes()))
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO lane_reauth_challenges(id,account_id,session_id,mode,expires_at,authenticated_at,proof_hash,proof_expires_at) VALUES($1,$2,$3,$4,clock_timestamp()+interval '10 minutes',clock_timestamp(),$5,clock_timestamp()+interval '10 minutes')")
        .bind(id())
        .bind(&fx.a.id)
        .bind(session)
        .bind(mode)
        .bind(hash(b"proof"))
        .execute(&fx.pool)
        .await
        .unwrap();
    json!({"meta":{"requestId":id()},"mode":mode,"confirmedAccountId":fx.a.id,"reauthProof":"proof"})
}

async fn delete_account_data(fx: &Fx, label: &str, mode: &str) -> Result<Value> {
    let request = deletion_request(fx, label, mode).await;
    fx.call(label, "AccountService", "DeleteAccountData", request)
        .await
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn rejection_commits_before_account_clearing_and_is_removed() {
    for mode in [
        "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL",
        "DATA_DELETION_MODE_DELETE_EVERYWHERE",
    ] {
        let fx = Fx::new().await;
        fx.pool
            .execute("INSERT INTO fixture_rejection_gate VALUES(1)")
            .await
            .unwrap();
        let mut gate = fx.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM fixture_rejection_gate WHERE id=1 FOR UPDATE")
            .execute(&mut *gate)
            .await
            .unwrap();
        let item = rejected(&fx);
        let request_id = item["meta"]["requestId"].as_str().unwrap().to_owned();
        let push = {
            let server = fx.server.clone();
            let device = fx.d.clone();
            let epoch = fx.a.epoch.clone();
            tokio::spawn(async move {
                server
                    .execute(
                        &bearer(&token(TOKEN_D)),
                        "SyncService",
                        "PushChanges",
                        json!({"formatVersion":3,"epoch":epoch,"deviceId":device,"mutations":[item]}),
                        false,
                    )
                    .await
            })
        };
        fx.waiting("SELECT count(*) FROM pg_stat_activity WHERE wait_event='transactionid' AND query LIKE 'INSERT INTO lane_sync_rejections%'")
            .await;
        assert_eq!(fx.pending_failures().await, 0);

        let request = deletion_request(&fx, TOKEN_D, mode).await;
        let clearing = {
            let server = fx.server.clone();
            tokio::spawn(async move {
                server
                    .execute(
                        &bearer(&token(TOKEN_D)),
                        "AccountService",
                        "DeleteAccountData",
                        request,
                        false,
                    )
                    .await
            })
        };
        fx.waiting("SELECT count(*) FROM pg_stat_activity WHERE wait_event='advisory' AND query LIKE '%pg_advisory_xact_lock%'")
            .await;
        gate.commit().await.unwrap();
        let result = push.await.unwrap().unwrap();
        assert!(is_rejection(&result, 0));
        let cleared = clearing.await.unwrap().unwrap();
        assert_ne!(cleared["newEpoch"], fx.a.epoch);
        assert_eq!(fx.pending_failures().await, 0);
        assert_eq!(fx.count("lane_sync_rejections").await, 0);
        for table in [
            "lane_entities",
            "lane_versions",
            "lane_heads",
            "lane_changes",
            "lane_entity_receipts",
            "lane_version_identities",
            "lane_attachments",
            "lane_attachment_identities",
        ] {
            assert_eq!(fx.count(table).await, 0, "{table}");
        }
        assert_eq!(fx.time(&fx.a, &fx.d).await, None);
        let revoked: bool = sqlx::query_scalar(
            "SELECT bool_and(revoked_at IS NOT NULL) FROM lane_sessions WHERE account_id=$1",
        )
        .bind(&fx.a.id)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
        assert!(revoked);
        let disconnected: bool = sqlx::query_scalar(
            "SELECT bool_and(disconnected_at IS NOT NULL) FROM lane_devices WHERE account_id=$1",
        )
        .bind(&fx.a.id)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
        assert!(disconnected);
        let rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM lane_sync_rejections WHERE account_id=$1 AND request_id=$2",
        )
        .bind(&fx.a.id)
        .bind(&request_id)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
        assert_eq!(rows, 0);
        fx.close().await;
    }

    let fx = Fx::new().await;
    sqlx::query(
        "INSERT INTO lane_sync_rejections(account_id,request_id,reason) VALUES($1,$2,8),($3,$4,8)",
    )
    .bind(&fx.a.id)
    .bind(id())
    .bind(&fx.b.id)
    .bind(id())
    .execute(&fx.pool)
    .await
    .unwrap();
    delete_account_data(&fx, TOKEN_D, "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL")
        .await
        .unwrap();
    let account_b_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM lane_sync_rejections WHERE account_id=$1")
            .bind(&fx.b.id)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(account_b_rows, 1);
    assert_eq!(fx.pending_failures().await, 1);
    fx.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn clearing_or_revocation_first_rejects_waiting_push_without_bookkeeping() {
    for mode in [
        "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL",
        "DATA_DELETION_MODE_DELETE_EVERYWHERE",
        "session_revocation",
    ] {
        let fx = Fx::new().await;
        let mut operation = fx.pool.begin().await.unwrap();
        account_lock(&mut operation, &fx.a.id).await.unwrap();
        let push = {
            let server = fx.server.clone();
            let device = fx.d.clone();
            let epoch = fx.a.epoch.clone();
            let item = rejected(&fx);
            tokio::spawn(async move {
                server
                    .execute(
                        &bearer(&token(TOKEN_D)),
                        "SyncService",
                        "PushChanges",
                        json!({"formatVersion":3,"epoch":epoch,"deviceId":device,"mutations":[item]}),
                        false,
                    )
                    .await
            })
        };
        fx.waiting("SELECT count(*) FROM pg_stat_activity WHERE wait_event='advisory' AND query LIKE '%pg_advisory_xact_lock%'")
            .await;
        if mode == "session_revocation" {
            sqlx::query("UPDATE lane_sessions SET revoked_at=clock_timestamp() WHERE account_id=$1 AND device_id=$2")
                .bind(&fx.a.id).bind(&fx.d).execute(&mut *operation).await.unwrap();
        } else {
            let request = deletion_request(&fx, TOKEN_D, mode).await;
            let principal = fx
                .server
                .auth
                .authenticate(&bearer(&token(TOKEN_D)))
                .await
                .unwrap();
            fx.server
                .auth
                .authorize(&mut operation, &principal)
                .await
                .unwrap();
            fx.server
                .auth
                .dispatch(&mut operation, &principal, "DeleteAccountData", &request)
                .await
                .unwrap();
        }
        operation.commit().await.unwrap();
        assert!(push.await.unwrap().is_err());
        assert_eq!(fx.count("lane_sync_rejections").await, 0);
        assert_eq!(fx.pending_failures().await, 0);
        assert_eq!(fx.time(&fx.a, &fx.d).await, None);
        fx.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn failed_item_work_rolls_back_and_bookkeeping_failure_has_no_fallback() {
    let fx = Fx::new().await;
    let accepted = create_repo(&fx, "example.com/o/accepted-before-failure");
    let accepted_id = accepted["version"]["entityId"].as_str().unwrap().to_owned();
    let output = fx
        .push(TOKEN_D, &fx.d, &fx.a.epoch, vec![accepted])
        .await
        .unwrap();
    assert!(!is_rejection(&output, 0));
    let contact = fx.time(&fx.a, &fx.d).await;
    sqlx::query("INSERT INTO lane_sync_rejections(account_id,request_id,reason) VALUES($1,$2,8)")
        .bind(&fx.b.id)
        .bind(id())
        .execute(&fx.pool)
        .await
        .unwrap();
    assert_eq!(fx.pending_failures().await, 1);

    fx.pool
        .execute(
            "CREATE FUNCTION fixture_fail_version() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'injected version failure'; END $$;
         CREATE TRIGGER fixture_fail_version BEFORE INSERT ON lane_versions
           FOR EACH ROW EXECUTE FUNCTION fixture_fail_version();",
        )
        .await
        .unwrap();
    let rejected_item = create_repo(&fx, "example.com/o/rolled-back");
    let rejected_id = rejected_item["version"]["entityId"]
        .as_str()
        .unwrap()
        .to_owned();
    let result = fx
        .push(TOKEN_D, &fx.d, &fx.a.epoch, vec![rejected_item])
        .await
        .unwrap();
    assert!(is_rejection(&result, 0));
    assert_eq!(fx.time(&fx.a, &fx.d).await, contact);
    for table in [
        "lane_entities",
        "lane_versions",
        "lane_heads",
        "lane_changes",
        "lane_entity_receipts",
        "lane_version_identities",
    ] {
        assert_eq!(fx.count(table).await, 1, "{table}");
    }
    let entities: Vec<String> =
        sqlx::query_scalar("SELECT id FROM lane_entities WHERE account_id=$1")
            .bind(&fx.a.id)
            .fetch_all(&fx.pool)
            .await
            .unwrap();
    assert_eq!(entities, vec![accepted_id]);
    assert!(!entities.contains(&rejected_id));
    assert_eq!(fx.count("lane_attachments").await, 0);
    assert_eq!(fx.count("lane_attachment_identities").await, 0);
    assert_eq!(fx.pending_failures().await, 2);

    fx.pool
        .execute(
            "CREATE FUNCTION fixture_fail_rejection() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'injected bookkeeping failure'; END $$;
         CREATE TRIGGER fixture_fail_rejection BEFORE INSERT ON lane_sync_rejections
           FOR EACH ROW EXECUTE FUNCTION fixture_fail_rejection();",
        )
        .await
        .unwrap();
    let result = fx
        .push(TOKEN_D, &fx.d, &fx.a.epoch, vec![rejected(&fx)])
        .await;
    assert!(result.is_err());
    assert_eq!(fx.pending_failures().await, 2);
    assert_eq!(fx.count("lane_entities").await, 1);
    assert_eq!(fx.time(&fx.a, &fx.d).await, contact);
    fx.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn deletion_reset_and_directive_pull_record_only_the_current_device() {
    for (mode, delete_everywhere) in [
        ("DATA_DELETION_MODE_DELETE_EVERYWHERE", true),
        ("DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL", false),
    ] {
        let fx = Fx::new().await;
        let old_epoch = fx.a.epoch.clone();
        fx.pull(TOKEN_D, json!({})).await.unwrap();
        fx.pull(TOKEN_D2, json!({})).await.unwrap();
        assert!(fx.time(&fx.a, &fx.d).await.is_some() && fx.time(&fx.a, &fx.d2).await.is_some());
        let cleared = delete_account_data(&fx, TOKEN_D, mode).await.unwrap();
        let new_epoch = cleared["newEpoch"].as_str().unwrap().to_owned();
        // Retained old device rows have their times reset.
        assert_eq!(fx.time(&fx.a, &fx.d).await, None);
        assert_eq!(fx.time(&fx.a, &fx.d2).await, None);
        // Approve a current session on a new device for the new generation.
        let current = id();
        sqlx::query("INSERT INTO lane_devices(account_id,id,name) VALUES($1,$2,'current')")
            .bind(&fx.a.id)
            .bind(&current)
            .execute(&fx.pool)
            .await
            .unwrap();
        fx.session(&fx.a, &current, "current-device", &new_epoch)
            .await;
        assert_eq!(fx.time(&fx.a, &current).await, None);

        // An unknown stale epoch fails without advancing time.
        let stale = fx
            .call(
                "current-device",
                "SyncService",
                "PullChanges",
                json!({"formatVersion":3,"epoch":id()}),
            )
            .await;
        assert_eq!(stale.unwrap_err().reason, 17);
        assert_eq!(fx.time(&fx.a, &current).await, None);
        let start = fx.db_now().await;
        let known = fx
            .call(
                "current-device",
                "SyncService",
                "PullChanges",
                json!({"formatVersion":3,"epoch":old_epoch}),
            )
            .await;
        if delete_everywhere {
            let out = known.unwrap();
            assert_eq!(
                out,
                json!({"changes":[{"sequence":"0","deletion":{"kind":"ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT","id":fx.a.id,"deletionVersionId":new_epoch}}],"nextCursor":"","hasMore":false})
            );
            let at = fx.time(&fx.a, &current).await.unwrap();
            let end = fx.db_now().await;
            assert!(at >= start && at <= end);
        } else {
            assert_eq!(known.unwrap_err().reason, 17);
            assert_eq!(fx.time(&fx.a, &current).await, None);
        }
        // Retained old devices never record contact (their grants are revoked).
        assert_eq!(fx.time(&fx.a, &fx.d).await, None);
        assert_eq!(fx.time(&fx.a, &fx.d2).await, None);
        fx.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn revocation_ordering_and_established_watches() {
    let fx = Fx::new().await;
    let (a, d) = (&fx.a, fx.d.clone());
    let prior = fx.seed_prior(a, &d).await;

    // Revocation commits first: the waiting synchronization fails and its timestamp is never written.
    let mut revoke = fx.pool.begin().await.unwrap();
    account_lock(&mut revoke, &a.id).await.unwrap();
    let racing = {
        let (server, epoch) = (fx.server.clone(), a.epoch.clone());
        tokio::spawn(async move {
            server
                .execute(
                    &bearer(&token(TOKEN_D)),
                    "SyncService",
                    "PullChanges",
                    json!({"formatVersion":3,"epoch":epoch}),
                    false,
                )
                .await
        })
    };
    fx.waiting("SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND NOT granted")
        .await;
    sqlx::query(
        "UPDATE lane_devices SET disconnected_at=clock_timestamp() WHERE account_id=$1 AND id=$2",
    )
    .bind(&a.id)
    .bind(&d)
    .execute(&mut *revoke)
    .await
    .unwrap();
    sqlx::query("UPDATE lane_sessions SET revoked_at=clock_timestamp() WHERE account_id=$1 AND device_id=$2").bind(&a.id).bind(&d).execute(&mut *revoke).await.unwrap();
    revoke.commit().await.unwrap();
    assert!(racing.await.unwrap().is_err());
    assert_eq!(fx.time(a, &d).await, Some(prior));
    sqlx::query("UPDATE lane_devices SET disconnected_at=NULL WHERE account_id=$1 AND id=$2")
        .bind(&a.id)
        .bind(&d)
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE lane_sessions SET revoked_at=NULL WHERE account_id=$1 AND device_id=$2")
        .bind(&a.id)
        .bind(&d)
        .execute(&fx.pool)
        .await
        .unwrap();

    // Synchronization first: it holds the account lock through its slow commit, so the
    // disconnection waits and the timestamp commits atomically before the revocation.
    sqlx::query("INSERT INTO fixture_slow VALUES(1)")
        .execute(&fx.pool)
        .await
        .unwrap();
    let syncing = {
        let (server, epoch) = (fx.server.clone(), a.epoch.clone());
        tokio::spawn(async move {
            server
                .execute(
                    &bearer(&token(TOKEN_D)),
                    "SyncService",
                    "PullChanges",
                    json!({"formatVersion":3,"epoch":epoch}),
                    false,
                )
                .await
        })
    };
    fx.waiting("SELECT count(*) FROM pg_stat_activity WHERE wait_event='PgSleep'")
        .await;
    let disconnect = {
        let (server, device) = (fx.server.clone(), d.clone());
        tokio::spawn(async move {
            server
                .execute(
                    &bearer(&token(TOKEN_D2)),
                    "AccountService",
                    "DisconnectDevice",
                    json!({"meta":{"requestId":id()},"deviceId":device}),
                    false,
                )
                .await
        })
    };
    syncing.await.unwrap().unwrap();
    disconnect.await.unwrap().unwrap();
    sqlx::query("DELETE FROM fixture_slow")
        .execute(&fx.pool)
        .await
        .unwrap();
    let (synced, disconnected): (Option<DateTime<Utc>>, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT last_sync_at,disconnected_at FROM lane_devices WHERE account_id=$1 AND id=$2",
    )
    .bind(&a.id)
    .bind(&d)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    let synced = synced.expect("earlier successful synchronization committed its time");
    assert!(synced > prior && synced < disconnected.unwrap());
    assert!(
        fx.pull(TOKEN_D, json!({})).await.is_err(),
        "revoked device can no longer synchronize"
    );
    assert_eq!(fx.time(a, &d).await, Some(synced));

    // An established watch terminates after revocation and never refreshes the timestamp.
    let (current, label) = (id(), "watching-device");
    sqlx::query("INSERT INTO lane_devices(account_id,id,name) VALUES($1,$2,'watch')")
        .bind(&a.id)
        .bind(&current)
        .execute(&fx.pool)
        .await
        .unwrap();
    fx.session(a, &current, label, &a.epoch).await;
    let watch_prior = fx.seed_prior(a, &current).await;
    let app: Router = lane_api::http::router(fx.server.clone());
    let payload = serde_json::to_vec(&json!({"epoch":a.epoch,"cursor":""})).unwrap();
    let mut framed = vec![0u8];
    framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    framed.extend_from_slice(&payload);
    let request = Request::post("/lane.v1.SyncService/WatchChanges")
        .header("content-type", "application/connect+json")
        .header("connect-protocol-version", "1")
        .header("authorization", format!("Bearer {}", token(label)))
        .body(Body::from(framed))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert!(response.status().is_success());
    // Let several one-second ticks run, then revoke the watching device.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        fx.time(a, &current).await,
        Some(watch_prior),
        "watch ticks do not record contact"
    );
    fx.call(
        TOKEN_D2,
        "AccountService",
        "DisconnectDevice",
        json!({"meta":{"requestId":id()},"deviceId":current}),
    )
    .await
    .unwrap();
    let body = tokio::time::timeout(
        Duration::from_secs(15),
        to_bytes(response.into_body(), usize::MAX),
    )
    .await
    .expect("watch terminates after revocation")
    .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("unauthenticated"));
    assert_eq!(fx.time(a, &current).await, Some(watch_prior));
    fx.close().await;
}
