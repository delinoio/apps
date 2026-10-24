// SPDX-License-Identifier: FSL-1.1-ALv2
//! Shared harness: loopback mocks of Delcore (with Logto token endpoint) and the Vercel AI
//! Gateway, plus a running DeliTools server reached over real HTTP with Connect JSON.
#![allow(dead_code)]

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use base64::Engine;
use buffa::Enumeration;
use buffa::Message;
use delitools_api::{
    Server,
    config::{
        Config, DelcoreSettings, GatewaySettings, Limits, OutboxSettings, RetryPolicy, Secret,
    },
    http,
    models::ModelCatalog,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

pub const SERVICE_ID: &str = "019b0000-0000-7000-8000-0000000000aa";
pub const TENANT: &str = "019b0000-0000-7000-8000-0000000000b1";
pub const OTHER_TENANT: &str = "019b0000-0000-7000-8000-0000000000b2";
pub const USER: &str = "019b0000-0000-7000-8000-0000000000c1";
pub const USER_TOKEN: &str = "user-token";
pub const KEY_SECRET: &str =
    "dlk_00000000000000000000000000000001_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
pub const MODEL: &str = "google/gemini-translate";
pub const LUNA: &str = "openai/gpt-6-luna";

/// A scripted Gateway answer.
#[derive(Clone)]
pub enum Reply {
    Ok {
        content: String,
        usage: Value,
    },
    Status(u16),
    RetryAfter(u16, u64),
    Garbage,
    /// Never answers; the request ends only when the caller aborts it.
    Hang,
    Hook(Arc<dyn Fn() + Send + Sync>, Box<Reply>),
    Delay(u64, Box<Reply>),
}

impl Reply {
    pub fn ok(content: &str, prompt: u64, completion: u64) -> Self {
        Self::Ok {
            content: content.to_owned(),
            usage: json!({"prompt_tokens": prompt, "completion_tokens": completion}),
        }
    }
    pub fn without_usage(content: &str) -> Self {
        Self::Ok {
            content: content.to_owned(),
            usage: Value::Null,
        }
    }
}

#[derive(Default)]
pub struct GatewayState {
    pub script: Mutex<VecDeque<Reply>>,
    pub requests: Mutex<Vec<Value>>,
    pub calls: AtomicUsize,
    pub dropped: AtomicUsize,
}

impl GatewayState {
    pub fn push(&self, reply: Reply) {
        self.script.lock().unwrap().push_back(reply);
    }
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::SeqCst)
    }
    pub fn last_request(&self) -> Value {
        self.requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a gateway request")
    }
}

struct DropCounter(Arc<GatewayState>);
impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

async fn gateway_handler(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer gw-secret") {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.calls.fetch_add(1, Ordering::SeqCst);
    state
        .requests
        .lock()
        .unwrap()
        .push(serde_json::from_slice(&body).unwrap_or_default());
    let mut reply = state
        .script
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or(Reply::Status(500));
    loop {
        match reply {
            Reply::Hook(hook, next) => {
                hook();
                reply = *next;
            }
            Reply::Delay(ms, next) => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                reply = *next;
            }
            Reply::Ok { content, usage } => {
                let mut body = json!({
                    "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}]
                });
                if !usage.is_null() {
                    body["usage"] = usage;
                }
                return Json(body).into_response();
            }
            Reply::Status(code) => {
                return StatusCode::from_u16(code).unwrap().into_response();
            }
            Reply::RetryAfter(code, seconds) => {
                return (
                    StatusCode::from_u16(code).unwrap(),
                    [("retry-after", seconds.to_string())],
                )
                    .into_response();
            }
            Reply::Garbage => return (StatusCode::OK, "<html>not json</html>").into_response(),
            Reply::Hang => {
                let _guard = DropCounter(state.clone());
                std::future::pending::<()>().await;
                unreachable!()
            }
        }
    }
}

/// A scripted `IngestEvent` answer.
#[derive(Clone)]
pub enum Ingest {
    Ok,
    Error {
        status: u16,
        code: &'static str,
        reason: Option<i32>,
    },
}

#[derive(Default)]
pub struct DelcoreState {
    pub users: HashMap<String, String>,
    /// Reply body for `Authorize`; `script` entries take precedence.
    pub authorize: Value,
    pub authorize_script: VecDeque<(u16, Value)>,
    pub authorize_calls: Vec<Value>,
    /// API key secret -> reply body of `AuthorizeApiKey`.
    pub keys: HashMap<String, Value>,
    pub key_calls: usize,
    pub priced: Vec<String>,
    pub paginate_catalog: bool,
    pub catalog_calls: usize,
    pub catalog_fails: bool,
    pub ingest_script: VecDeque<Ingest>,
    pub ingest_default: Option<Ingest>,
    pub ingested: Vec<Value>,
    pub tenant_states: HashMap<String, String>,
    pub scopes: Vec<String>,
    pub token_calls: usize,
    pub down: bool,
}

