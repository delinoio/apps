// SPDX-License-Identifier: FSL-1.1-ALv2
//! Logto JWT validation with a cached JWKS.
use crate::{ApiError, Result};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use serde_json::Value;
use server_common::authority::bounded_json;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

/// Keys are refreshed after this age and on an unknown key ID.
const KEY_TTL: Duration = Duration::from_secs(3600);
/// Unknown key IDs refresh the set at most this often so invalid tokens cannot flood Logto.
const UNKNOWN_KID_REFRESH: Duration = Duration::from_secs(30);
const ALGORITHMS: [Algorithm; 9] = [
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
    Algorithm::EdDSA,
];

pub struct Verifier {
    http: reqwest::Client,
    issuer: String,
    jwks_url: String,
    keys: RwLock<Option<(JwkSet, Instant)>>,
    refresh: Mutex<()>,
}

#[derive(Clone, Debug)]
pub struct Claims {
    pub subject: String,
}

impl Verifier {
    /// Logto publishes the issuer's signing keys at `<issuer>/jwks`.
    pub fn new(http: reqwest::Client, issuer: &str) -> Self {
        let issuer = issuer.trim_end_matches('/').to_owned();
        Self {
            jwks_url: format!("{issuer}/jwks"),
            http,
            issuer,
            keys: RwLock::new(None),
            refresh: Mutex::new(()),
        }
    }

    async fn fetch(&self) -> Result<JwkSet> {
        let response = self.http.get(&self.jwks_url).send().await.map_err(|_| {
            tracing::warn!(category = "jwks", "Logto key retrieval failed");
            ApiError::dependency()
        })?;
        if !response.status().is_success() {
            tracing::warn!(
                category = "jwks",
                status = response.status().as_u16(),
                "Logto key retrieval failed"
            );
            return Err(ApiError::dependency());
        }
        let body = bounded_json(response, 256 << 10)
            .await
            .map_err(|_| ApiError::dependency())?;
        serde_json::from_value(body).map_err(|_| ApiError::dependency())
    }

    async fn key(&self, kid: &str) -> Result<Option<jsonwebtoken::jwk::Jwk>> {
        let lookup = |keys: &Option<(JwkSet, Instant)>| {
            keys.as_ref()
                .map(|(set, at)| (set.find(kid).cloned(), at.elapsed()))
        };
        let cached = lookup(&*self.keys.read().await);
        match cached {
            Some((Some(key), age)) if age < KEY_TTL => return Ok(Some(key)),
            Some((None, age)) if age < UNKNOWN_KID_REFRESH => return Ok(None),
            _ => {}
        }
        let _guard = self.refresh.lock().await;
        let cached = lookup(&*self.keys.read().await);
        match cached {
            Some((Some(key), age)) if age < KEY_TTL => return Ok(Some(key)),
            Some((None, age)) if age < UNKNOWN_KID_REFRESH => return Ok(None),
            _ => {}
        }
        let set = self.fetch().await?;
        let key = set.find(kid).cloned();
        *self.keys.write().await = Some((set, Instant::now()));
        Ok(key)
    }

    /// Validates the signature, `iss`, `aud`, `exp` and `nbf` of a Logto access token.
    pub async fn verify(&self, token: &str, audience: &str) -> Result<Claims> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| ApiError::unauthenticated())?;
        if !ALGORITHMS.contains(&header.alg) {
            return Err(ApiError::unauthenticated());
        }
        let kid = header.kid.ok_or_else(ApiError::unauthenticated)?;
        let jwk = self
            .key(&kid)
            .await?
            .ok_or_else(ApiError::unauthenticated)?;
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| ApiError::unauthenticated())?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[audience]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let claims = jsonwebtoken::decode::<Value>(token, &key, &validation)
            .map_err(|_| ApiError::unauthenticated())?
            .claims;
        let subject = claims["sub"].as_str().unwrap_or_default();
        if subject.is_empty() || subject.len() > 255 {
            return Err(ApiError::unauthenticated());
        }
        Ok(Claims {
            subject: subject.into(),
        })
    }
}

