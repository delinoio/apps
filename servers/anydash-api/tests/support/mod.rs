// SPDX-License-Identifier: FSL-1.1-ALv2
//! Disposable PostgreSQL schema, mocked Logto, Delcore and LiteLLM, and a Connect JSON client.
#![allow(dead_code)]
use anydash_api::{Server, config::Config, worker::Job};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use base64::Engine;
use buffa::Message;
use delino_protocol::anydash::v1::ErrorDetail;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgConnectOptions};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub const RESOURCE: &str = "https://anydash.api.delino.io";
pub const DELCORE_RESOURCE: &str = "https://delcore.api.delino.io";

// Public test fixture only; this key never signs production credentials.
const TEST_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQglAnLdw1E7sKU4Od0
KUR6w73q0kPwV2ZuIrCYCc5O+1+hRANCAAS7WL2yuRlkebV9eg5DC8EgeL5sLCc+
SWa6SCqGYyxWx3gNrhl069h8P2zMhX5WKLrOwiNGwCHc0xVxIsdq2G49
-----END PRIVATE KEY-----
"#;

pub fn id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// One mocked Delcore user.
#[derive(Clone, Debug)]
pub struct Person {
    pub subject: String,
    pub user_id: String,
    pub personal: String,
}

/// One mocked team membership: fixed roles and the start of the current membership, which
/// survives role changes and is renewed when the user leaves and rejoins, as in Delcore.
#[derive(Clone, Debug)]
pub struct Membership {
    pub roles: Vec<&'static str>,
    pub since: String,
}

impl Membership {
    fn new(roles: &[&'static str]) -> Self {
        Self {
            roles: roles.to_vec(),
            since: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        }
    }
}

/// Mutable state of the mocked dependencies.
#[derive(Default)]
pub struct Mock {
    pub users: HashMap<String, Person>,
    /// Team tenants: id → (name, members user_id → membership).
    pub teams: HashMap<String, (String, HashMap<String, Membership>)>,
    /// Tenant states by tenant ID; unknown IDs are UNKNOWN, known ones ACTIVE by default.
    pub states: HashMap<String, &'static str>,
    pub delcore_down: bool,
    pub prices: (u16, String),
    pub calls: Vec<String>,
}

pub struct Fixture {
    pub db: PgPool,
    pub server: Server,
    pub base: String,
    pub http: reqwest::Client,
    pub mock: Arc<Mutex<Mock>>,
    pub issuer: String,
    schema: String,
    database_url: String,
}

fn sign(claims: Value) -> String {
    let mut h = Header::new(Algorithm::ES256);
    h.kid = Some("fixture".into());
    jsonwebtoken::encode(
        &h,
        &claims,
        &EncodingKey::from_ec_pem(TEST_KEY.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn subject_of(headers: &HeaderMap) -> String {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let payload = token.split('.').nth(1).unwrap_or_default();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .unwrap_or_default();
    let claims: Value = serde_json::from_slice(&bytes).unwrap_or_default();
    claims["sub"].as_str().unwrap_or_default().to_owned()
}

type Shared = Arc<Mutex<Mock>>;

async fn current_user(
    State(m): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let m = m.lock().unwrap();
    if m.delcore_down {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"code":"unavailable"})),
        );
    }
    let Some(person) = m.users.get(&subject_of(&headers)).cloned() else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"code":"unauthenticated"})),
        );
    };
    // One tenant per page so that callers must follow next_after_id.
    let mut tenants = vec![
        json!({"id":person.personal,"name":"Personal","personal":true,"ownerId":person.user_id}),
    ];
    let mut teams: Vec<_> = m
        .teams
        .iter()
        .filter(|(_, (_, members))| members.contains_key(&person.user_id))
        .collect();
    teams.sort_by_key(|(id, _)| id.to_string());
    for (id, (name, _)) in teams {
        tenants.push(json!({"id":id,"name":name,"personal":false,"ownerId":""}));
    }
    let index: usize = body["page"]["afterId"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let next = if index + 1 < tenants.len() {
        (index + 1).to_string()
    } else {
        String::new()
    };
    (
        StatusCode::OK,
        Json(
            json!({"userId":person.user_id,"tenants":[tenants[index].clone()],"nextAfterId":next}),
        ),
    )
}

