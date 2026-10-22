// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::{Json, Router, routing::get};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use buffa::Message;
use delcore_api::{Server, auth::Verifier, config::Config, postgres, worker::Worker};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use rsa::{RsaPrivateKey, pkcs8::DecodePrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{collections::HashMap, sync::Arc};

pub mod api_keys;
pub mod billing_domain;
pub mod billing_safeguards;
pub mod catalog;
pub mod free_allowance;
pub mod metering;

pub fn key() -> String {
    uuid::Uuid::now_v7().to_string()
}

pub struct Fixture {
    pub db: PgPool,
    pub config: Config,
    pub worker: Arc<Worker>,
    pub base: String,
    pub http: reqwest::Client,
    issuer: String,
    schema: String,
    database_url: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Fixture {
    pub async fn setup() -> Self {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter("delcore_api=debug")
            .try_init();
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("requires disposable TEST_DATABASE_URL");
        let admin = PgPool::connect(&database_url).await.unwrap();
        let schema = format!("delcore_{}", uuid::Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let search = schema.clone();
        let db = sqlx::postgres::PgPoolOptions::new()
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
            .connect(&database_url)
            .await
            .unwrap();
        postgres::migrate(&db).await.unwrap();
        postgres::migrate(&db).await.unwrap();
        postgres::check_schema(&db).await.unwrap();
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
        values.insert("REQUESTS_PER_MINUTE".into(), "100000".into());
        let config = Config::from_getter(|k| values.get(k).cloned().unwrap_or_default()).unwrap();
        let auth = Verifier::new(&config).await.unwrap();
        let server = Arc::new(Server::new(db.clone(), auth, config.clone()));
        let worker = Arc::new(Worker::new(db.clone(), config.clone()).unwrap());
        let app = delcore_api::http::router(server, worker.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let api = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            db,
            config,
            worker,
            base,
            http: reqwest::Client::new(),
            issuer,
            schema,
            database_url,
            tasks: vec![oidc, api],
        }
    }
    pub fn token(&self, actor: &str) -> String {
        let service = matches!(actor, "service" | "other" | "ghost" | "noscope" | "limited");
        let subject = if actor == "noscope" { "service" } else { actor };
        let mut claims = json!({"iss":self.issuer,"aud":self.config.audience,"sub":subject,
            "exp":chrono::Utc::now().timestamp()+3600,"email":format!("{actor}@example.test"),
            "email_verified":true,"roles":if actor=="owner" {vec![self.config.operator_role.clone()]}else{vec![]}});
        if service {
            claims["client_id"] = json!(subject);
            claims["scope"] = json!(if actor == "noscope" {
                "usage:write"
            } else {
                "authorize usage:write usage:read"
            });
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
    pub async fn response(&self, actor: &str, method: &str, body: Value) -> reqwest::Response {
        self.http
            .post(format!("{}/delcore.v1.DelcoreService/{method}", self.base))
            .bearer_auth(self.token(actor))
            .header("connect-protocol-version", "1")
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    pub async fn rpc(&self, actor: &str, method: &str, body: Value) -> Value {
        let response = self.response(actor, method, body).await;
        let status = response.status();
        let correlation = response
            .headers()
            .get("x-delcore-correlation-id")
            .map(|v| v.to_str().unwrap().to_owned());
        let body: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{actor} {method}: {status} {body}");
        assert_eq!(
            body["metadata"]["correlationId"].as_str(),
            correlation.as_deref()
        );
        assert!(
            correlation.is_some(),
            "missing correlation header: {method}"
        );
        body
    }
    pub async fn error(&self, actor: &str, method: &str, body: Value, code: &str) -> Value {
        let response = self.response(actor, method, body).await;
        assert!(
            !response.status().is_success(),
            "{method} unexpectedly succeeded"
        );
        assert!(
            response.headers().contains_key("x-delcore-correlation-id"),
            "missing error correlation header: {method}"
        );
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["code"], code, "{method}: {body}");
        let detail = body["details"]
            .as_array()
            .unwrap()
            .iter()
            .find(|detail| detail["type"] == "delcore.v1.ErrorDetail")
            .expect("typed error detail");
        let bytes = URL_SAFE_NO_PAD
            .decode(detail["value"].as_str().unwrap())
            .unwrap();
        let detail = delino_protocol::delcore::v1::ErrorDetail::decode_from_slice(&bytes).unwrap();
        assert!(
            !detail.correlation_id.is_empty(),
            "missing error detail correlation: {method}"
        );
        body
    }
    pub async fn process_metering(&self) {
        self.worker.process_metering(1000).await.unwrap();
    }
    pub async fn bootstrap(&self) {
        self.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"bootstrap fixture","plan":{"name":"free","free":true,"defaultFree":true}})).await;
    }
    pub async fn service(&self) -> String {
        self.rpc("owner","PutService",json!({"requestKey":key(),"reason":"register fixture","service":{"name":"test","clientId":"service","capabilities":["execute"],"scopes":["authorize","usage:write","usage:read"]}})).await["service"]["id"].as_str().unwrap().into()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        let url = self.database_url.clone();
        let schema = self.schema.clone();
        // A separate runtime also cleans up when an assertion unwinds the test runtime.
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async move {
                    if let Ok(pool) = PgPool::connect(&url).await {
                        let _ = sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
                            .execute(&pool)
                            .await;
                        pool.close().await;
                    }
                });
        })
        .join()
        .unwrap();
    }
}
