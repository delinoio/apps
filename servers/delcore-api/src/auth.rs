// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Fault, Result, config::Config};
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use serde::Deserialize;
use sha2::Sha256;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Clone, Debug, Default)]
pub struct Identity {
    pub subject_hash: String,
    pub email: String,
    pub client_id: String,
    pub operator: bool,
    pub scopes: Vec<String>,
}
#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    #[serde(default)]
    nbf: Option<i64>,
    #[serde(default)]
    email: String,
    #[serde(default)]
    email_verified: bool,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    roles: Vec<String>,
}
pub struct Verifier {
    http: reqwest::Client,
    jwks_url: String,
    keys: RwLock<(JwkSet, Instant)>,
    issuer: String,
    audience: String,
    identity_key: String,
    operator_role: String,
}
impl Verifier {
    pub async fn new(config: &Config) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds as u64))
            .build()?;
        let discovery: serde_json::Value = http
            .get(format!(
                "{}/.well-known/openid-configuration",
                config.issuer.trim_end_matches('/')
            ))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        anyhow::ensure!(
            discovery["issuer"].as_str() == Some(config.issuer.as_str()),
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
            issuer: config.issuer.clone(),
            audience: config.audience.clone(),
            identity_key: config.identity_key.clone(),
            operator_role: config.operator_role.clone(),
        })
    }
    pub async fn verify(&self, authorization: &str) -> Result<Identity> {
        let parts: Vec<_> = authorization.split_whitespace().collect();
        if parts.len() != 2 || !parts[0].eq_ignore_ascii_case("Bearer") {
            return Err(Fault::unauthenticated());
        }
        let header = jsonwebtoken::decode_header(parts[1]).map_err(|_| Fault::unauthenticated())?;
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256) {
            return Err(Fault::unauthenticated());
        }
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        validation.leeway = 0;
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let cached = self.keys.read().await;
        let claims = Self::decode(parts[1], header.kid.as_deref(), &cached.0, &validation);
        if let Ok(claims) = claims {
            return self.identity(claims);
        }
        drop(cached);
        // Refresh after verification failure to support provider key rotation,
        // including rotation that keeps the key ID. Serialize concurrent refreshes.
        let mut keys = self.keys.write().await;
        if let Ok(claims) = Self::decode(parts[1], header.kid.as_deref(), &keys.0, &validation) {
            return self.identity(claims);
        }
        *keys = (
            self.http
                .get(&self.jwks_url)
                .send()
                .await
                .map_err(|_| Fault::unauthenticated())?
                .error_for_status()
                .map_err(|_| Fault::unauthenticated())?
                .json()
                .await
                .map_err(|_| Fault::unauthenticated())?,
            Instant::now(),
        );
        self.identity(Self::decode(
            parts[1],
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
                // go-oidc permits five minutes of clock skew for nbf only.
                // Keep exp strict instead of applying JWT-wide leeway.
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
        let mut identity = Identity {
            scopes: claims.scope.split_whitespace().map(str::to_owned).collect(),
            ..Identity::default()
        };
        // Delegated user tokens can contain client_id, but must never gain M2M authority.
        if !claims.client_id.is_empty() && claims.sub == claims.client_id {
            identity.client_id = claims.client_id;
            return Ok(identity);
        }
        if !claims.email_verified || claims.email.is_empty() {
            return Err(Fault::unauthenticated());
        }
        let mut mac = Hmac::<Sha256>::new_from_slice(self.identity_key.as_bytes())
            .map_err(|_| Fault::unauthenticated())?;
        mac.update(claims.iss.as_bytes());
        mac.update(&[0]);
        mac.update(claims.sub.as_bytes());
        identity.subject_hash = hex::encode(mac.finalize().into_bytes());
        identity.email = claims.email.to_lowercase();
        identity.operator = claims.roles.contains(&self.operator_role);
        Ok(identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn verifier() -> Verifier {
        Verifier {
            http: reqwest::Client::new(),
            jwks_url: String::new(),
            keys: RwLock::new((JwkSet { keys: vec![] }, Instant::now())),
            issuer: "https://issuer.example".into(),
            audience: "api".into(),
            identity_key: "test-identity-key".into(),
            operator_role: "operator".into(),
        }
    }
    fn claims(subject: &str, client: &str, verified: bool) -> Claims {
        Claims {
            iss: "https://issuer.example".into(),
            sub: subject.into(),
            nbf: None,
            email: "USER@EXAMPLE.COM".into(),
            email_verified: verified,
            client_id: client.into(),
            scope: "authorize usage:read".into(),
            roles: vec!["operator".into()],
        }
    }
    #[test]
    fn delegated_user_cannot_acquire_service_identity() {
        let identity = verifier()
            .identity(claims("user", "service", true))
            .unwrap();
        assert!(identity.client_id.is_empty());
        assert_eq!(identity.email, "user@example.com");
        assert_eq!(identity.subject_hash.len(), 64);
        assert!(identity.operator);
    }
    #[test]
    fn service_identity_does_not_require_email_or_grant_operator() {
        let identity = verifier()
            .identity(claims("service", "service", false))
            .unwrap();
        assert_eq!(identity.client_id, "service");
        assert!(identity.subject_hash.is_empty());
        assert!(!identity.operator);
        assert_eq!(identity.scopes, vec!["authorize", "usage:read"]);
    }
    #[test]
    fn user_requires_verified_email_and_subject() {
        assert!(
            verifier()
                .identity(claims("user", "service", false))
                .is_err()
        );
        assert!(verifier().identity(claims("", "", true)).is_err());
    }
    #[test]
    fn identity_hash_separates_issuer_and_subject() {
        let v = verifier();
        let original = v.identity(claims("user", "", true)).unwrap().subject_hash;
        let mut other = claims("user", "", true);
        other.iss = "https://other.example".into();
        assert_ne!(original, v.identity(other).unwrap().subject_hash);
    }
    #[tokio::test]
    async fn malformed_authorization_is_rejected() {
        let v = verifier();
        for header in ["", "Basic abc", "Bearer", "Bearer a b", "Bearer broken"] {
            assert!(v.verify(header).await.is_err());
        }
    }
    #[tokio::test]
    async fn signed_tokens_enforce_audience_and_separate_time_tolerances() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use jsonwebtoken::{EncodingKey, Header};
        use rsa::{RsaPrivateKey, pkcs8::EncodePrivateKey, traits::PublicKeyParts};
        use serde_json::json;
        let private = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
        let pem = private.to_pkcs8_pem(Default::default()).unwrap();
        let signing = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
        let mut v = verifier();
        v.keys = RwLock::new((
            serde_json::from_value(json!({"keys":[{
                "kty":"RSA", "kid":"test", "alg":"RS256", "use":"sig",
                "n":URL_SAFE_NO_PAD.encode(private.n().to_bytes_be()),
                "e":URL_SAFE_NO_PAD.encode(private.e().to_bytes_be())
            }]}))
            .unwrap(),
            Instant::now(),
        ));
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test".into());
        let now = chrono::Utc::now().timestamp();
        let valid = json!({"iss":v.issuer,"aud":v.audience,"sub":"service","client_id":"service","exp":now+3600});
        let bearer = |claims: &serde_json::Value| {
            format!(
                "Bearer {}",
                jsonwebtoken::encode(&header, claims, &signing).unwrap()
            )
        };
        assert!(v.verify(&bearer(&valid)).await.is_ok());
        let mut early = valid.clone();
        early["nbf"] = json!(now + 3600);
        assert!(v.verify(&bearer(&early)).await.is_err());
        early["nbf"] = json!(now + 240);
        assert!(v.verify(&bearer(&early)).await.is_ok());
        let mut expired = valid.clone();
        expired["exp"] = json!(now - 1);
        assert!(v.verify(&bearer(&expired)).await.is_err());
        let mut no_audience = valid.clone();
        no_audience.as_object_mut().unwrap().remove("aud");
        assert!(v.verify(&bearer(&no_audience)).await.is_err());
        for audience in [json!(null), json!(42), json!({"api":true}), json!("other")] {
            let mut malformed = valid.clone();
            malformed["aud"] = audience;
            assert!(v.verify(&bearer(&malformed)).await.is_err());
        }
    }
}