async fn tenant_states(State(m): State<Shared>, Json(body): Json<Value>) -> impl IntoResponse {
    let mut m = m.lock().unwrap();
    m.calls.push("GetTenantStates".into());
    if m.delcore_down {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"code":"unavailable"})),
        );
    }
    let known: Vec<String> = m
        .users
        .values()
        .map(|p| p.personal.clone())
        .chain(m.teams.keys().cloned())
        .collect();
    let states: Vec<Value> = body["tenantIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let t = t.as_str().unwrap();
            let state = m
                .states
                .get(t)
                .copied()
                .unwrap_or(if known.iter().any(|k| k == t) {
                    "TENANT_STATE_ACTIVE"
                } else {
                    "TENANT_STATE_UNKNOWN"
                });
            json!({"tenantId":t,"state":state})
        })
        .collect();
    (StatusCode::OK, Json(json!({"states":states})))
}

async fn member_roles(State(m): State<Shared>, Json(body): Json<Value>) -> impl IntoResponse {
    let mut m = m.lock().unwrap();
    m.calls.push("GetTenantMemberRoles".into());
    if m.delcore_down {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"code":"unavailable"})),
        );
    }
    let tenant = body["tenantId"].as_str().unwrap().to_owned();
    let deleted = m.states.get(&tenant) == Some(&"TENANT_STATE_DELETED");
    let members: Vec<Value> = body["userIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            let u = u.as_str().unwrap();
            let personal_owner = m
                .users
                .values()
                .any(|p| p.personal == tenant && p.user_id == u);
            let membership = if personal_owner {
                Some(Membership {
                    roles: vec!["ROLE_OWNER"],
                    since: "2026-01-01T00:00:00Z".into(),
                })
            } else {
                m.teams
                    .get(&tenant)
                    .and_then(|(_, members)| members.get(u).cloned())
            };
            match membership {
                Some(m) if !deleted => {
                    json!({"userId":u,"member":true,"roles":m.roles,"memberSince":m.since})
                }
                _ => json!({"userId":u,"member":false}),
            }
        })
        .collect();
    (StatusCode::OK, Json(json!({"members":members})))
}

async fn prices(State(m): State<Shared>) -> impl IntoResponse {
    let (status, body) = m.lock().unwrap().prices.clone();
    (StatusCode::from_u16(status).unwrap(), body)
}

/// A synthetic LiteLLM document with 120 models.
pub fn price_document(gpt_input: f64) -> String {
    let mut map = serde_json::Map::new();
    map.insert(
        "gpt-5".into(),
        json!({"input_cost_per_token":gpt_input,"output_cost_per_token":0.00001,"cache_read_input_token_cost":0.000000125}),
    );
    map.insert(
        "anthropic/claude-opus-4".into(),
        json!({"input_cost_per_token":0.000015,"output_cost_per_token":0.000075,"cache_read_input_token_cost":0.0000015,"cache_creation_input_token_cost":0.00001875}),
    );
    for i in 0..118 {
        map.insert(
            format!("model-{i}"),
            json!({"input_cost_per_token":0.000001}),
        );
    }
    Value::Object(map).to_string()
}

/// A Connect error: code, message and decoded detail.
#[derive(Debug)]
pub struct Failure {
    pub status: u16,
    pub code: String,
    pub detail: ErrorDetail,
    pub correlation: String,
    pub retry_after: Option<String>,
}

pub struct Caller {
    pub authorization: Option<String>,
    pub kind: &'static str,
    pub version: &'static str,
}