pub type Delcore = Arc<Mutex<DelcoreState>>;

fn connect_error(status: u16, code: &str, reason: Option<i32>) -> Response {
    let mut body = json!({"code": code, "message": "mock"});
    if let Some(reason) = reason {
        let detail = delino_protocol::delcore::v1::ErrorDetail {
            reason: delino_protocol::delcore::v1::ErrorReason::from_i32(reason)
                .unwrap_or_default()
                .into(),
            correlation_id: "c".into(),
            ..Default::default()
        };
        body["details"] = json!([{
            "type": "delcore.v1.ErrorDetail",
            "value": base64::engine::general_purpose::STANDARD_NO_PAD.encode(detail.encode_to_vec()),
        }]);
    }
    (StatusCode::from_u16(status).unwrap(), Json(body)).into_response()
}

fn service_ok(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer svc-token")
}

async fn token(State(state): State<Delcore>, headers: HeaderMap, body: String) -> Response {
    let mut s = state.lock().unwrap();
    s.token_calls += 1;
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("svc-client:svc-secret")
    );
    if s.down
        || headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(expected.as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    for pair in body.split('&') {
        if let Some(scope) = pair.strip_prefix("scope=") {
            s.scopes.push(scope.replace('+', " ").replace("%3A", ":"));
        }
    }
    Json(json!({"access_token": "svc-token", "expires_in": 3600})).into_response()
}

async fn delcore_method(
    State(state): State<Delcore>,
    axum::extract::Path(method): axum::extract::Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let mut s = state.lock().unwrap();
    if s.down {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if method == "GetCurrentUser" {
        let token = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        return match s.users.get(token) {
            Some(user) => Json(json!({"userId": user})).into_response(),
            None => connect_error(401, "unauthenticated", Some(2)),
        };
    }
    if !service_ok(&headers) {
        return connect_error(401, "unauthenticated", Some(2));
    }
    match method.as_str() {
        "Authorize" => {
            s.authorize_calls.push(body);
            if let Some((status, reply)) = s.authorize_script.pop_front() {
                return (StatusCode::from_u16(status).unwrap(), Json(reply)).into_response();
            }
            Json(s.authorize.clone()).into_response()
        }
        "AuthorizeApiKey" => {
            s.key_calls += 1;
            match s.keys.get(body["secret"].as_str().unwrap_or("")) {
                Some(reply) => Json(reply.clone()).into_response(),
                None => connect_error(401, "unauthenticated", Some(2)),
            }
        }
        "GetCatalog" => {
            s.catalog_calls += 1;
            if s.catalog_fails {
                return connect_error(500, "internal", None);
            }
            let prices: Vec<Value> = s
                .priced
                .iter()
                .map(|meter| {
                    json!({
                        "id": "p", "serviceId": SERVICE_ID, "meter": meter, "unitPrice": "0.000002",
                        "kind": "PRICE_KIND_USAGE", "active": true
                    })
                })
                .collect();
            if s.paginate_catalog && body["page"]["afterId"].is_null() {
                let half = prices.len() / 2;
                return Json(json!({
                    "prices": prices[..half],
                    "nextAfterId": "019b0000-0000-7000-8000-0000000000f1"
                }))
                .into_response();
            }
            let tail = if s.paginate_catalog {
                prices.len() / 2
            } else {
                0
            };
            Json(json!({"prices": prices[tail..]})).into_response()
        }
        "IngestEvent" => {
            let reply = s
                .ingest_script
                .pop_front()
                .or_else(|| s.ingest_default.clone())
                .unwrap_or(Ingest::Ok);
            s.ingested.push(body);
            match reply {
                Ingest::Ok => Json(json!({"eventId": "e", "revision": 1})).into_response(),
                Ingest::Error {
                    status,
                    code,
                    reason,
                } => connect_error(status, code, reason),
            }
        }
        "GetTenantStates" => {
            let states: Vec<Value> = body["tenantIds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| {
                    let id = id.as_str().unwrap();
                    json!({"tenantId": id, "state": s.tenant_states.get(id).cloned().unwrap_or("TENANT_STATE_ACTIVE".into())})
                })
                .collect();
            Json(json!({"states": states})).into_response()
        }
        _ => connect_error(404, "unimplemented", None),
    }
}

