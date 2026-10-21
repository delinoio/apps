// SPDX-License-Identifier: FSL-1.1-ALv2
//! Shared fixture for the PostgreSQL integration tests: a per-test schema, a local mock Logto
//! (discovery and JWKS), an in-memory tenant authority and the real Connect router.
// Each integration-test crate compiles this module separately and uses a different subset.
#![allow(dead_code)]
use axum::{Json, Router, routing::get};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use buffa::Message;
use delino_protocol::delitools::v1::ErrorDetail;
use delitools_api::{
    Server,
    auth::Verifier,
    authority::{InMemoryAuthority, TenantAuthority, TenantRole},
    config::Config,
    delcore::DelcoreAuthority,
    exec::Executor,
    gateway::Gateway,
    platform::Platform,
    store::{self, Store},
    usage::{DeliveryTask, PriceSyncTask, ReconciliationTask, UsageService},
    worker::Worker,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use rsa::{RsaPrivateKey, pkcs8::DecodePrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{collections::HashMap, sync::Arc};
use uuid::Uuid;

pub const OPERATOR_ROLE: &str = "delitools-operator";

pub struct Fixture {
    pub pool: PgPool,
    pub platform: Arc<Platform>,
    pub authority: Arc<InMemoryAuthority>,
    pub base: String,
    pub http: reqwest::Client,
    pub issuer: String,
    pub audience: String,
    schema: String,
    database_url: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Present in execution mode: usage services and the background worker stop signal.
    pub usage: Option<Arc<UsageService>>,
    pub gateway_client: Option<Arc<Gateway>>,
    stop: Option<tokio::sync::watch::Sender<bool>>,
}

pub mod mocks;

/// Wiring of an execution-mode fixture: the real delcore adapter, Gateway client, execution
/// pipeline and background tasks, all pointed at mocked HTTP servers.
pub struct Execution {
    pub gateway_url: String,
    pub delcore_url: String,
    pub token_url: String,
    /// Environment overrides applied on top of `.env.example` (settings seeds included).
    pub overrides: Vec<(String, String)>,
    /// Run the background tasks (delivery, reconciliation, price sync).
    pub background: bool,
}

pub struct Reply {
    pub status: u16,
    pub request_id_header: String,
    pub body: Value,
}
impl Reply {
    pub fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or("")
    }
    /// Decode the typed `delitools.v1.ErrorDetail` attached to an error.
    pub fn detail(&self) -> ErrorDetail {
        assert_eq!(self.body["details"][0]["type"], "delitools.v1.ErrorDetail");
        let raw = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(self.body["details"][0]["value"].as_str().unwrap())
            .unwrap();
        ErrorDetail::decode_from_slice(&raw).unwrap()
    }
}

pub fn id() -> Uuid {
    Uuid::now_v7()
}

impl Fixture {
    pub async fn setup() -> Self {
        Self::build(None).await
    }

    /// Execution mode: tenant checks and usage reports go to the mocked delcore, AI calls to the
    /// mocked Gateway.
    pub async fn setup_execution(execution: Execution) -> Self {
        Self::build(Some(execution)).await
    }

    async fn build(execution: Option<Execution>) -> Self {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter("delitools_api=debug")
            .try_init();
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("requires disposable TEST_DATABASE_URL");
        let admin = PgPool::connect(&database_url).await.unwrap();
        let schema = format!("delitools_{}", Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let pool = pool_for(&database_url, &schema).await;
        store::migrate(&pool).await.unwrap();
        store::migrate(&pool).await.unwrap();
        store::check_schema(&pool).await.unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        // This committed key is a public test fixture and never authenticates production callers.
        let private = RsaPrivateKey::from_pkcs8_pem(include_str!("oidc-test-key.pem")).unwrap();
        let jwks = json!({"keys":[{"kty":"RSA","kid":"fixture","alg":"RS256","use":"sig",
            "n":URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(private.e().to_bytes_be())}]});
        let discovery = json!({"issuer":issuer,"jwks_uri":format!("{issuer}/jwks")});
        let mock = Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || async { Json(discovery) }),
            )
            .route("/jwks", get(move || async { Json(jwks) }));
        let oidc = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });

        let mut values: HashMap<String, String> =
            dotenvy::from_read_iter(include_bytes!("../../.env.example").as_slice())
                .collect::<Result<_, _>>()
                .unwrap();
        values.insert("ISSUER".into(), issuer.clone());
        values.insert("OPERATOR_ROLE".into(), OPERATOR_ROLE.into());
        if let Some(execution) = &execution {
            values.insert("GATEWAY_URL".into(), execution.gateway_url.clone());
            values.insert("DELCORE_URL".into(), execution.delcore_url.clone());
            values.insert("DELCORE_RESOURCE".into(), execution.delcore_url.clone());
            values.insert("LOGTO_TOKEN_URL".into(), execution.token_url.clone());
            for (key, value) in &execution.overrides {
                values.insert(key.clone(), value.clone());
            }
        }
        let config =
            Arc::new(Config::from_getter(|k| values.get(k).cloned().unwrap_or_default()).unwrap());
        let audience = config.audience.clone();
        let verifier = Verifier::new(
            &config.issuer,
            &config.audience,
            &config.operator_role,
            config.timeout(),
        )
        .await
        .unwrap();
        let store = Store::from_pool(pool.clone());
        store::settings::seed(&pool, &config.initial_settings, store::now())
            .await
            .unwrap();
        let settings = store::settings::load(&pool).await.unwrap();
        let authority = Arc::new(InMemoryAuthority::new());
        let mut tasks = vec![oidc];
        let (mut usage, mut gateway_client, mut stop) = (None, None, None);
        let (platform, server) = if let Some(execution) = &execution {
            let delcore = Arc::new(DelcoreAuthority::new(&config, config.timeout()).unwrap());
            let platform = Platform::new(
                store,
                config.clone(),
                verifier,
                delcore.clone() as Arc<dyn TenantAuthority>,
                settings,
            );
            let gateway = Arc::new(
                Gateway::new(
                    &config.gateway_url,
                    &config.gateway_key,
                    config.max_gateway_response_bytes,
                    config.timeout(),
                )
                .unwrap(),
            );
            let service = UsageService::new(
                platform.clone(),
                gateway.clone(),
                delcore.clone(),
                config.service_id,
            );
            let executor = Executor::new(platform.clone(), gateway.clone(), service.clone());
            if execution.background {
                let mut worker = Worker::new(platform.clone());
                worker.register(Arc::new(DeliveryTask(service.clone())));
                worker.register(Arc::new(ReconciliationTask(service.clone())));
                worker.register(Arc::new(PriceSyncTask {
                    gateway: gateway.clone(),
                }));
                let (tx, rx) = tokio::sync::watch::channel(false);
                stop = Some(tx);
                tasks.push(tokio::spawn(async move {
                    let _ = worker.run(rx).await;
                }));
            }
            usage = Some(service);
            gateway_client = Some(gateway);
            let server = Arc::new(Server::with_execution(platform.clone(), executor));
            (platform, server)
        } else {
            let platform = Platform::new(
                store,
                config,
                verifier,
                authority.clone() as Arc<dyn TenantAuthority>,
                settings,
            );
            let server = Arc::new(Server::new(platform.clone()));
            (platform, server)
        };
        let app = delitools_api::http::router(server);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tasks.push(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        Self {
            pool,
            platform,
            authority,
            base,
            http: reqwest::Client::new(),
            issuer,
            audience,
            schema,
            database_url,
            tasks,
            usage,
            gateway_client,
            stop,
        }
    }

    /// Sign a Logto-style access token.
    pub fn token(&self, subject: &str, roles: &[&str], machine: bool) -> String {
        let mut claims = json!({
            "iss": self.issuer, "aud": self.audience, "sub": subject,
            "exp": chrono::Utc::now().timestamp() + 3600, "roles": roles,
        });
        if machine {
            claims["client_id"] = json!(subject);
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("fixture".into());
        jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(include_bytes!("oidc-test-key.pem")).unwrap(),
        )
        .unwrap()
    }

    /// A user known to the tenant authority with an optional tenant role. Returns the bearer
    /// token and the platform user ID.
    pub fn user(&self, name: &str, membership: Option<(Uuid, TenantRole)>) -> (String, Uuid) {
        let token = self.token(name, &[], false);
        let user = id();
        self.authority.add_user(&token, user);
        if let Some((tenant, role)) = membership {
            self.authority.set_role(tenant, user, role);
        }
        (token, user)
    }

    pub fn operator(&self) -> String {
        self.token("platform-operator", &[OPERATOR_ROLE], false)
    }

    pub async fn call(&self, bearer: Option<&str>, method: &str, body: Value) -> Reply {
        let mut request = self
            .http
            .post(format!(
                "{}/delitools.v1.DeliToolsService/{method}",
                self.base
            ))
            .header("connect-protocol-version", "1")
            .header("X-Delitools-Request-Id", "client-supplied-value")
            .json(&body);
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let request_id_header = response
            .headers()
            .get("x-delitools-request-id")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let body = response.json().await.unwrap_or(Value::Null);
        Reply {
            status,
            request_id_header,
            body,
        }
    }

    /// Call and require success.
    pub async fn ok(&self, bearer: &str, method: &str, body: Value) -> Reply {
        let reply = self.call(Some(bearer), method, body).await;
        assert_eq!(reply.status, 200, "{method}: {}", reply.body);
        assert!(server_common::canonical_id(&reply.request_id_header));
        assert_eq!(reply.body["requestId"], reply.request_id_header);
        reply
    }

    /// Call and require a Connect error with the given code, a typed reason and a request ID.
    pub async fn fails(
        &self,
        bearer: Option<&str>,
        method: &str,
        body: Value,
        code: &str,
        reason: i32,
    ) -> Reply {
        let reply = self.call(bearer, method, body).await;
        assert_eq!(reply.code(), code, "{method}: {}", reply.body);
        let detail = reply.detail();
        assert_eq!(detail.reason.to_i32(), reason, "{method}: {}", reply.body);
        assert!(server_common::canonical_id(&detail.request_id));
        assert_eq!(detail.request_id, reply.request_id_header);
        assert_ne!(detail.request_id, "client-supplied-value");
        reply
    }

    pub async fn finish(self) {
        if let Some(stop) = &self.stop {
            let _ = stop.send(true);
        }
        for task in self.tasks {
            task.abort();
        }
        self.pool.close().await;
        let admin = PgPool::connect(&self.database_url).await.unwrap();
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}

