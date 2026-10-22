// SPDX-License-Identifier: FSL-1.1-ALv2
//! Bearer credential parsing and Logto JWT verification.
//!
//! Two credential types reach the API: Logto user access tokens (JWTs) and DeliTools API keys
//! (`dtk_...`). Parsing only classifies the credential; verification of keys happens against
//! PostgreSQL in [`crate::platform`] and verification of JWTs happens here.
use crate::{
    error::{Fault, Result},
    keys,
};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation,
    jwk::{Jwk, JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse},
};
use serde::Deserialize;
use server_common::{authority::bounded_json, config::safe_url};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

/// Upper bound for a presented bearer token. Real Logto tokens are far smaller.
const MAX_TOKEN_BYTES: usize = 8 << 10;
/// Minimum interval between forced key-set refreshes. A flood of unverifiable tokens must not turn
/// into a flood of requests to the identity provider.
const MIN_REFRESH: Duration = Duration::from_secs(10);
/// Longest time a fetched key set is trusted. Once it is older, no token is accepted from it: the
/// next verification must fetch a fresh set first. Without this bound a signing key that the
/// identity provider has retired or revoked would keep verifying tokens for as long as no token
/// failed verification, because refreshes otherwise happen only after a failure. The bound is also
/// the longest delay before a removed key stops working. When the provider cannot be reached after
/// the bound, JWT authentication fails closed; API keys do not depend on it.
const MAX_KEY_AGE: Duration = Duration::from_secs(300);
/// Largest OIDC discovery document read from the identity provider. Real documents are a few KiB.
const MAX_DISCOVERY_BYTES: usize = 64 << 10;
/// Largest key set read from the identity provider. A handful of keys is far below this. Both
/// limits keep a faulty or compromised provider from forcing large allocations, which any
/// unauthenticated caller could otherwise repeat through the throttled refresh path.
const MAX_KEY_SET_BYTES: usize = 256 << 10;

/// GET a JSON document from the identity provider, reading at most `limit` bytes of body.
async fn fetch_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
    limit: usize,
) -> anyhow::Result<T> {
    let response = http.get(url).send().await?.error_for_status()?;
    let value = bounded_json(response, limit).await?;
    Ok(serde_json::from_value(value)?)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Credential<'a> {
    /// A well-formed DeliTools key: its UUID and the verification hash of the presented secret.
    ApiKey(uuid::Uuid, [u8; 32]),
    /// Anything else with a bearer scheme is treated as a Logto JWT and verified cryptographically.
    Jwt(&'a str),
}

/// Split an `Authorization` header value. The scheme is case-insensitive and there must be exactly
/// one token. A `dtk_`-prefixed token that is malformed fails here instead of reaching JWT parsing.
pub fn parse_credential(authorization: &str) -> Result<Credential<'_>> {
    let mut parts = authorization.split_whitespace();
    let (Some(scheme), Some(token), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(Fault::unauthenticated());
    };
    if !scheme.eq_ignore_ascii_case("Bearer") || token.len() > MAX_TOKEN_BYTES {
        return Err(Fault::unauthenticated());
    }
    if token.starts_with(keys::KEY_PREFIX) {
        let (id, hash) = keys::parse(token).ok_or_else(Fault::unauthenticated)?;
        return Ok(Credential::ApiKey(id, hash));
    }
    Ok(Credential::Jwt(token))
}

/// Whether the key set entry may verify a token signed with one of `algorithms`. The decoding key
/// is built from the key material alone, so the entry's own restrictions have to be applied
/// here: a key published for encryption, or without the verify operation, or declared for another
/// algorithm, must not verify a signature even when its `kid` matches. Absent restrictions allow
/// the use, as RFC 7517 specifies.
fn may_verify(key: &Jwk, algorithms: &[Algorithm]) -> bool {
    let common = &key.common;
    common
        .public_key_use
        .as_ref()
        .is_none_or(|usage| *usage == PublicKeyUse::Signature)
        && common
            .key_operations
            .as_ref()
            .is_none_or(|operations| operations.contains(&KeyOperations::Verify))
        && common.key_algorithm.is_none_or(|declared| {
            algorithms.iter().any(|algorithm| match algorithm {
                Algorithm::RS256 => declared == KeyAlgorithm::RS256,
                Algorithm::ES256 => declared == KeyAlgorithm::ES256,
                _ => false,
            })
        })
}

