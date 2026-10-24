// SPDX-License-Identifier: FSL-1.1-ALv2
//! Loopback tests for JWKS refresh coordination in `Verifier`.
//!
//! The OIDC provider is a tiny raw-TCP HTTP server so each test can hold a JWKS
//! response at a precise point (before the response headers or in the middle of the
//! body). No database, credentials or external network are used. The key files in
//! `tests/jwks_refresh/` and `tests/support/oidc-test-key.pem` are public test fixtures
//! and never authenticate production callers.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use delcore_api::{auth::Verifier, config::Config};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use rsa::{RsaPrivateKey, pkcs8::DecodePrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    time::{sleep, timeout},
};

const RSA_PEM: &str = include_str!("support/oidc-test-key.pem");
const ROTATED_RSA_PEM: &str = include_str!("jwks_refresh/rotated-rsa-test-key.pem");
const ES256_PEM: &str = include_str!("jwks_refresh/es256-test-key.pem");
// Public coordinates of the committed ES256 fixture key.
const ES256_X: &str = "Bu8tIZpa9hzH_hTuL-jmKJ1Fv7oTpalsXN1-mV2ac_k";
const ES256_Y: &str = "quCyyjK5Zg8yc45VIGna9cWuxGruQWVfHk88sn_xHJA";
// Header {"alg":"RS256","kid":"absent-fixture"}, payload {}, invalid signature. Only the
// header is decoded before the key lookup, so this needs no signing key.
const HOSTILE: &str = "Bearer eyJhbGciOiJSUzI1NiIsImtpZCI6ImFic2VudC1maXh0dXJlIn0.e30.AA";

/// Two semaphores used as signals: `arrived` is added to by the mock when a request
/// reaches the gate, and the test adds to `release` to let it continue.
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
            .expect("refresh never reached the provider")
            .unwrap()
            .forget();
    }
    fn release(&self, permits: usize) {
        self.release.add_permits(permits);
    }
}

#[derive(Clone)]
enum Mode {
    Serve,
    GateHeaders(Arc<Gate>),
    GateBody(Arc<Gate>),
    Status(u16),
    BadJson,
    Drop,
}

struct Mock {
    issuer: String,
    jwks: StdMutex<Value>,
    mode: StdMutex<Mode>,
    fetches: AtomicUsize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}
struct InFlight<'a>(&'a AtomicUsize);
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl Mock {
    fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }
    fn set_mode(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }
    fn set_jwks(&self, keys: Vec<Value>) {
        *self.jwks.lock().unwrap() = json!({ "keys": keys });
    }
    async fn serve(self: Arc<Self>, listener: TcpListener) {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(self.clone().handle(stream));
        }
    }
    async fn handle(self: Arc<Self>, mut stream: TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(n) => request.extend_from_slice(&buffer[..n]),
            }
        }
        let head = String::from_utf8_lossy(&request);
        let path = head.split_whitespace().nth(1).unwrap_or_default();
        if path == "/.well-known/openid-configuration" {
            let body = json!({"issuer":self.issuer,"jwks_uri":format!("{}/jwks",self.issuer)});
            respond(&mut stream, 200, &body.to_string()).await;
            return;
        }
        self.fetches.fetch_add(1, Ordering::SeqCst);
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        let _in_flight = InFlight(&self.in_flight);
        let mode = self.mode.lock().unwrap().clone();
        let body = self.jwks.lock().unwrap().to_string();
        match mode {
            Mode::Serve => respond(&mut stream, 200, &body).await,
            Mode::GateHeaders(gate) => {
                gate.arrived.add_permits(1);
                gate.release.acquire().await.unwrap().forget();
                respond(&mut stream, 200, &body).await;
            }
            Mode::GateBody(gate) => {
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let (first, rest) = body.split_at(body.len() / 2);
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(first.as_bytes()).await;
                let _ = stream.flush().await;
                gate.arrived.add_permits(1);
                gate.release.acquire().await.unwrap().forget();
                let _ = stream.write_all(rest.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
            Mode::Status(status) => respond(&mut stream, status, "{}").await,
            Mode::BadJson => respond(&mut stream, 200, "not json").await,
            Mode::Drop => drop(stream),
        }
    }
}
async fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn rsa_jwk(pem: &str, kid: &str) -> Value {
    let private = RsaPrivateKey::from_pkcs8_pem(pem).unwrap();
    json!({"kty":"RSA","kid":kid,"alg":"RS256","use":"sig",
        "n":URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),
        "e":URL_SAFE_NO_PAD.encode(private.e().to_bytes_be())})
}
fn es_jwk(kid: &str) -> Value {
    json!({"kty":"EC","crv":"P-256","kid":kid,"alg":"ES256","use":"sig","x":ES256_X,"y":ES256_Y})
}
fn initial_keys() -> Vec<Value> {
    vec![rsa_jwk(RSA_PEM, "rsa"), es_jwk("ec")]
}