/// Bearer credential kinds presented to the API.
pub enum Bearer<'a> {
    None,
    Device(&'a str),
    User(&'a str),
    Malformed,
}

pub fn bearer(authorization: Option<&str>) -> Bearer<'_> {
    let Some(value) = authorization else {
        return Bearer::None;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Bearer::Malformed;
    };
    if token.starts_with("ada_") {
        if valid_token(token, "ada_") {
            Bearer::Device(token)
        } else {
            Bearer::Malformed
        }
    } else if token.is_empty() || token.len() > 8192 {
        Bearer::Malformed
    } else {
        Bearer::User(token)
    }
}

/// `<prefix><43 base64url characters>`.
pub fn valid_token(token: &str, prefix: &str) -> bool {
    token.strip_prefix(prefix).is_some_and(|rest| {
        rest.len() == 43
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

/// A random 32-byte secret in unpadded base64url with an optional prefix.
pub fn secret(prefix: &str) -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!(
        "{prefix}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use axum::{Json, Router, routing::get};
    use serde_json::json;

    // Public test fixture only; this key never signs production credentials.
    pub const TEST_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQglAnLdw1E7sKU4Od0
KUR6w73q0kPwV2ZuIrCYCc5O+1+hRANCAAS7WL2yuRlkebV9eg5DC8EgeL5sLCc+
SWa6SCqGYyxWx3gNrhl069h8P2zMhX5WKLrOwiNGwCHc0xVxIsdq2G49
-----END PRIVATE KEY-----
"#;
    pub fn jwks() -> Value {
        json!({"keys":[{"kty":"EC","crv":"P-256","alg":"ES256","use":"sig","kid":"fixture","x":"u1i9srkZZHm1fXoOQwvBIHi-bCwnPklmukgqhmMsVsc","y":"eA2uGXTr2Hw_bMyFflYous7CI0bAIdzTFXEix2rYbj0"}]})
    }
    pub fn sign(claims: Value) -> String {
        let mut h = jsonwebtoken::Header::new(Algorithm::ES256);
        h.kid = Some("fixture".into());
        jsonwebtoken::encode(
            &h,
            &claims,
            &jsonwebtoken::EncodingKey::from_ec_pem(TEST_KEY.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn validates_issuer_audience_expiry_and_reports_key_outages() {
        let app = Router::new().route("/oidc/jwks", get(|| async { Json(jwks()) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let issuer = format!("{base}/oidc");
        let verifier = Verifier::new(reqwest::Client::new(), &issuer);
        let now = chrono::Utc::now().timestamp();
        let good =
            sign(json!({"iss":issuer,"aud":"api","sub":"user-1","exp":now+600,"nbf":now-10}));
        assert_eq!(
            verifier.verify(&good, "api").await.unwrap().subject,
            "user-1"
        );
        for claims in [
            json!({"iss":"https://other","aud":"api","sub":"u","exp":now+600}),
            json!({"iss":issuer,"aud":"other","sub":"u","exp":now+600}),
            json!({"iss":issuer,"aud":"api","sub":"u","exp":now-600}),
            json!({"iss":issuer,"aud":"api","sub":"u","exp":now+600,"nbf":now+600}),
            json!({"iss":issuer,"aud":"api","exp":now+600}),
        ] {
            let e = verifier
                .verify(&sign(claims.clone()), "api")
                .await
                .unwrap_err();
            assert_eq!(
                e.reason,
                delino_protocol::anydash::v1::ErrorReason::Unauthenticated,
                "{claims}"
            );
        }
        assert!(verifier.verify("not-a-jwt", "api").await.is_err());
        // An unreachable JWKS is a dependency failure, never an invalid caller credential.
        let down = Verifier::new(reqwest::Client::new(), "http://127.0.0.1:1/oidc");
        let e = down.verify(&good, "api").await.unwrap_err();
        assert_eq!(
            e.reason,
            delino_protocol::anydash::v1::ErrorReason::DependencyUnavailable
        );
    }

    #[test]
    fn bearer_kinds_and_token_shapes() {
        let device = secret("ada_");
        assert!(valid_token(&device, "ada_"));
        assert!(matches!(
            bearer(Some(&format!("Bearer {device}"))),
            Bearer::Device(_)
        ));
        assert!(matches!(
            bearer(Some("Bearer ada_short")),
            Bearer::Malformed
        ));
        assert!(matches!(bearer(Some("Bearer eyJ.x.y")), Bearer::User(_)));
        assert!(matches!(bearer(Some("Basic x")), Bearer::Malformed));
        assert!(matches!(bearer(None), Bearer::None));
        assert!(!valid_token(&secret("adr_"), "ada_"));
    }
}