pub async fn pool_for(database_url: &str, schema: &str) -> PgPool {
    let search = schema.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(12)
        .after_connect(move |conn, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {search}"))
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .unwrap()
}

/// Constants of `delitools.v1.ErrorReason` used by assertions.
pub mod reason {
    pub const INVALID_INPUT: i32 = 1;
    pub const INVALID_SCHEMA: i32 = 2;
    pub const INVALID_MODEL: i32 = 3;
    pub const UNAUTHENTICATED: i32 = 4;
    pub const FORBIDDEN: i32 = 5;
    pub const BILLING_BLOCKED: i32 = 6;
    pub const LIMIT: i32 = 7;
    pub const PROCESSING: i32 = 8;
    pub const ALREADY_PROCESSED: i32 = 9;
    pub const INPUT_CONFLICT: i32 = 10;
    pub const UNKNOWN_OUTCOME: i32 = 11;
    pub const EXTRACTION_IMPOSSIBLE: i32 = 12;
    pub const INVALID_OUTPUT: i32 = 13;
    pub const PROVIDER: i32 = 14;
    pub const TIMEOUT: i32 = 15;
    pub const CANCELED: i32 = 16;
    pub const PRICE_UNAVAILABLE: i32 = 17;
    pub const INTERNAL: i32 = 18;
}

pub const TRANSLATION_MODEL: &str = "anthropic/claude-sonnet-5";
pub const EXTRACTION_MODEL: &str = "openai/gpt-6-luna";
/// Default catalog prices (USD per token).
pub const LUNA_PRICE: (&str, &str, Option<&str>, Option<&str>) = (
    "0.0000001",
    "0.0000005",
    Some("0.00000001"),
    Some("0.000000125"),
);
pub const SONNET_PRICE: (&str, &str, Option<&str>, Option<&str>) = (
    "0.000003",
    "0.000015",
    Some("0.0000003"),
    Some("0.00000375"),
);

/// A complete execution environment: the real service wired to mocked Gateway and delcore.
pub struct Env {
    pub fx: Fixture,
    pub gateway: Arc<mocks::GatewayMock>,
    pub delcore: Arc<mocks::DelcoreMock>,
    pub tenant: Uuid,
}

impl Env {
    /// Start mocks and the service. `overrides` replace `.env.example` values (including the
    /// initial settings). With `background` the worker tasks run and the first price
    /// synchronization is awaited; without it prices are seeded directly.
    pub async fn start(overrides: &[(&str, &str)], background: bool) -> Env {
        let gateway = mocks::GatewayMock::start().await;
        let delcore = mocks::DelcoreMock::start().await;
        publish_catalog(&gateway);
        let mut all: Vec<(String, String)> = vec![
            (
                "TRANSLATION_MODELS".into(),
                format!("[\"{TRANSLATION_MODEL}\",\"{EXTRACTION_MODEL}\"]"),
            ),
            ("MAX_AI_RETRIES".into(), "2".into()),
            ("DELIVERY_RETRY_SECONDS".into(), "1".into()),
            ("MAX_BACKOFF_SECONDS".into(), "4".into()),
            ("PRICE_SYNC_SECONDS".into(), "1".into()),
            ("TIMEOUT_SECONDS".into(), "5".into()),
        ];
        all.extend(
            overrides
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
        );
        let fx = Fixture::setup_execution(Execution {
            gateway_url: gateway.url.clone(),
            delcore_url: delcore.url.clone(),
            token_url: delcore.token_url.clone(),
            overrides: all,
            background,
        })
        .await;
        let env = Env {
            fx,
            gateway,
            delcore,
            tenant: id(),
        };
        env.delcore.add_tenant(env.tenant);
        if background {
            env.wait_for_price(EXTRACTION_MODEL).await;
        } else {
            env.seed_prices().await;
        }
        env
    }

    /// Insert effective-dated catalog prices directly (used when the sync task is not running).
    pub async fn seed_prices(&self) {
        for (model, route, price) in [
            (TRANSLATION_MODEL, "anthropic", SONNET_PRICE),
            (EXTRACTION_MODEL, "openai", LUNA_PRICE),
        ] {
            self.set_price(model, route, price).await;
        }
    }

    pub async fn set_price(
        &self,
        model: &str,
        route: &str,
        price: (&str, &str, Option<&str>, Option<&str>),
    ) {
        sqlx::query("INSERT INTO catalog_prices(id,model,provider_route,effective_at,input_usd_per_token,output_usd_per_token,cache_read_usd_per_token,cache_write_usd_per_token,synced_at,created_at) VALUES($1,$2,$3,now()-interval '1 minute',$4::numeric,$5::numeric,$6::numeric,$7::numeric,now(),now())")
            .bind(id())
            .bind(model)
            .bind(route)
            .bind(price.0)
            .bind(price.1)
            .bind(price.2)
            .bind(price.3)
            .execute(&self.fx.pool)
            .await
            .unwrap();
    }

    pub async fn wait_for_price(&self, model: &str) {
        for _ in 0..100 {
            let found: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog_prices WHERE model=$1)")
                    .bind(model)
                    .fetch_one(&self.fx.pool)
                    .await
                    .unwrap();
            if found {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("price for {model} was not synchronized");
    }

    /// A Logto user token known to the mocked delcore, optionally a member of the tenant.
    pub fn user(&self, name: &str, role: Option<mocks::MemberRole>) -> (String, Uuid) {
        let token = self.fx.token(name, &[], false);
        let user = id();
        self.delcore.add_user(&token, user);
        if let Some(role) = role {
            self.delcore.set_role(self.tenant, user, role);
        }
        (token, user)
    }

    /// Create a key through the public RPC and return `(secret, key id)`.
    pub async fn create_key(&self, bearer: &str, kind: &str, caps: &[&str]) -> (String, String) {
        let reply = self
            .fx
            .ok(
                bearer,
                "CreateKey",
                json!({"tenantId": self.tenant.to_string(), "kind": kind, "capabilities": caps}),
            )
            .await;
        (
            reply.body["secret"].as_str().unwrap().to_owned(),
            reply.body["key"]["id"].as_str().unwrap().to_owned(),
        )
    }

    /// A service key with both capabilities, created by a fresh tenant owner.
    pub async fn service_key(&self) -> String {
        let (owner, _) = self.user("service-owner", Some(mocks::MemberRole::Owner));
        self.create_key(
            &owner,
            "KEY_KIND_SERVICE",
            &["CAPABILITY_TRANSLATOR", "CAPABILITY_EXTRACTOR"],
        )
        .await
        .0
    }

    pub fn translate_body(&self, texts: &[&str], key: Option<&str>) -> Value {
        let mut body = json!({
            "tenantId": self.tenant.to_string(), "outputLanguage": "ko",
            "model": TRANSLATION_MODEL, "texts": texts,
        });
        if let Some(key) = key {
            body["idempotencyKey"] = json!(key);
        }
        body
    }

    pub fn extract_body(&self, text: &str, schema: &str, key: Option<&str>) -> Value {
        let mut body = json!({
            "tenantId": self.tenant.to_string(), "text": text, "jsonSchema": schema,
        });
        if let Some(key) = key {
            body["idempotencyKey"] = json!(key);
        }
        body
    }

    /// Make every pending attempt due now (the next background run picks it up).
    pub async fn make_due(&self) {
        sqlx::query(
            "UPDATE attempts SET retry_at=now()-interval '1 second' WHERE delivered_at IS NULL",
        )
        .execute(&self.fx.pool)
        .await
        .unwrap();
    }

    /// Wait until `count` distinct events were accepted by the mocked delcore.
    pub async fn wait_for_events(&self, count: usize) -> HashMap<String, Value> {
        for _ in 0..150 {
            let accepted = self.delcore.accepted();
            if accepted.len() >= count {
                return accepted;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!(
            "expected {count} accepted events, got {}",
            self.delcore.accepted().len()
        );
    }

    pub async fn attempt_rows(&self) -> Vec<(String, bool, bool)> {
        sqlx::query_as::<_, (String, bool, bool)>(
            "SELECT id::text, measured_usage IS NOT NULL, delivered_at IS NOT NULL FROM attempts ORDER BY created_at,id",
        )
        .fetch_all(&self.fx.pool)
        .await
        .unwrap()
    }

    pub async fn finish(self) {
        self.gateway.stop();
        self.delcore.stop();
        self.fx.finish().await;
    }
}

/// The default public catalog: both models priced, one matching active provider each.
pub fn publish_catalog(gateway: &mocks::GatewayMock) {
    let entry = |model: &str, p: (&str, &str, Option<&str>, Option<&str>)| {
        mocks::catalog_entry(model, p.0, p.1, p.2, p.3)
    };
    let endpoint = |provider: &str, p: (&str, &str, Option<&str>, Option<&str>)| {
        mocks::endpoint_entry(provider, p.0, p.1, p.2, p.3)
    };
    gateway.set_catalog(
        vec![
            entry(TRANSLATION_MODEL, SONNET_PRICE),
            entry(EXTRACTION_MODEL, LUNA_PRICE),
            // A model that is priced but not allowlisted must never become executable.
            entry(
                "google/gemini-unlisted",
                ("0.000001", "0.000002", None, None),
            ),
        ],
        vec![
            (TRANSLATION_MODEL, vec![endpoint("anthropic", SONNET_PRICE)]),
            (
                EXTRACTION_MODEL,
                vec![
                    endpoint("openai", LUNA_PRICE),
                    endpoint("azure", ("0.00000012", "0.0000006", None, None)),
                ],
            ),
        ],
    );
}

/// A translation answer for the given JSON content.
pub fn translation_content(texts: &[&str]) -> String {
    json!({"translations": texts}).to_string()
}

/// The requested texts echoed back with a prefix, as a fake translator.
pub fn echo_translations(request: &Value) -> String {
    let user: Value =
        serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
    let texts: Vec<String> = user["texts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            format!(
                "[{}] {}",
                user["targetLanguage"].as_str().unwrap(),
                t.as_str().unwrap()
            )
        })
        .collect();
    json!({"translations": texts}).to_string()
}

pub fn extraction_content(status: &str, data: Value) -> String {
    json!({"status": status, "data": data}).to_string()
}

pub const INVOICE_SCHEMA: &str = r#"{"type":"object","properties":{"vendor":{"type":"string"},"total":{"type":"number"},"currency":{"type":"string","enum":["USD","EUR"]}},"required":["vendor","total","currency"],"additionalProperties":false}"#;