struct Harness {
    mock: Arc<Mock>,
    verifier: Arc<Verifier>,
    config: Config,
    base_fetches: usize,
}
impl Harness {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let mock = Arc::new(Mock {
            issuer: issuer.clone(),
            jwks: StdMutex::new(json!({ "keys": initial_keys() })),
            mode: StdMutex::new(Mode::Serve),
            fetches: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
        });
        tokio::spawn(mock.clone().serve(listener));
        let mut values: HashMap<String, String> =
            dotenvy::from_read_iter(include_bytes!("../.env.example").as_slice())
                .collect::<Result<_, _>>()
                .unwrap();
        values.insert("ISSUER".into(), issuer);
        let config = Config::from_getter(|k| values.get(k).cloned().unwrap_or_default()).unwrap();
        let verifier = Arc::new(Verifier::new(&config).await.unwrap());
        let base_fetches = mock.fetches();
        Self {
            mock,
            verifier,
            config,
            base_fetches,
        }
    }
    fn claims(&self) -> Value {
        json!({"iss":self.mock.issuer,"aud":self.config.audience,"sub":"service",
            "client_id":"service","exp":chrono::Utc::now().timestamp()+3600})
    }
    fn bearer(&self, alg: Algorithm, kid: &str, claims: &Value) -> String {
        let signer = match alg {
            Algorithm::RS256 => Signer::Rsa,
            Algorithm::ES256 => Signer::Es256,
            _ => Signer::Hmac,
        };
        self.bearer_signed(alg, kid, signer, claims)
    }
    fn bearer_signed(&self, alg: Algorithm, kid: &str, signer: Signer, claims: &Value) -> String {
        let key = match signer {
            Signer::Rsa => EncodingKey::from_rsa_pem(RSA_PEM.as_bytes()).unwrap(),
            Signer::RotatedRsa => EncodingKey::from_rsa_pem(ROTATED_RSA_PEM.as_bytes()).unwrap(),
            Signer::Es256 => EncodingKey::from_ec_pem(ES256_PEM.as_bytes()).unwrap(),
            Signer::Hmac => EncodingKey::from_secret(b"secret"),
        };
        let mut header = Header::new(alg);
        header.kid = Some(kid.into());
        format!(
            "Bearer {}",
            jsonwebtoken::encode(&header, claims, &key).unwrap()
        )
    }
    fn rsa_token(&self) -> String {
        self.bearer(Algorithm::RS256, "rsa", &self.claims())
    }
    fn es_token(&self) -> String {
        self.bearer(Algorithm::ES256, "ec", &self.claims())
    }
    /// Verifies within a short deadline so lock delay fails fast instead of hanging.
    async fn verify_promptly(
        &self,
        authorization: &str,
    ) -> delcore_api::Result<delcore_api::auth::Identity> {
        timeout(Duration::from_secs(2), self.verifier.verify(authorization))
            .await
            .expect("verification waited behind an unrelated JWKS refresh")
    }
    fn spawn_verify(&self, authorization: &str) -> tokio::task::JoinHandle<bool> {
        let verifier = self.verifier.clone();
        let authorization = authorization.to_owned();
        tokio::spawn(async move { verifier.verify(&authorization).await.is_ok() })
    }
}

#[derive(Clone, Copy)]
enum Signer {
    Rsa,
    RotatedRsa,
    Es256,
    Hmac,
}

