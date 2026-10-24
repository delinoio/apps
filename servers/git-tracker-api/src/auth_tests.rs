// SPDX-License-Identifier: FSL-1.1-ALv2
//! Actions OIDC verification and JWKS refresh coordination regressions.
//!
//! A loopback TCP mock serves the JWKS with a request counter, an arrival signal and gates for
//! the response head and the response body. A controlled monotonic clock moves the 30-second
//! attempt interval and the 300-second snapshot freshness without sleeping.
use crate::auth::Actions;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use connectrpc::ErrorCode;
use jsonwebtoken::{Algorithm, EncodingKey, Header, jwk::JwkSet};
use rsa::{RsaPrivateKey, pkcs8::DecodePrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use server_common::Fault;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, Semaphore},
};

const KEY: &[u8] = include_bytes!("../tests/fixtures/github-test-key.pem");

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Keys,
    ServiceUnavailable,
    InvalidJson,
    InvalidJwks,
    Oversized,
}

#[derive(Clone, Copy, PartialEq)]
enum Hold {
    None,
    BeforeHead,
    MidBody,
}

struct Plan {
    mode: Mode,
    hold: Hold,
    kids: Vec<String>,
}

struct Mock {
    hits: AtomicUsize,
    arrived: Notify,
    gate: Semaphore,
    plan: Mutex<Plan>,
    n: String,
    e: String,
}

impl Mock {
    fn jwks(&self, kids: &[String]) -> Value {
        let keys: Vec<Value> = kids
            .iter()
            .map(|kid| json!({"kty":"RSA","kid":kid,"alg":"RS256","use":"sig","n":self.n,"e":self.e}))
            .collect();
        json!({ "keys": keys })
    }
    async fn handle(self: Arc<Self>, mut stream: TcpStream) {
        let mut seen = Vec::new();
        let mut buffer = [0u8; 1024];
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(n) => seen.extend_from_slice(&buffer[..n]),
            }
        }
        self.hits.fetch_add(1, Ordering::SeqCst);
        self.arrived.notify_one();
        let (mode, hold, kids) = {
            let plan = self.plan.lock().unwrap();
            (plan.mode, plan.hold, plan.kids.clone())
        };
        if hold == Hold::BeforeHead {
            self.gate.acquire().await.unwrap().forget();
        }
        let (status, body) = match mode {
            Mode::Keys => ("200 OK", self.jwks(&kids).to_string()),
            Mode::ServiceUnavailable => ("503 Service Unavailable", "{}".to_owned()),
            Mode::InvalidJson => ("200 OK", "not json".to_owned()),
            Mode::InvalidJwks => ("200 OK", json!({"keys":[{"kty":"RSA"}]}).to_string()),
            Mode::Oversized => ("200 OK", " ".repeat(300 << 10)),
        };
        let head = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let (first, rest) = body.split_at(body.len() / 2);
        let _ = stream.write_all(head.as_bytes()).await;
        if hold == Hold::MidBody {
            let _ = stream.write_all(first.as_bytes()).await;
            let _ = stream.flush().await;
            self.gate.acquire().await.unwrap().forget();
            let _ = stream.write_all(rest.as_bytes()).await;
        } else {
            let _ = stream.write_all(body.as_bytes()).await;
        }
        let _ = stream.shutdown().await;
    }
}

struct Harness {
    actions: Arc<Actions>,
    mock: Arc<Mock>,
    offset: Arc<AtomicU64>,
    _task: tokio::task::JoinHandle<()>,
}

