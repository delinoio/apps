// SPDX-License-Identifier: FSL-1.1-ALv2
//! Loopback tests for GitHub Actions JWKS refreshes. The private keys are public test fixtures.
use axum::{Json, Router, http::StatusCode, routing::get};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, jwk::JwkSet};
use perfine_api::auth::Actions;
use rsa::{RsaPrivateKey, pkcs8::DecodePrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Semaphore, time::timeout};

const KEY: &[u8] = include_bytes!("fixtures/github-test-key.pem");
const ROTATED_KEY: &[u8] = include_bytes!("fixtures/github-rotated-test-key.pem");

struct Gate {
    arrived: Semaphore,
    release: Semaphore,
}
impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            arrived: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }
    async fn wait_arrival(&self) {
        timeout(Duration::from_secs(5), self.arrived.acquire())
            .await
            .expect("JWKS request did not arrive")
            .unwrap()
            .forget();
    }
    fn release(&self) {
        self.release.add_permits(1);
    }
}

#[derive(Clone)]
enum Mode {
    Serve,
    Gate(Arc<Gate>),
    Status(StatusCode),
}
struct Provider {
    keys: Mutex<Value>,
    mode: Mutex<Mode>,
    requests: AtomicUsize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}
struct InFlight<'a>(&'a AtomicUsize);
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl Provider {
    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
    fn set_mode(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }
    fn set_keys(&self, keys: Vec<Value>) {
        *self.keys.lock().unwrap() = json!({"keys": keys});
    }
    async fn jwks(self: Arc<Self>) -> (StatusCode, Json<Value>) {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(in_flight, Ordering::SeqCst);
        let _in_flight = InFlight(&self.in_flight);
        let mode = self.mode.lock().unwrap().clone();
        let keys = self.keys.lock().unwrap().clone();
        match mode {
            Mode::Serve => (StatusCode::OK, Json(keys)),
            Mode::Gate(gate) => {
                gate.arrived.add_permits(1);
                gate.release.acquire().await.unwrap().forget();
                (StatusCode::OK, Json(keys))
            }
            Mode::Status(status) => (status, Json(json!({}))),
        }
    }
}

fn rsa_jwk(pem: &[u8], kid: &str) -> Value {
    let private = RsaPrivateKey::from_pkcs8_pem(std::str::from_utf8(pem).unwrap()).unwrap();
    json!({"kty":"RSA","kid":kid,"alg":"RS256","use":"sig",
        "n":URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),
        "e":URL_SAFE_NO_PAD.encode(private.e().to_bytes_be())})
}
fn token(pem: &[u8], kid: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.into());
    let now = chrono::Utc::now().timestamp();
    let claims = json!({"iss":"https://token.actions.githubusercontent.com",
        "aud":"https://perfine.api.delino.io","sub":"repo:owner/repo:ref:refs/heads/main",
        "iat":now,"nbf":now-1,"exp":now+300});
    jsonwebtoken::encode(&header, &claims, &EncodingKey::from_rsa_pem(pem).unwrap()).unwrap()
}

struct Harness {
    actions: Arc<Actions>,
    provider: Arc<Provider>,
}
impl Harness {
    async fn new(client_timeout: Duration) -> Self {
        let provider = Arc::new(Provider {
            keys: Mutex::new(json!({"keys":[rsa_jwk(KEY, "cached")]})),
            mode: Mutex::new(Mode::Serve),
            requests: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/jwks", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/jwks",
            get({
                let provider = provider.clone();
                move || provider.clone().jwks()
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder()
            .timeout(client_timeout)
            .build()
            .unwrap();
        let keys: JwkSet =
            serde_json::from_value(json!({"keys":[rsa_jwk(KEY, "cached")]})).unwrap();
        Self {
            actions: Arc::new(Actions::new(client, url, keys)),
            provider,
        }
    }
    fn spawn_verify(&self, raw: String) -> tokio::task::JoinHandle<perfine_api::Result<Value>> {
        let actions = self.actions.clone();
        tokio::spawn(async move { actions.verify(&raw).await })
    }
    async fn verify_cached_promptly(&self) {
        timeout(
            Duration::from_secs(1),
            self.actions.verify(&token(KEY, "cached")),
        )
        .await
        .expect("cached verification waited for JWKS")
        .unwrap();
    }
}

#[tokio::test]
async fn cached_token_verifies_while_unknown_key_refresh_waits() {
    let h = Harness::new(Duration::from_secs(5)).await;
    let gate = Gate::new();
    h.provider.set_mode(Mode::Gate(gate.clone()));
    let unknown = h.spawn_verify(token(KEY, "unknown"));
    gate.wait_arrival().await;
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        gate.release();
    });
    h.verify_cached_promptly().await;
    assert_eq!(h.provider.requests(), 1);
    assert!(!unknown.is_finished());
    release.await.unwrap();
    assert_eq!(
        unknown.await.unwrap().unwrap_err().code,
        perfine_api::Fault::denied().code
    );
    h.verify_cached_promptly().await;
}

#[tokio::test]
async fn failed_and_cancelled_refreshes_preserve_cached_keys() {
    let h = Harness::new(Duration::from_millis(150)).await;
    h.provider
        .set_mode(Mode::Status(StatusCode::INTERNAL_SERVER_ERROR));
    let fault = h.actions.verify(&token(KEY, "unknown")).await.unwrap_err();
    assert_eq!(fault.code, perfine_api::Fault::unavailable().code);
    h.verify_cached_promptly().await;

    let gate = Gate::new();
    h.provider.set_mode(Mode::Gate(gate.clone()));
    let timed_out = h.spawn_verify(token(KEY, "unknown"));
    gate.wait_arrival().await;
    let fault = timed_out.await.unwrap().unwrap_err();
    assert_eq!(fault.code, perfine_api::Fault::unavailable().code);
    h.verify_cached_promptly().await;
    gate.release();

    let cancelled_gate = Gate::new();
    h.provider.set_mode(Mode::Gate(cancelled_gate.clone()));
    let cancelled = h.spawn_verify(token(KEY, "unknown"));
    cancelled_gate.wait_arrival().await;
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    h.verify_cached_promptly().await;
    cancelled_gate.release();
    h.provider.set_mode(Mode::Serve);
    let fault = h.actions.verify(&token(KEY, "unknown")).await.unwrap_err();
    assert_eq!(fault.code, perfine_api::Fault::denied().code);
    assert_eq!(h.provider.requests(), 4);
    h.verify_cached_promptly().await;
}

#[tokio::test]
async fn concurrent_rotated_key_waiters_share_one_refresh() {
    let h = Harness::new(Duration::from_secs(5)).await;
    h.provider.set_keys(vec![
        rsa_jwk(KEY, "cached"),
        rsa_jwk(ROTATED_KEY, "rotated"),
    ]);
    let gate = Gate::new();
    h.provider.set_mode(Mode::Gate(gate.clone()));
    let rotated = token(ROTATED_KEY, "rotated");
    let first = h.spawn_verify(rotated.clone());
    gate.wait_arrival().await;
    let waiters: Vec<_> = (0..4).map(|_| h.spawn_verify(rotated.clone())).collect();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.verify_cached_promptly().await;
    assert_eq!(h.provider.requests(), 1);
    gate.release();
    assert!(first.await.unwrap().is_ok());
    for waiter in waiters {
        assert!(
            timeout(Duration::from_secs(2), waiter)
                .await
                .expect("rotated-key waiter stalled")
                .unwrap()
                .is_ok()
        );
    }
    assert_eq!(h.provider.requests(), 1);
    assert_eq!(h.provider.max_in_flight.load(Ordering::SeqCst), 1);
}