#[derive(Clone, Copy)]
enum GateKind {
    Headers,
    Body,
}
async fn cached_token_verifies_during_refresh(alg: Algorithm, kind: GateKind) {
    let h = Harness::new().await;
    let gate = Gate::new();
    h.mock.set_mode(match kind {
        GateKind::Headers => Mode::GateHeaders(gate.clone()),
        GateKind::Body => Mode::GateBody(gate.clone()),
    });
    let hostile = h.spawn_verify(HOSTILE);
    gate.wait_arrival().await;
    let valid = match alg {
        Algorithm::ES256 => h.es_token(),
        _ => h.rsa_token(),
    };
    // Both fixtures stay unexpired and cached; the gate is still closed.
    let identity = h.verify_promptly(&valid).await.unwrap();
    assert_eq!(identity.client_id, "service");
    // A known kid with a bad signature also starts a refresh but must not block either.
    assert!(!hostile.is_finished());
    gate.release(1);
    assert!(!hostile.await.unwrap());
    assert_eq!(h.mock.fetches(), h.base_fetches + 1);
    assert!(h.verify_promptly(&valid).await.is_ok());
}
#[tokio::test]
async fn rs256_cached_token_verifies_while_refresh_waits_for_headers() {
    cached_token_verifies_during_refresh(Algorithm::RS256, GateKind::Headers).await;
}
#[tokio::test]
async fn rs256_cached_token_verifies_while_refresh_waits_for_body() {
    cached_token_verifies_during_refresh(Algorithm::RS256, GateKind::Body).await;
}
#[tokio::test]
async fn es256_cached_token_verifies_while_refresh_waits_for_headers() {
    cached_token_verifies_during_refresh(Algorithm::ES256, GateKind::Headers).await;
}
#[tokio::test]
async fn es256_cached_token_verifies_while_refresh_waits_for_body() {
    cached_token_verifies_during_refresh(Algorithm::ES256, GateKind::Body).await;
}

#[tokio::test]
async fn known_kid_with_bad_signature_refreshes_without_blocking_cached_tokens() {
    let h = Harness::new().await;
    let gate = Gate::new();
    h.mock.set_mode(Mode::GateHeaders(gate.clone()));
    // Signed by a different key but claiming the cached "rsa" key ID.
    let forged = h.bearer_signed(Algorithm::RS256, "rsa", Signer::RotatedRsa, &h.claims());
    let hostile = h.spawn_verify(&forged);
    gate.wait_arrival().await;
    assert!(h.verify_promptly(&h.rsa_token()).await.is_ok());
    assert!(h.verify_promptly(&h.es_token()).await.is_ok());
    gate.release(1);
    assert!(!hostile.await.unwrap());
}