pub fn allow() -> Value {
    json!({"allowed": true})
}
pub fn deny_permission() -> Value {
    json!({"allowed": false, "reasons": ["BLOCK_REASON_PERMISSION"]})
}
pub fn deny_billing() -> Value {
    json!({"allowed": false, "reasons": ["BLOCK_REASON_BALANCE"]})
}
pub fn key_reply(tenant: &str, allowed: bool) -> Value {
    if allowed {
        json!({"allowed": true, "keyId": "k", "tenantId": tenant, "ownerKind": "API_KEY_OWNER_KIND_TENANT"})
    } else {
        json!({"allowed": false, "reasons": ["BLOCK_REASON_PERMISSION"], "keyId": "k", "tenantId": tenant})
    }
}

pub fn models() -> ModelCatalog {
    ModelCatalog::parse(
        r#"{"models":[
            {"model":"google/gemini-translate","input_meter_key":"gem_in","output_meter_key":"gem_out"},
            {"model":"openai/gpt-6-luna","input_meter_key":"luna_in","output_meter_key":"luna_out"},
            {"model":"vendor/unpriced","input_meter_key":"un_in","output_meter_key":"un_out"}
        ]}"#,
        true,
    )
    .unwrap()
}

pub fn config(delcore: &str, gateway: &str) -> Config {
    Config {
        listen: "127.0.0.1:0".into(),
        delcore: DelcoreSettings {
            url: delcore.to_owned(),
            token_url: format!("{delcore}/token"),
            resource: "https://delcore.test".into(),
            client_id: "svc-client".into(),
            client_secret: Secret::new("svc-secret"),
            service_id: SERVICE_ID.into(),
        },
        gateway: GatewaySettings {
            base_url: format!("{gateway}/v1"),
            api_key: Secret::new("gw-secret"),
            max_output_tokens: 1000,
        },
        limits: Limits {
            max_input_bytes: 4096,
            max_translation_items: 5,
            max_schema_bytes: 2048,
            execution_timeout: Duration::from_secs(10),
            tenant_rate: 1000.0,
            tenant_burst: 1000,
            tenant_concurrency: 8,
        },
        retry: RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(5),
            max_delay: Duration::from_millis(20),
        },
        outbox: OutboxSettings {
            interval: Duration::from_millis(50),
            batch: 50,
            backoff_min: Duration::from_millis(50),
            backoff_max: Duration::from_millis(400),
            alert_backlog: 100,
            alert_age: Duration::from_secs(600),
        },
        dependency_timeout: Duration::from_secs(2),
        price_cache_ttl: Duration::from_millis(1),
        models: models(),
    }
}

/// A decoded Connect JSON reply.
pub struct Wire {
    pub status: u16,
    pub request_id_header: Option<String>,
    pub retry_after: Option<String>,
    pub body: Value,
}

impl Wire {
    pub fn ok(&self) -> bool {
        self.status == 200
    }
    pub fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or("")
    }
    /// The `delitools.v1.ErrorDetail` reason name and request ID.
    pub fn detail(&self) -> (String, String) {
        let raw = self.body["details"][0]["value"]
            .as_str()
            .expect("error detail");
        let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(raw.trim_end_matches('='))
            .unwrap();
        let detail =
            delino_protocol::delitools::v1::ErrorDetail::decode_from_slice(&bytes).unwrap();
        (
            format!("{:?}", detail.reason.as_known().unwrap()),
            detail.request_id,
        )
    }
    pub fn reason(&self) -> String {
        self.detail().0
    }
}

pub struct Harness {
    pub base: String,
    pub delcore: Delcore,
    pub gateway: Arc<GatewayState>,
    pub server: Server,
    pub config: Config,
    client: reqwest::Client,
}

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}")
}

/// Serves through the production shared transport instead of `axum::serve`, so its idle-read,
/// header and write bounds apply.
async fn serve_transport(router: Router, limits: server_common::transport::Limits) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        server_common::transport::serve(listener, router, limits, std::future::pending())
            .await
            .unwrap()
    });
    format!("http://{addr}")
}

/// A pool that never connects; for tests that must not touch PostgreSQL.
pub fn unused_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://nobody:nobody@127.0.0.1:1/none")
        .unwrap()
}

impl Harness {
    pub async fn start(db: Option<sqlx::PgPool>) -> Self {
        Self::start_with(db, |_| {}).await
    }

    pub async fn start_with(db: Option<sqlx::PgPool>, tweak: impl FnOnce(&mut Config)) -> Self {
        Self::start_inner(db, None, tweak).await
    }

    /// Starts DeliTools behind the production shared transport with the given socket idle-read
    /// interval. Header, write and shutdown bounds follow the production derivation from the
    /// (tweaked) execution timeout.
    pub async fn start_transport(
        db: Option<sqlx::PgPool>,
        idle: Duration,
        tweak: impl FnOnce(&mut Config),
    ) -> Self {
        Self::start_inner(db, Some(idle), tweak).await
    }