impl Fixture {
    pub async fn setup() -> Self {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("requires disposable TEST_DATABASE_URL");
        let admin = PgPool::connect(&database_url).await.unwrap();
        let schema = format!("anydash_{}", uuid::Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let db = schema_pool(&database_url, &schema, 16).await;
        assert!(
            anydash_api::migration::check(&db).await.is_err(),
            "an empty schema is refused"
        );
        anydash_api::migration::migrate(&db).await.unwrap();
        anydash_api::migration::migrate(&db).await.unwrap();
        anydash_api::migration::check(&db).await.unwrap();
        let mock: Shared = Arc::new(Mutex::new(Mock {
            prices: (200, price_document(0.00000125)),
            ..Default::default()
        }));
        let jwks = json!({"keys":[{"kty":"EC","crv":"P-256","alg":"ES256","use":"sig","kid":"fixture","x":"u1i9srkZZHm1fXoOQwvBIHi-bCwnPklmukgqhmMsVsc","y":"eA2uGXTr2Hw_bMyFflYous7CI0bAIdzTFXEix2rYbj0"}]});
        let app = Router::new()
            .route("/oidc/jwks", get(move || async move { Json(jwks.clone()) }))
            .route(
                "/token",
                post(|| async { Json(json!({"access_token":"m2m","expires_in":3600})) }),
            )
            .route(
                "/delcore.v1.DelcoreService/GetCurrentUser",
                post(current_user),
            )
            .route(
                "/delcore.v1.DelcoreService/GetTenantStates",
                post(tenant_states),
            )
            .route(
                "/delcore.v1.DelcoreService/GetTenantMemberRoles",
                post(member_roles),
            )
            .route("/prices.json", get(prices))
            .with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let issuer = format!("{mock_base}/oidc");
        let config = Config::fixture(&issuer, &mock_base, &format!("{mock_base}/prices.json"));
        let server = Server::new(db.clone(), config).unwrap();
        let base = serve(&server).await;
        Self {
            db,
            server,
            base,
            http: reqwest::Client::new(),
            mock,
            issuer,
            schema,
            database_url,
        }
    }

    /// A second API instance on the same schema and mocks whose pool holds only `connections`
    /// connections and gives up acquiring one after two seconds. It shows that a handler never
    /// waits for another pooled connection while its transaction holds one. Close its pool with
    /// `db.close()`; only the original fixture drops the schema.
    pub async fn with_pool_size(&self, connections: u32) -> Self {
        let db =
            sqlx::postgres::PgPoolOptions::new().acquire_timeout(std::time::Duration::from_secs(2));
        let db = schema_pool_with(db, &self.database_url, &self.schema, connections, None).await;
        let server = Server::new(db.clone(), (*self.server.config).clone()).unwrap();
        let base = serve(&server).await;
        Self {
            db,
            server,
            base,
            http: reqwest::Client::new(),
            mock: self.mock.clone(),
            issuer: self.issuer.clone(),
            schema: self.schema.clone(),
            database_url: self.database_url.clone(),
        }
    }

    /// A second API instance on the same schema and mocks whose connections give up waiting for a
    /// row lock after `lock_timeout_ms`, so a lock held by `lock_row` turns into a database error
    /// instead of a wait. Close its pool with `db.close()`; only the original fixture drops the
    /// schema.
    pub async fn with_lock_timeout(&self, lock_timeout_ms: u32) -> Self {
        let db = schema_pool_with(
            sqlx::postgres::PgPoolOptions::new(),
            &self.database_url,
            &self.schema,
            4,
            Some(lock_timeout_ms),
        )
        .await;
        let server = Server::new(db.clone(), (*self.server.config).clone()).unwrap();
        let base = serve(&server).await;
        Self {
            db,
            server,
            base,
            http: reqwest::Client::new(),
            mock: self.mock.clone(),
            issuer: self.issuer.clone(),
            schema: self.schema.clone(),
            database_url: self.database_url.clone(),
        }
    }

    /// Holds a row lock taken by `select_for_update` (a `SELECT ... FOR UPDATE` statement) until
    /// the returned transaction is dropped or rolled back.
    pub async fn lock_row(
        &self,
        select_for_update: &str,
    ) -> sqlx::Transaction<'static, sqlx::Postgres> {
        let mut tx = self.db.begin().await.unwrap();
        sqlx::query(select_for_update)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx
    }

    pub async fn cleanup(self) {
        self.db.close().await;
        let admin = PgPool::connect(&self.database_url).await.unwrap();
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }

    /// Registers a Delcore user with a personal tenant.
    pub fn person(&self, name: &str) -> Person {
        let p = Person {
            subject: format!("logto-{name}-{}", id()),
            user_id: id(),
            personal: id(),
        };
        self.mock
            .lock()
            .unwrap()
            .users
            .insert(p.subject.clone(), p.clone());
        p
    }