impl Harness {
    async fn new() -> Self {
        Self::with_timeout(Duration::from_secs(5)).await
    }
    async fn with_timeout(timeout: Duration) -> Self {
        let private = RsaPrivateKey::from_pkcs8_pem(std::str::from_utf8(KEY).unwrap()).unwrap();
        let mock = Arc::new(Mock {
            hits: AtomicUsize::new(0),
            arrived: Notify::new(),
            gate: Semaphore::new(0),
            plan: Mutex::new(Plan {
                mode: Mode::Keys,
                hold: Hold::None,
                kids: vec!["fixture".to_owned()],
            }),
            n: URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),
            e: URL_SAFE_NO_PAD.encode(private.e().to_bytes_be()),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/jwks", listener.local_addr().unwrap());
        let served = mock.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(served.clone().handle(stream));
            }
        });
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let initial: JwkSet = serde_json::from_value(mock.jwks(&["fixture".to_owned()])).unwrap();
        let (base, offset) = (Instant::now(), Arc::new(AtomicU64::new(0)));
        let clock_offset = offset.clone();
        let clock =
            Arc::new(move || base + Duration::from_secs(clock_offset.load(Ordering::SeqCst)));
        Self {
            actions: Arc::new(Actions::with_clock(http, url, initial, clock)),
            mock,
            offset,
            _task: task,
        }
    }
    fn advance(&self, seconds: u64) {
        self.offset.fetch_add(seconds, Ordering::SeqCst);
    }
    fn hits(&self) -> usize {
        self.mock.hits.load(Ordering::SeqCst)
    }
    fn serve(&self, mode: Mode, hold: Hold, kids: &[&str]) {
        let mut plan = self.mock.plan.lock().unwrap();
        plan.mode = mode;
        plan.hold = hold;
        plan.kids = kids.iter().map(|k| (*k).to_owned()).collect();
    }
    fn release(&self) {
        self.mock.gate.add_permits(64);
    }
}

fn claims() -> Value {
    let now = chrono::Utc::now().timestamp();
    json!({"iss":"https://token.actions.githubusercontent.com","aud":"https://git-tracker.api.delino.io","exp":now+300,"nbf":now-10,"repository_id":"42"})
}

fn sign(header: Header, claims: &Value) -> String {
    jsonwebtoken::encode(&header, claims, &EncodingKey::from_rsa_pem(KEY).unwrap()).unwrap()
}

fn token(kid: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_owned());
    sign(header, &claims())
}

fn code(result: Result<Value, Fault>) -> ErrorCode {
    result.expect_err("verification must fail").code
}

#[tokio::test]
async fn gated_unknown_key_refresh_does_not_delay_fresh_known_key() {
    for hold in [Hold::BeforeHead, Hold::MidBody] {
        let h = Harness::new().await;
        h.serve(Mode::Keys, hold, &["fixture"]);
        let actions = h.actions.clone();
        let forged = tokio::spawn(async move { actions.verify(&token("absent-fixture")).await });
        h.mock.arrived.notified().await;
        // The refresh is in flight and gated; a fresh cached key must still verify at once.
        let known =
            tokio::time::timeout(Duration::from_secs(2), h.actions.verify(&token("fixture")))
                .await
                .expect("known-key verification must not wait for the refresh");
        assert_eq!(known.unwrap()["repository_id"], "42");
        assert_eq!(h.hits(), 1);
        h.release();
        assert_eq!(code(forged.await.unwrap()), ErrorCode::PermissionDenied);
        assert_eq!(h.hits(), 1);
    }
}

#[tokio::test]
async fn unknown_keys_start_one_fetch_per_interval_and_one_at_a_time() {
    let h = Harness::new().await;
    // Sequential identical and distinct attacker key identifiers.
    for kid in ["a", "a", "b", "c", "d"] {
        assert_eq!(
            code(h.actions.verify(&token(kid)).await),
            ErrorCode::PermissionDenied
        );
    }
    assert_eq!(h.hits(), 1);
    // Concurrent distinct identifiers inside the interval.
    h.advance(29);
    let calls = (0..32).map(|i| {
        let actions = h.actions.clone();
        tokio::spawn(async move { actions.verify(&token(&format!("kid-{i}"))).await })
    });
    for call in calls.collect::<Vec<_>>() {
        assert_eq!(code(call.await.unwrap()), ErrorCode::PermissionDenied);
    }
    assert_eq!(h.hits(), 1);
    // At eligibility concurrent misses share exactly one further fetch.
    h.advance(1);
    let calls = (0..32).map(|i| {
        let actions = h.actions.clone();
        tokio::spawn(async move { actions.verify(&token(&format!("other-{i}"))).await })
    });
    for call in calls.collect::<Vec<_>>() {
        assert_eq!(code(call.await.unwrap()), ErrorCode::PermissionDenied);
    }
    assert_eq!(h.hits(), 2);
}