    async fn start_inner(
        db: Option<sqlx::PgPool>,
        idle: Option<Duration>,
        tweak: impl FnOnce(&mut Config),
    ) -> Self {
        let delcore: Delcore = Arc::new(Mutex::new(DelcoreState {
            users: HashMap::from([(USER_TOKEN.to_owned(), USER.to_owned())]),
            authorize: allow(),
            keys: HashMap::from([(KEY_SECRET.to_owned(), key_reply(TENANT, true))]),
            priced: ["gem_in", "gem_out", "luna_in", "luna_out"]
                .map(String::from)
                .to_vec(),
            ..Default::default()
        }));
        let delcore_url = serve(
            Router::new()
                .route("/token", post(token))
                .route("/delcore.v1.DelcoreService/{method}", post(delcore_method))
                .with_state(delcore.clone()),
        )
        .await;
        let gateway = Arc::new(GatewayState::default());
        let gateway_url = serve(
            Router::new()
                .route("/v1/chat/completions", post(gateway_handler))
                .with_state(gateway.clone()),
        )
        .await;
        let mut config = config(&delcore_url, &gateway_url);
        tweak(&mut config);
        let server = Server::new(db.unwrap_or_else(unused_pool), config.clone()).unwrap();
        let router = http::router(server.clone());
        let base = match idle {
            None => serve(router).await,
            Some(idle) => {
                let execution = config.limits.execution_timeout;
                serve_transport(
                    router,
                    server_common::transport::Limits {
                        header: Duration::from_secs(5),
                        idle,
                        write: execution + Duration::from_secs(5),
                        shutdown: execution,
                    },
                )
                .await
            }
        };
        Self {
            base,
            delcore,
            gateway,
            server,
            config,
            client: reqwest::Client::new(),
        }
    }

    pub async fn call(&self, method: &str, token: Option<&str>, body: Value) -> Wire {
        let mut request = self
            .client
            .post(format!(
                "{}/delitools.v1.DelitoolsService/{method}",
                self.base
            ))
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .json(&body);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let request_id_header = response
            .headers()
            .get("x-delitools-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = response.json().await.unwrap_or(Value::Null);
        Wire {
            status,
            request_id_header,
            retry_after,
            body,
        }
    }

    /// Sends a request that the caller may abandon: returns the in-flight future.
    pub fn call_future(
        &self,
        method: &str,
        token: &str,
        body: Value,
    ) -> impl std::future::Future<Output = Result<reqwest::Response, reqwest::Error>> + use<> {
        let request = self
            .client
            .post(format!(
                "{}/delitools.v1.DelitoolsService/{method}",
                self.base
            ))
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .bearer_auth(token)
            .json(&body);
        request.send()
    }

    pub async fn translate(&self, token: &str, body: Value) -> Wire {
        self.call("Translate", Some(token), body).await
    }
    pub async fn extract(&self, token: &str, body: Value) -> Wire {
        self.call("Extract", Some(token), body).await
    }
}

pub fn translate_body(texts: &[&str]) -> Value {
    json!({"tenantId": TENANT, "texts": texts, "targetLanguage": "fr", "model": MODEL})
}

pub const PERSON_SCHEMA: &str = r#"{"type":"object","properties":{"name":{"type":"string"},"age":{"type":"integer"}},"required":["name"],"additionalProperties":false}"#;

pub fn extract_body() -> Value {
    json!({"tenantId": TENANT, "text": "Ada is 36 years old", "jsonSchema": PERSON_SCHEMA})
}

pub fn extracted(data: Value) -> String {
    json!({"status": "extracted", "data": data}).to_string()
}

/// A disposable PostgreSQL schema. Requires `TEST_DATABASE_URL`.
pub struct TestDb {
    pub url: String,
    pub schema: String,
    pub pool: sqlx::PgPool,
    admin: sqlx::PgPool,
}

impl TestDb {
    pub async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("delitools_{}", uuid::Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let pool = Self::connect(&url, &schema).await;
        Self {
            url,
            schema,
            pool,
            admin,
        }
    }

    /// Opens a second pool on the same schema, as a restarted process would.
    pub async fn reconnect(&self) -> sqlx::PgPool {
        Self::connect(&self.url, &self.schema).await
    }

    /// A database URL whose sessions use this schema.
    pub fn scoped_url(&self) -> String {
        let separator = if self.url.contains('?') { '&' } else { '?' };
        format!(
            "{}{separator}options=-csearch_path%3D{}",
            self.url, self.schema
        )
    }

    async fn connect(url: &str, schema: &str) -> sqlx::PgPool {
        let search = schema.to_owned();
        sqlx::postgres::PgPoolOptions::new()
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
            .connect(url)
            .await
            .unwrap()
    }

    pub async fn migrated() -> Self {
        let db = Self::new().await;
        delitools_api::store::migrate(&db.pool).await.unwrap();
        db
    }

    pub async fn finish(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
    }
}