/// Claims DeliTools trusts after signature, issuer, audience and expiry validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// SHA-256 of issuer and subject, hex encoded. A stable pseudonymous actor reference for
    /// audit records; the raw subject is never stored or logged.
    pub subject_hash: String,
    /// The token carries the dedicated platform-operator role.
    pub operator: bool,
    /// A machine-to-machine token (subject equals client). It never acts as a user.
    pub machine: bool,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    #[serde(default)]
    nbf: Option<i64>,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    roles: Vec<String>,
}

/// Validate a discovery document and return its key endpoint. The endpoint is held to the same
/// rule as every configured URL: HTTPS, or HTTP only for a loopback fixture, without credentials.
/// A plaintext key set could be replaced on the path and would let an attacker sign tokens that
/// satisfy the configured issuer and audience.
fn jwks_endpoint(discovery: &serde_json::Value, issuer: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        discovery["issuer"].as_str() == Some(issuer),
        "OIDC issuer mismatch"
    );
    let jwks_url = discovery["jwks_uri"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing OIDC key endpoint"))?;
    anyhow::ensure!(
        safe_url(jwks_url),
        "OIDC key endpoint must be an https URL (loopback http is allowed for fixtures)"
    );
    Ok(jwks_url.to_owned())
}

pub struct Verifier {
    http: reqwest::Client,
    jwks_url: String,
    /// The cached key set and when it was fetched. Readers never wait for a network refresh: the
    /// write lock is taken only to install a completed result. A set older than [`MAX_KEY_AGE`] is
    /// not used.
    keys: RwLock<(JwkSet, Instant)>,
    /// Serializes refreshes and records when a fetch was last attempted, successful or not. It is
    /// held during the identity-provider request, so only callers that could not verify against
    /// the cached keys wait for it.
    refresh: Mutex<Instant>,
    issuer: String,
    audience: String,
    operator_role: String,
}

impl Verifier {
    /// Discover the issuer's key set. Startup fails when discovery or the key fetch fails.
    pub async fn new(
        issuer: &str,
        audience: &str,
        operator_role: &str,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let discovery: serde_json::Value = fetch_json(
            &http,
            &format!(
                "{}/.well-known/openid-configuration",
                issuer.trim_end_matches('/')
            ),
            MAX_DISCOVERY_BYTES,
        )
        .await?;
        let jwks_url = jwks_endpoint(&discovery, issuer)?;
        let keys = fetch_json(&http, &jwks_url, MAX_KEY_SET_BYTES).await?;
        Ok(Self {
            http,
            jwks_url,
            keys: RwLock::new((keys, Instant::now())),
            refresh: Mutex::new(Instant::now()),
            issuer: issuer.to_owned(),
            audience: audience.to_owned(),
            operator_role: operator_role.to_owned(),
        })
    }

