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
use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Upper bound for a presented bearer token. Real Logto tokens are far smaller.
const MAX_TOKEN_BYTES: usize = 8 << 10;
/// Minimum interval between forced key-set refreshes. A flood of unverifiable tokens must not turn
/// into a flood of requests to the identity provider.
const MIN_REFRESH: Duration = Duration::from_secs(10);

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

pub struct Verifier {
    http: reqwest::Client,
    jwks_url: String,
    keys: RwLock<(JwkSet, Instant)>,
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
        let discovery: serde_json::Value = http
            .get(format!(
                "{}/.well-known/openid-configuration",
                issuer.trim_end_matches('/')
            ))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        anyhow::ensure!(
            discovery["issuer"].as_str() == Some(issuer),
            "OIDC issuer mismatch"
        );
        let jwks_url = discovery["jwks_uri"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing OIDC key endpoint"))?
            .to_owned();
        let keys = http
            .get(&jwks_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(Self {
            http,
            jwks_url,
            keys: RwLock::new((keys, Instant::now())),
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
        {
            let cached = self.keys.read().await;
            if let Ok(claims) = Self::decode(token, header.kid.as_deref(), &cached.0, &validation) {
                return self.identity(claims);
            }
        }
        // Refresh after a verification failure to support provider key rotation, including
        // rotation that keeps the key ID. Serialize refreshes and throttle them.
        let mut keys = self.keys.write().await;
        if let Ok(claims) = Self::decode(token, header.kid.as_deref(), &keys.0, &validation) {
            return self.identity(claims);
        }
        if keys.1.elapsed() < MIN_REFRESH {
            return Err(Fault::unauthenticated());
        }
        let fetched = async {
            self.http
                .get(&self.jwks_url)
                .send()
                .await?
                .error_for_status()?
                .json::<JwkSet>()
                .await
        }
        .await;
        keys.1 = Instant::now();
        match fetched {
            Ok(set) => keys.0 = set,
            Err(_) => {
                tracing::warn!(category = "jwks_refresh", "identity key refresh failed");
                return Err(Fault::unauthenticated());
            }
        }
        self.identity(Self::decode(
            token,
            header.kid.as_deref(),
            &keys.0,
            &validation,
        )?)
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
            issuer: "https://issuer.example".into(),
            audience: "api".into(),
            operator_role: "operator".into(),
        }
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