#[tokio::test]
async fn waiters_reevaluate_the_shared_refresh_result() {
    let h = Harness::new().await;
    h.serve(Mode::Keys, Hold::BeforeHead, &["fixture", "rotated"]);
    let first = {
        let actions = h.actions.clone();
        tokio::spawn(async move { actions.verify(&token("rotated")).await })
    };
    h.mock.arrived.notified().await;
    let waiters: Vec<_> = (0..8)
        .map(|_| {
            let actions = h.actions.clone();
            tokio::spawn(async move { actions.verify(&token("rotated")).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.release();
    assert!(first.await.unwrap().is_ok());
    for waiter in waiters {
        assert!(waiter.await.unwrap().is_ok());
    }
    assert_eq!(h.hits(), 1);
}

#[tokio::test]
async fn failed_refreshes_are_rate_limited_for_every_failure_kind() {
    for mode in [
        Mode::ServiceUnavailable,
        Mode::InvalidJson,
        Mode::InvalidJwks,
        Mode::Oversized,
    ] {
        let h = Harness::new().await;
        h.serve(mode, Hold::None, &[]);
        // The attempt that fails reports a dependency failure; suppressed misses are denied.
        assert_eq!(
            code(h.actions.verify(&token("absent")).await),
            ErrorCode::Unavailable
        );
        for _ in 0..2 {
            assert_eq!(
                code(h.actions.verify(&token("absent")).await),
                ErrorCode::PermissionDenied
            );
        }
        assert_eq!(h.hits(), 1);
        // A fresh known key survives the unrelated failure.
        assert!(h.actions.verify(&token("fixture")).await.is_ok());
        h.advance(30);
        h.serve(Mode::Keys, Hold::None, &["fixture", "absent"]);
        assert!(h.actions.verify(&token("absent")).await.is_ok());
        assert_eq!(h.hits(), 2);
    }
}

#[tokio::test]
async fn transport_timeout_is_rate_limited() {
    let h = Harness::with_timeout(Duration::from_millis(300)).await;
    h.serve(Mode::Keys, Hold::BeforeHead, &["fixture"]);
    assert_eq!(
        code(h.actions.verify(&token("absent")).await),
        ErrorCode::Unavailable
    );
    assert_eq!(
        code(h.actions.verify(&token("absent")).await),
        ErrorCode::PermissionDenied
    );
    assert_eq!(h.hits(), 1);
}

#[tokio::test]
async fn cancelled_refresh_releases_coordination_without_a_retry_storm() {
    let h = Harness::new().await;
    h.serve(Mode::Keys, Hold::BeforeHead, &["fixture", "rotated"]);
    let actions = h.actions.clone();
    let initiator = tokio::spawn(async move { actions.verify(&token("rotated")).await });
    h.mock.arrived.notified().await;
    initiator.abort();
    let _ = initiator.await;
    // The in-flight state is released: this call returns promptly and starts no new fetch.
    let retry = tokio::time::timeout(Duration::from_secs(2), h.actions.verify(&token("rotated")))
        .await
        .expect("coordinator must not stay locked after cancellation");
    assert_eq!(code(retry), ErrorCode::PermissionDenied);
    assert_eq!(h.hits(), 1);
    // After the interval a healthy provider recovers without a restart.
    h.release();
    h.advance(30);
    h.serve(Mode::Keys, Hold::None, &["fixture", "rotated"]);
    assert!(h.actions.verify(&token("rotated")).await.is_ok());
    assert_eq!(h.hits(), 2);
}

#[tokio::test]
async fn rotated_key_requires_a_permitted_refresh() {
    let h = Harness::new().await;
    // The first miss is permitted and observes a provider that does not yet publish K3.
    assert_eq!(
        code(h.actions.verify(&token("k3")).await),
        ErrorCode::PermissionDenied
    );
    assert_eq!(h.hits(), 1);
    h.serve(Mode::Keys, Hold::None, &["fixture", "k2", "k3"]);
    // Inside the interval the now-published key is still denied without a fetch.
    h.advance(29);
    assert_eq!(
        code(h.actions.verify(&token("k3")).await),
        ErrorCode::PermissionDenied
    );
    assert_eq!(h.hits(), 1);
    // At eligibility one refresh discovers it, and later keys come from the cache.
    h.advance(1);
    assert!(h.actions.verify(&token("k3")).await.is_ok());
    assert!(h.actions.verify(&token("k2")).await.is_ok());
    assert!(h.actions.verify(&token("fixture")).await.is_ok());
    assert_eq!(h.hits(), 2);
}

#[tokio::test]
async fn expired_snapshot_never_authorizes_after_failed_or_suppressed_refresh() {
    let h = Harness::new().await;
    h.advance(301);
    h.serve(Mode::ServiceUnavailable, Hold::None, &[]);
    // The eligible refresh fails; the expired key is rejected and the age is not extended.
    assert_eq!(
        code(h.actions.verify(&token("fixture")).await),
        ErrorCode::Unavailable
    );
    assert_eq!(h.hits(), 1);
    // Cooldown suppresses another fetch and the expired key still fails closed.
    h.advance(5);
    assert_eq!(
        code(h.actions.verify(&token("fixture")).await),
        ErrorCode::Unavailable
    );
    assert_eq!(h.hits(), 1);
    // Recovery at eligibility restores validation.
    h.advance(25);
    h.serve(Mode::Keys, Hold::None, &["fixture"]);
    assert!(h.actions.verify(&token("fixture")).await.is_ok());
    assert_eq!(h.hits(), 2);
}

#[tokio::test]
async fn fresh_key_stays_usable_only_until_its_original_expiry_after_failed_refresh() {
    let h = Harness::new().await;
    h.serve(Mode::ServiceUnavailable, Hold::None, &[]);
    assert_eq!(
        code(h.actions.verify(&token("absent")).await),
        ErrorCode::Unavailable
    );
    h.advance(200);
    assert!(h.actions.verify(&token("fixture")).await.is_ok());
    assert_eq!(h.hits(), 1);
    // Past the original 300 seconds the failed attempt gave no extra freshness.
    h.advance(101);
    assert_eq!(
        code(h.actions.verify(&token("fixture")).await),
        ErrorCode::Unavailable
    );
    assert_eq!(h.hits(), 2);
}

#[tokio::test]
async fn token_validation_is_preserved() {
    let h = Harness::new().await;
    let signed = |tweak: &dyn Fn(&mut Value)| {
        let mut claims = claims();
        tweak(&mut claims);
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("fixture".to_owned());
        sign(header, &claims)
    };
    let now = chrono::Utc::now().timestamp();
    let bad = [
        signed(&|c| c["iss"] = "https://example.invalid".into()),
        signed(&|c| c["aud"] = "other".into()),
        signed(&|c| c["exp"] = (now - 3600).into()),
        signed(&|c| c["nbf"] = (now + 3600).into()),
    ];
    for token in bad {
        assert_eq!(
            code(h.actions.verify(&token).await),
            ErrorCode::PermissionDenied
        );
    }
    // Tampered signature, missing kid and non-RS256 headers are rejected before any refresh.
    let mut tampered = token("fixture");
    tampered.pop();
    tampered.push(if tampered.ends_with('A') { 'B' } else { 'A' });
    assert_eq!(
        code(h.actions.verify(&tampered).await),
        ErrorCode::PermissionDenied
    );
    let no_kid = sign(Header::new(Algorithm::RS256), &claims());
    assert_eq!(
        code(h.actions.verify(&no_kid).await),
        ErrorCode::PermissionDenied
    );
    let mut hs = Header::new(Algorithm::HS256);
    hs.kid = Some("absent".to_owned());
    let hs256 = jsonwebtoken::encode(&hs, &claims(), &EncodingKey::from_secret(b"secret")).unwrap();
    assert_eq!(
        code(h.actions.verify(&hs256).await),
        ErrorCode::PermissionDenied
    );
    assert_eq!(h.hits(), 0);
    assert!(h.actions.verify(&token("fixture")).await.is_ok());
}