    pub async fn verify(&self, token: &str) -> Result<Identity> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| Fault::unauthenticated())?;
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256) {
            return Err(Fault::unauthenticated());
        }
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        validation.leeway = 0;
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        if let Some(claims) = self
            .decode_cached(token, header.kid.as_deref(), &validation)
            .await
        {
            return self.identity(claims);
        }
        // Refresh after a verification failure to support provider key rotation, including
        // rotation that keeps the key ID. Serialize refreshes and throttle them. The cached key
        // set stays readable while the identity provider answers, so tokens that verify against it
        // never wait on a slow or unavailable provider.
        let mut last_fetch = self.refresh.lock().await;
        // A refresh that finished while this request waited may already hold the signing key.
        if let Some(claims) = self
            .decode_cached(token, header.kid.as_deref(), &validation)
            .await
        {
            return self.identity(claims);
        }
        if last_fetch.elapsed() < MIN_REFRESH {
            return Err(Fault::unauthenticated());
        }
        let fetched = fetch_json::<JwkSet>(&self.http, &self.jwks_url, MAX_KEY_SET_BYTES).await;
        *last_fetch = Instant::now();
        match fetched {
            Ok(set) => *self.keys.write().await = (set, Instant::now()),
            Err(_) => {
                tracing::warn!(category = "jwks_refresh", "identity key refresh failed");
                return Err(Fault::unauthenticated());
            }
        }
        drop(last_fetch);
        self.identity(
            self.decode_cached(token, header.kid.as_deref(), &validation)
                .await
                .ok_or_else(Fault::unauthenticated)?,
        )
    }

    async fn decode_cached(
        &self,
        raw: &str,
        kid: Option<&str>,
        validation: &Validation,
    ) -> Option<Claims> {
        let cached = self.keys.read().await;
        if cached.1.elapsed() > MAX_KEY_AGE {
            return None;
        }
        Self::decode(raw, kid, &cached.0, validation).ok()
    }

    fn decode(
        raw: &str,
        kid: Option<&str>,
        keys: &JwkSet,
        validation: &Validation,
    ) -> Result<Claims> {
        for key in &keys.keys {
            if kid.is_some_and(|kid| key.common.key_id.as_deref() != Some(kid)) {
                continue;
            }
            if !may_verify(key, &validation.algorithms) {
                continue;
            }
            if let Ok(key) = DecodingKey::from_jwk(key)
                && let Ok(token) = jsonwebtoken::decode::<Claims>(raw, &key, validation)
            {
                // Like go-oidc, tolerate five minutes of clock skew for nbf only; exp stays strict.
                if token
                    .claims
                    .nbf
                    .is_some_and(|nbf| nbf > chrono::Utc::now().timestamp().saturating_add(300))
                {
                    return Err(Fault::unauthenticated());
                }
                return Ok(token.claims);
            }
        }
        Err(Fault::unauthenticated())
    }

    fn identity(&self, claims: Claims) -> Result<Identity> {
        if claims.sub.is_empty() {
            return Err(Fault::unauthenticated());
        }
        let machine = !claims.client_id.is_empty() && claims.sub == claims.client_id;
        let mut hasher = Sha256::new();
        hasher.update(claims.iss.as_bytes());
        hasher.update([0]);
        hasher.update(claims.sub.as_bytes());
        Ok(Identity {
            subject_hash: hex::encode(hasher.finalize()),
            // A machine token never acquires platform-operator authority, even with the role claim.
            operator: !machine && claims.roles.contains(&self.operator_role),
            machine,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use jsonwebtoken::{EncodingKey, Header};
    use rsa::{RsaPrivateKey, pkcs8::EncodePrivateKey, traits::PublicKeyParts};
    use serde_json::json;

    fn verifier(keys: JwkSet) -> Verifier {
        Verifier {
            http: reqwest::Client::new(),
            jwks_url: "http://127.0.0.1:9/jwks".into(),
            keys: RwLock::new((keys, Instant::now())),
            refresh: Mutex::new(Instant::now()),
            issuer: "https://issuer.example".into(),
            audience: "api".into(),
            operator_role: "operator".into(),
        }
    }

    #[test]
    fn discovered_key_endpoints_must_be_secure() {
        let issuer = "https://issuer.example/oidc";
        let discovery = |uri: &str| json!({"issuer": issuer, "jwks_uri": uri});
        for secure in [
            "https://issuer.example/oidc/jwks",
            "https://keys.example/jwks",
            "http://127.0.0.1:9/jwks",
            "http://localhost/jwks",
        ] {
            assert_eq!(
                jwks_endpoint(&discovery(secure), issuer).unwrap(),
                secure,
                "{secure}"
            );
        }
        for insecure in [
            "http://keys.example/jwks",
            "http://issuer.example/oidc/jwks",
            "ftp://keys.example/jwks",
            "https://user:pass@keys.example/jwks",
            "not a url",
            "",
        ] {
            assert!(
                jwks_endpoint(&discovery(insecure), issuer).is_err(),
                "{insecure}"
            );
        }
        assert!(jwks_endpoint(&json!({"issuer": issuer}), issuer).is_err());
        assert!(jwks_endpoint(&json!({"issuer": "https://other.example", "jwks_uri": "https://keys.example/jwks"}), issuer).is_err());
    }

    /// An unverifiable token triggers a refresh against a provider that never answers in time.
    /// Tokens that verify against the cached keys must not wait for it.
    #[tokio::test]
    async fn a_slow_key_refresh_does_not_block_verification_against_cached_keys() {
        use rsa::pkcs8::DecodePrivateKey;
        let private =
            RsaPrivateKey::from_pkcs8_pem(include_str!("../tests/support/oidc-test-key.pem"))
                .unwrap();
        let signing =
            EncodingKey::from_rsa_pem(private.to_pkcs8_pem(Default::default()).unwrap().as_bytes())
                .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let provider = tokio::spawn(async move {
            let app = axum::Router::new().route(
                "/jwks",
                axum::routing::get(|| async {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    axum::Json(json!({"keys": []}))
                }),
            );
            axum::serve(listener, app).await.unwrap()
        });
        let mut v = verifier(
            serde_json::from_value(json!({"keys":[{
                "kty":"RSA","kid":"cached","alg":"RS256","use":"sig",
                "n":URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),
                "e":URL_SAFE_NO_PAD.encode(private.e().to_bytes_be())
            }]}))
            .unwrap(),
        );
        v.jwks_url = format!("http://{address}/jwks");
        // Make the next unverifiable token eligible for a refresh.
        *v.refresh.lock().await = Instant::now()
            .checked_sub(MIN_REFRESH * 2)
            .expect("monotonic clock has run long enough");
        let v = std::sync::Arc::new(v);
        let now = chrono::Utc::now().timestamp();
        let claims = json!({"iss":v.issuer,"aud":v.audience,"sub":"user","exp":now+3600});
        let sign = |kid: &str| {
            let mut header = Header::new(Algorithm::RS256);
            header.kid = Some(kid.into());
            jsonwebtoken::encode(&header, &claims, &signing).unwrap()
        };

        let rotating = tokio::spawn({
            let (v, token) = (v.clone(), sign("rotated"));
            async move { v.verify(&token).await }
        });
        // Let the refresh start and block on the provider.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let verified = tokio::time::timeout(Duration::from_secs(2), v.verify(&sign("cached")))
            .await
            .expect("verification against cached keys waited for the refresh");
        assert!(verified.is_ok());
        assert!(
            !rotating.is_finished(),
            "the refresh should still be waiting"
        );
        rotating.abort();
        provider.abort();
    }

    /// A key set entry restricted to another use, operation or algorithm must not verify a token
    /// even though its key ID matches and its key material is the signer's.
    #[tokio::test]
    async fn key_restrictions_in_the_key_set_are_honored() {
        use rsa::pkcs8::DecodePrivateKey;
        let private =
            RsaPrivateKey::from_pkcs8_pem(include_str!("../tests/support/oidc-test-key.pem"))
                .unwrap();
        let signing =
            EncodingKey::from_rsa_pem(private.to_pkcs8_pem(Default::default()).unwrap().as_bytes())
                .unwrap();
        let with = |restriction: serde_json::Value| {
            let mut key = json!({
                "kty":"RSA","kid":"k",
                "n":URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),
                "e":URL_SAFE_NO_PAD.encode(private.e().to_bytes_be())
            });
            key.as_object_mut()
                .unwrap()
                .extend(restriction.as_object().unwrap().clone());
            verifier(serde_json::from_value(json!({"keys":[key]})).unwrap())
        };
        let now = chrono::Utc::now().timestamp();
        let claims =
            json!({"iss":"https://issuer.example","aud":"api","sub":"user","exp":now+3600});
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("k".into());
        let token = jsonwebtoken::encode(&header, &claims, &signing).unwrap();
        for allowed in [
            json!({}),
            json!({"use":"sig"}),
            json!({"key_ops":["verify"]}),
            json!({"key_ops":["sign","verify"],"alg":"RS256"}),
        ] {
            assert!(
                with(allowed.clone()).verify(&token).await.is_ok(),
                "{allowed}"
            );
        }
        for rejected in [
            json!({"use":"enc"}),
            json!({"key_ops":["encrypt"]}),
            json!({"key_ops":["sign"]}),
            json!({"alg":"RS384"}),
            json!({"alg":"ES256"}),
            json!({"alg":"RSA-OAEP"}),
        ] {
            assert!(
                with(rejected.clone()).verify(&token).await.is_err(),
                "{rejected}"
            );
        }
    }

    /// A provider response above the limit is rejected without being deserialized, both at
    /// startup and on a refresh, and a refresh that fails this way leaves the cached keys alone.
    #[tokio::test]
    async fn oversized_provider_documents_are_rejected() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let provider = tokio::spawn(async move {
            let app = axum::Router::new()
                .route(
                    "/small",
                    axum::routing::get(|| async { axum::Json(json!({"keys": []})) }),
                )
                .route(
                    "/large",
                    axum::routing::get(|| async {
                        let padding = "x".repeat(MAX_KEY_SET_BYTES);
                        axum::Json(json!({"keys": [], "padding": padding}))
                    }),
                );
            axum::serve(listener, app).await.unwrap()
        });
        let http = reqwest::Client::new();
        let url = |path: &str| format!("http://{address}/{path}");
        assert!(
            fetch_json::<JwkSet>(&http, &url("small"), MAX_KEY_SET_BYTES)
                .await
                .is_ok()
        );
        assert!(
            fetch_json::<JwkSet>(&http, &url("large"), MAX_KEY_SET_BYTES)
                .await
                .is_err()
        );

        let mut v = verifier(JwkSet { keys: Vec::new() });
        v.jwks_url = url("large");
        *v.refresh.lock().await = Instant::now()
            .checked_sub(MIN_REFRESH * 2)
            .expect("monotonic clock has run long enough");
        let now = chrono::Utc::now().timestamp();
        let claims = json!({"iss":v.issuer,"aud":v.audience,"sub":"user","exp":now+3600});
        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::RS256),
            &claims,
            &EncodingKey::from_rsa_pem(include_bytes!("../tests/support/oidc-test-key.pem"))
                .unwrap(),
        )
        .unwrap();
        assert!(v.verify(&token).await.is_err());
        provider.abort();
    }

    /// Make the cached key set older than `MAX_KEY_AGE` and the next refresh attempt eligible.
    async fn age_cache(v: &Verifier) {
        v.keys.write().await.1 = Instant::now()
            .checked_sub(MAX_KEY_AGE * 2)
            .expect("monotonic clock has run long enough");
        *v.refresh.lock().await = Instant::now()
            .checked_sub(MIN_REFRESH * 2)
            .expect("monotonic clock has run long enough");
    }

    /// A signing key that the provider has retired must stop verifying once the cached key set
    /// is older than `MAX_KEY_AGE`, even though no token ever failed verification and so nothing
    /// forced a refresh.
    #[tokio::test]
    async fn an_expired_key_set_is_refreshed_before_it_verifies_anything() {
        use rsa::pkcs8::DecodePrivateKey;
        use std::sync::{
            Arc, Mutex as StdMutex,
            atomic::{AtomicUsize, Ordering},
        };
        let private =
            RsaPrivateKey::from_pkcs8_pem(include_str!("../tests/support/oidc-test-key.pem"))
                .unwrap();
        let signing =
            EncodingKey::from_rsa_pem(private.to_pkcs8_pem(Default::default()).unwrap().as_bytes())
                .unwrap();
        let with_key: serde_json::Value = json!({"keys":[{
            "kty":"RSA","kid":"fixture","alg":"RS256","use":"sig",
            "n":URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),
            "e":URL_SAFE_NO_PAD.encode(private.e().to_bytes_be())
        }]});
        let served = Arc::new(StdMutex::new(with_key.clone()));
        let fetches = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let provider = tokio::spawn({
            let (served, fetches) = (served.clone(), fetches.clone());
            async move {
                let app = axum::Router::new().route(
                    "/jwks",
                    axum::routing::get(move || {
                        let (served, fetches) = (served.clone(), fetches.clone());
                        async move {
                            fetches.fetch_add(1, Ordering::SeqCst);
                            axum::Json(served.lock().unwrap().clone())
                        }
                    }),
                );
                axum::serve(listener, app).await.unwrap()
            }
        });
        let mut v = verifier(serde_json::from_value(with_key.clone()).unwrap());
        v.jwks_url = format!("http://{address}/jwks");
        let now = chrono::Utc::now().timestamp();
        let claims = json!({"iss":v.issuer,"aud":v.audience,"sub":"user","exp":now+3600});
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("fixture".into());
        let token = jsonwebtoken::encode(&header, &claims, &signing).unwrap();
        // A fresh cache verifies without any request to the provider.
        assert!(v.verify(&token).await.is_ok());
        assert_eq!(fetches.load(Ordering::SeqCst), 0);

        // The provider retires the key. The old cache must not keep accepting its tokens.
        *served.lock().unwrap() = json!({"keys": []});
        age_cache(&v).await;
        assert!(v.verify(&token).await.is_err(), "a retired key verified");
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        // The failed attempt is throttled like any other refresh.
        assert!(v.verify(&token).await.is_err());
        assert_eq!(fetches.load(Ordering::SeqCst), 1);

        // A still-valid key is accepted again after one refresh, which also renews the cache.
        *served.lock().unwrap() = with_key;
        *v.refresh.lock().await = Instant::now()
            .checked_sub(MIN_REFRESH * 2)
            .expect("monotonic clock has run long enough");
        assert!(v.verify(&token).await.is_ok());
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        assert!(v.verify(&token).await.is_ok());
        assert_eq!(fetches.load(Ordering::SeqCst), 2);

        // An unreachable provider fails closed once the cache is too old.
        provider.abort();
        let _ = provider.await;
        age_cache(&v).await;
        assert!(v.verify(&token).await.is_err());
    }

    #[test]
    fn credential_parsing_classifies_and_rejects() {
        let key = keys::generate();
        let header = format!("bearer {}", key.token);
        match parse_credential(&header).unwrap() {
            Credential::ApiKey(id, hash) => {
                assert_eq!(id, key.id);
                assert_eq!(hash, key.hash);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse_credential("Bearer aaa.bbb.ccc").unwrap(),
            Credential::Jwt("aaa.bbb.ccc")
        );
        for bad in [
            "",
            "Bearer",
            "Basic abc",
            "Bearer a b",
            "dtk_token",
            "Bearer dtk_short",
        ] {
            assert!(parse_credential(bad).is_err(), "{bad}");
        }
        let oversized = format!("Bearer {}", "a".repeat(MAX_TOKEN_BYTES + 1));
        assert!(parse_credential(&oversized).is_err());
    }

    #[test]
    fn machine_tokens_never_become_operators_or_users() {
        let v = verifier(JwkSet { keys: vec![] });
        let machine = v
            .identity(Claims {
                iss: "i".into(),
                sub: "svc".into(),
                nbf: None,
                client_id: "svc".into(),
                roles: vec!["operator".into()],
            })
            .unwrap();
        assert!(machine.machine && !machine.operator);
        let user = v
            .identity(Claims {
                iss: "i".into(),
                sub: "user".into(),
                nbf: None,
                client_id: "app".into(),
                roles: vec!["operator".into()],
            })
            .unwrap();
        assert!(!user.machine && user.operator);
        assert_eq!(user.subject_hash.len(), 64);
        let plain = v
            .identity(Claims {
                iss: "i".into(),
                sub: "user".into(),
                nbf: None,
                client_id: String::new(),
                roles: vec![],
            })
            .unwrap();
        assert!(!plain.operator);
        assert_eq!(plain.subject_hash, user.subject_hash);
        assert!(
            v.identity(Claims {
                iss: "i".into(),
                sub: String::new(),
                nbf: None,
                client_id: String::new(),
                roles: vec![],
            })
            .is_err()
        );
    }

    #[tokio::test]
    async fn signed_tokens_enforce_issuer_audience_and_time() {
        let private = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
        let pem = private.to_pkcs8_pem(Default::default()).unwrap();
        let signing = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
        let v = verifier(
            serde_json::from_value(json!({"keys":[{
                "kty":"RSA","kid":"test","alg":"RS256","use":"sig",
                "n":URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),
                "e":URL_SAFE_NO_PAD.encode(private.e().to_bytes_be())
            }]}))
            .unwrap(),
        );
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test".into());
        let now = chrono::Utc::now().timestamp();
        let valid = json!({"iss":v.issuer,"aud":v.audience,"sub":"user","exp":now+3600,"roles":["operator"]});
        let sign =
            |claims: &serde_json::Value| jsonwebtoken::encode(&header, claims, &signing).unwrap();
        let identity = v.verify(&sign(&valid)).await.unwrap();
        assert!(identity.operator);
        let mut early = valid.clone();
        early["nbf"] = json!(now + 3600);
        assert!(v.verify(&sign(&early)).await.is_err());
        early["nbf"] = json!(now + 240);
        assert!(v.verify(&sign(&early)).await.is_ok());
        let mut expired = valid.clone();
        expired["exp"] = json!(now - 1);
        assert!(v.verify(&sign(&expired)).await.is_err());
        let mut wrong_issuer = valid.clone();
        wrong_issuer["iss"] = json!("https://other.example");
        assert!(v.verify(&sign(&wrong_issuer)).await.is_err());
        let mut no_audience = valid.clone();
        no_audience.as_object_mut().unwrap().remove("aud");
        assert!(v.verify(&sign(&no_audience)).await.is_err());
        for audience in [json!(null), json!(42), json!({"api":true}), json!("other")] {
            let mut malformed = valid.clone();
            malformed["aud"] = audience;
            assert!(v.verify(&sign(&malformed)).await.is_err());
        }
        let other = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
        let other_pem = other.to_pkcs8_pem(Default::default()).unwrap();
        let forged = jsonwebtoken::encode(
            &header,
            &valid,
            &EncodingKey::from_rsa_pem(other_pem.as_bytes()).unwrap(),
        )
        .unwrap();
        assert!(v.verify(&forged).await.is_err());
        assert!(v.verify("not-a-jwt").await.is_err());
    }
}
