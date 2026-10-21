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
    platform::Platform,
    store::{self, Store},
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
        let platform = Platform::new(
            store,
            config,
            verifier,
            authority.clone() as Arc<dyn TenantAuthority>,
            settings,
        );
        let app = delitools_api::http::router(Arc::new(Server::new(platform.clone())));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let api = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
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
            tasks: vec![oidc, api],
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
    pub const UNAUTHENTICATED: i32 = 4;
    pub const FORBIDDEN: i32 = 5;
    pub const BILLING_BLOCKED: i32 = 6;
    pub const INTERNAL: i32 = 18;
}