#[tokio::test]
async fn concurrent_unknown_key_requests_share_one_refresh() {
    let h = Harness::new().await;
    let gate = Gate::new();
    let mut keys = initial_keys();
    keys.push(rsa_jwk(ROTATED_RSA_PEM, "rotated"));
    h.mock.set_jwks(keys);
    h.mock.set_mode(Mode::GateHeaders(gate.clone()));
    let token = h.bearer_signed(Algorithm::RS256, "rotated", Signer::RotatedRsa, &h.claims());
    let waiters: Vec<_> = (0..4).map(|_| h.spawn_verify(&token)).collect();
    gate.wait_arrival().await;
    // Let the remaining requests queue behind the refresh lock.
    sleep(Duration::from_millis(200)).await;
    assert_eq!(h.mock.fetches(), h.base_fetches + 1);
    // Extra permits make a redundant fetch visible in the counters instead of hanging.
    gate.release(4);
    for waiter in waiters {
        assert!(waiter.await.unwrap());
    }
    assert_eq!(h.mock.fetches(), h.base_fetches + 1);
    assert_eq!(h.mock.max_in_flight.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn same_kid_rotation_recovers_through_refresh() {
    let h = Harness::new().await;
    // Same key ID, different key material.
    h.mock
        .set_jwks(vec![rsa_jwk(ROTATED_RSA_PEM, "rsa"), es_jwk("ec")]);
    let token = h.bearer_signed(Algorithm::RS256, "rsa", Signer::RotatedRsa, &h.claims());
    assert!(h.verify_promptly(&token).await.is_ok());
    assert_eq!(h.mock.fetches(), h.base_fetches + 1);
    // The published snapshot now serves the rotated key without another fetch.
    assert!(h.verify_promptly(&token).await.is_ok());
    assert_eq!(h.mock.fetches(), h.base_fetches + 1);
}

#[tokio::test]
async fn failed_refreshes_keep_cached_keys_and_release_coordination() {
    let h = Harness::new().await;
    for mode in [Mode::Status(500), Mode::BadJson, Mode::Drop] {
        h.mock.set_mode(mode);
        let hostile = timeout(Duration::from_secs(5), h.verifier.verify(HOSTILE))
            .await
            .expect("failed refresh did not release coordination");
        assert!(hostile.is_err());
        assert!(h.verify_promptly(&h.rsa_token()).await.is_ok());
        assert!(h.verify_promptly(&h.es_token()).await.is_ok());
    }
    // A later refresh is still possible and recovers an unknown key.
    let mut keys = initial_keys();
    keys.push(rsa_jwk(ROTATED_RSA_PEM, "rotated"));
    h.mock.set_jwks(keys);
    h.mock.set_mode(Mode::Serve);
    let rotated = h.bearer_signed(Algorithm::RS256, "rotated", Signer::RotatedRsa, &h.claims());
    assert!(h.verify_promptly(&rotated).await.is_ok());
}

#[tokio::test]
async fn cancelled_refresh_keeps_cached_keys_and_releases_coordination() {
    let h = Harness::new().await;
    let gate = Gate::new();
    h.mock.set_mode(Mode::GateBody(gate.clone()));
    let hostile = h.spawn_verify(HOSTILE);
    gate.wait_arrival().await;
    hostile.abort();
    assert!(hostile.await.unwrap_err().is_cancelled());
    assert!(h.verify_promptly(&h.rsa_token()).await.is_ok());
    let mut keys = initial_keys();
    keys.push(rsa_jwk(ROTATED_RSA_PEM, "rotated"));
    h.mock.set_jwks(keys);
    h.mock.set_mode(Mode::Serve);
    let rotated = h.bearer_signed(Algorithm::RS256, "rotated", Signer::RotatedRsa, &h.claims());
    assert!(h.verify_promptly(&rotated).await.is_ok());
}

#[tokio::test]
async fn rejection_and_identity_rules_are_unchanged() {
    let h = Harness::new().await;
    let now = chrono::Utc::now().timestamp();
    let mut expired = h.claims();
    expired["exp"] = json!(now - 1);
    let mut issuer = h.claims();
    issuer["iss"] = json!("https://other.example");
    let mut audience = h.claims();
    audience["aud"] = json!("other");
    let mut early = h.claims();
    early["nbf"] = json!(now + 3600);
    for claims in [&expired, &issuer, &audience, &early] {
        for (alg, kid) in [(Algorithm::RS256, "rsa"), (Algorithm::ES256, "ec")] {
            let token = h.bearer(alg, kid, claims);
            assert!(h.verify_promptly(&token).await.is_err());
        }
    }
    // A known kid with a signature from different key material is rejected.
    let wrong_signature = h.bearer(Algorithm::ES256, "rsa", &h.claims());
    assert!(h.verify_promptly(&wrong_signature).await.is_err());

    // Failures decided before key lookup or after successful decoding never fetch.
    let fetches = h.mock.fetches();
    let unsupported = h.bearer(Algorithm::HS256, "rsa", &h.claims());
    for malformed in [
        "",
        "Basic abc",
        "Bearer",
        "Bearer a b",
        "Bearer a.b.c",
        &unsupported,
    ] {
        assert!(h.verify_promptly(malformed).await.is_err());
    }
    let mut unverified = h.claims();
    unverified["sub"] = json!("user");
    unverified["client_id"] = json!("service");
    unverified["email"] = json!("user@example.test");
    unverified["email_verified"] = json!(false);
    assert!(
        h.verify_promptly(&h.bearer(Algorithm::RS256, "rsa", &unverified))
            .await
            .is_err()
    );
    assert_eq!(h.mock.fetches(), fetches);

    // A delegated user token never gains service authority.
    let mut delegated = unverified.clone();
    delegated["email_verified"] = json!(true);
    let identity = h
        .verify_promptly(&h.bearer(Algorithm::RS256, "rsa", &delegated))
        .await
        .unwrap();
    assert!(identity.client_id.is_empty());
    assert_eq!(identity.subject_hash.len(), 64);
    assert_eq!(h.mock.fetches(), fetches);
}