    pub fn team(&self, name: &str, members: &[(&Person, &[&'static str])]) -> String {
        let tenant = id();
        let map = members
            .iter()
            .map(|(p, r)| (p.user_id.clone(), Membership::new(r)))
            .collect();
        self.mock
            .lock()
            .unwrap()
            .teams
            .insert(tenant.clone(), (name.into(), map));
        tenant
    }

    /// Changes a person's team roles. Empty roles remove the membership; roles for a person who
    /// is not a member start a new membership, while a role change keeps the membership start.
    pub fn set_roles(&self, tenant: &str, person: &Person, roles: &[&'static str]) {
        let mut m = self.mock.lock().unwrap();
        let members = &mut m.teams.get_mut(tenant).unwrap().1;
        if roles.is_empty() {
            members.remove(&person.user_id);
        } else if let Some(existing) = members.get_mut(&person.user_id) {
            existing.roles = roles.to_vec();
        } else {
            members.insert(person.user_id.clone(), Membership::new(roles));
        }
    }

    pub fn set_state(&self, tenant: &str, state: &'static str) {
        self.mock
            .lock()
            .unwrap()
            .states
            .insert(tenant.into(), state);
    }

    pub fn token(&self, p: &Person, audience: &str) -> String {
        let now = chrono::Utc::now().timestamp();
        sign(json!({"iss":self.issuer,"aud":audience,"sub":p.subject,"exp":now+3600,"iat":now}))
    }

    pub fn user(&self, p: &Person) -> Caller {
        Caller {
            authorization: Some(format!("Bearer {}", self.token(p, RESOURCE))),
            kind: "CLIENT_KIND_WEB",
            version: "1.0.0",
        }
    }

    pub fn device(&self, access: &str) -> Caller {
        Caller {
            authorization: Some(format!("Bearer {access}")),
            kind: "CLIENT_KIND_CLI",
            version: "1.0.0",
        }
    }

    pub fn anonymous(&self) -> Caller {
        Caller {
            authorization: None,
            kind: "CLIENT_KIND_CLI",
            version: "1.0.0",
        }
    }

    /// Calls a method with Connect JSON and returns the body or the decoded failure.
    pub async fn call(
        &self,
        caller: &Caller,
        method: &str,
        body: Value,
    ) -> Result<Value, Box<Failure>> {
        self.send(
            caller,
            method,
            "application/json",
            body.to_string().into_bytes(),
        )
        .await
    }

    /// Calls a method with a binary protobuf request and expects a failure with the given
    /// `ErrorReason` value name. ProtoJSON cannot carry timestamps outside years 1 to 9999, so
    /// the binary encoding is the only way to send them. Error responses stay Connect JSON.
    pub async fn fail_proto(
        &self,
        caller: &Caller,
        method: &str,
        request: &impl Message,
        reason: &str,
    ) -> Box<Failure> {
        match self
            .send(caller, method, "application/proto", request.encode_to_vec())
            .await
        {
            Ok(v) => panic!("{method} unexpectedly succeeded: {v}"),
            Err(e) => {
                use buffa::Enumeration;
                let name = e
                    .detail
                    .reason
                    .as_known()
                    .map(|r| r.proto_name())
                    .unwrap_or_default();
                assert_eq!(name, reason, "{method}: {e:?}");
                e
            }
        }
    }

    async fn send(
        &self,
        caller: &Caller,
        method: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<Value, Box<Failure>> {
        let mut request = self
            .http
            .post(format!("{}/anydash.v1.{method}", self.base))
            .header("content-type", content_type)
            .header("connect-protocol-version", "1")
            .header("x-anydash-client-kind", caller.kind)
            .header("x-anydash-client-version", caller.version)
            .body(body);
        if let Some(a) = &caller.authorization {
            request = request.header("authorization", a);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let correlation = response
            .headers()
            .get("x-anydash-correlation-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        assert!(
            !correlation.is_empty(),
            "every response carries a correlation ID"
        );
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let value: Value = response.json().await.unwrap_or(Value::Null);
        if status == 200 {
            return Ok(value);
        }
        let details = value["details"].as_array().cloned().unwrap_or_default();
        assert_eq!(details.len(), 1, "exactly one ErrorDetail: {value}");
        assert_eq!(details[0]["type"], "anydash.v1.ErrorDetail");
        let encoded = details[0]["value"].as_str().unwrap().trim_end_matches('=');
        let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(encoded)
            .unwrap();
        let detail = ErrorDetail::decode_from_slice(&bytes).unwrap();
        assert_eq!(
            detail.correlation_id, correlation,
            "detail correlation equals the header"
        );
        Err(Box::new(Failure {
            status,
            code: value["code"].as_str().unwrap_or_default().into(),
            detail,
            correlation,
            retry_after,
        }))
    }

    pub async fn ok(&self, caller: &Caller, method: &str, body: Value) -> Value {
        match self.call(caller, method, body).await {
            Ok(v) => v,
            Err(e) => panic!("{method} failed: {e:?}"),
        }
    }

    /// Expects a failure with the given `ErrorReason` value name.
    pub async fn fail(
        &self,
        caller: &Caller,
        method: &str,
        body: Value,
        reason: &str,
    ) -> Box<Failure> {
        match self.call(caller, method, body).await {
            Ok(v) => panic!("{method} unexpectedly succeeded: {v}"),
            Err(e) => {
                use buffa::Enumeration;
                let name = e
                    .detail
                    .reason
                    .as_known()
                    .map(|r| r.proto_name())
                    .unwrap_or_default();
                assert_eq!(name, reason, "{method}: {e:?}");
                e
            }
        }
    }

    /// Connects the AnyDash account of a person.
    pub async fn connect(&self, p: &Person) -> Value {
        self.ok(
            &self.user(p),
            "AccountService/ConnectAccount",
            json!({"delcoreAccessToken": self.token(p, DELCORE_RESOURCE)}),
        )
        .await
    }

    /// Enrolls a collector device for a person and returns (device ID, access, refresh).
    pub async fn enroll(&self, p: &Person, name: &str, previous: &str) -> (String, String, String) {
        let start = self
            .ok(
                &self.anonymous(),
                "DeviceService/StartEnrollment",
                json!({"deviceName":name,"platform":"DEVICE_PLATFORM_MACOS","architecture":"CPU_ARCHITECTURE_ARM64","collectorKind":"COLLECTOR_KIND_CLI","collectorVersion":"1.0.0","osVersion":"14.5","previousDeviceId":previous}),
            )
            .await;
        let enrollment = start["enrollmentId"].as_str().unwrap().to_owned();
        self.ok(
            &self.user(p),
            "DeviceService/ApproveEnrollment",
            json!({"enrollmentId":enrollment,"userCode":start["userCode"]}),
        )
        .await;
        let polled = self
            .ok(
                &self.anonymous(),
                "DeviceService/PollEnrollment",
                json!({"enrollmentId":enrollment,"deviceCode":start["deviceCode"]}),
            )
            .await;
        assert_eq!(polled["state"], "ENROLLMENT_STATE_CONSUMED");
        (
            polled["device"]["id"].as_str().unwrap().into(),
            polled["credentials"]["accessToken"]
                .as_str()
                .unwrap()
                .into(),
            polled["credentials"]["refreshToken"]
                .as_str()
                .unwrap()
                .into(),
        )
    }

    /// Runs a worker job now, waiting for its advisory lock held by a parallel test.
    pub async fn job(&self, job: Job) {
        for _ in 0..200 {
            if anydash_api::worker::run(&self.server, job, anydash_api::time::now(), true)
                .await
                .unwrap()
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("job {} did not get its lock", job.name());
    }

    pub async fn exec(&self, sql: &str) {
        sqlx::raw_sql(sql).execute(&self.db).await.unwrap();
    }

    pub async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.db).await.unwrap()
    }
}

/// A pool whose connections use the test schema.
async fn schema_pool(database_url: &str, schema: &str, connections: u32) -> PgPool {
    schema_pool_with(
        sqlx::postgres::PgPoolOptions::new(),
        database_url,
        schema,
        connections,
        None,
    )
    .await
}

async fn schema_pool_with(
    options: sqlx::postgres::PgPoolOptions,
    database_url: &str,
    schema: &str,
    connections: u32,
    lock_timeout_ms: Option<u32>,
) -> PgPool {
    let search = schema.to_owned();
    options
        .max_connections(connections)
        .after_connect(move |conn, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {search}"))
                    .execute(&mut *conn)
                    .await?;
                if let Some(ms) = lock_timeout_ms {
                    sqlx::query(&format!("SET lock_timeout = {ms}"))
                        .execute(&mut *conn)
                        .await?;
                }
                Ok(())
            })
        })
        .connect_with(
            database_url
                .parse::<PgConnectOptions>()
                .unwrap()
                .application_name(schema),
        )
        .await
        .unwrap()
}

/// Serves the API router on a loopback port and returns its base URL.
async fn serve(server: &Server) -> String {
    let router = anydash_api::http::router(Arc::new(server.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    base
}

/// Timestamp text for a JSON request.
pub fn at(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
