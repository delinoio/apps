// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Fault, Result, string};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use serde_json::{Value, json};
use server_common::authority::{Authority, TenantState, bounded_json};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
pub struct Actions {
    http: reqwest::Client,
    jwks_url: String,
    keys: RwLock<(JwkSet, Instant)>,
    refresh_lock: Mutex<()>,
}
impl Actions {
    pub fn new(http: reqwest::Client, jwks_url: String, keys: JwkSet) -> Self {
        Self {
            http,
            jwks_url,
            keys: RwLock::new((keys, Instant::now())),
            refresh_lock: Mutex::new(()),
        }
    }
    pub async fn discover() -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let discovery = bounded_json(
            http.get(
                "https://token.actions.githubusercontent.com/.well-known/openid-configuration",
            )
            .send()
            .await?
            .error_for_status()?,
            64 << 10,
        )
        .await?;
        anyhow::ensure!(
            discovery["issuer"] == "https://token.actions.githubusercontent.com",
            "Actions OIDC issuer mismatch"
        );
        let jwks_url = string(&discovery, "jwks_uri").to_owned();
        anyhow::ensure!(
            jwks_url.starts_with("https://token.actions.githubusercontent.com/"),
            "invalid Actions JWKS endpoint"
        );
        let keys = bounded_json(
            http.get(&jwks_url).send().await?.error_for_status()?,
            256 << 10,
        )
        .await?;
        Ok(Self {
            http,
            jwks_url,
            keys: RwLock::new((serde_json::from_value(keys)?, Instant::now())),
            refresh_lock: Mutex::new(()),
        })
    }
    pub async fn verify(&self, raw: &str) -> Result<Value> {
        let header = jsonwebtoken::decode_header(raw).map_err(|_| Fault::denied())?;
        if header.alg != Algorithm::RS256 {
            return Err(Fault::denied());
        }
        let kid = header.kid.ok_or_else(Fault::denied)?;
        let needs_refresh = {
            let keys = self.keys.read().await;
            keys.1.elapsed() > Duration::from_secs(300) || keys.0.find(&kid).is_none()
        };
        if needs_refresh {
            // Serialize refreshes separately from the key cache. A key in the fresh
            // cache stays usable while the provider is slow; cancellation keeps it.
            let _refresh = self.refresh_lock.lock().await;
            let needs_refresh = {
                let keys = self.keys.read().await;
                keys.1.elapsed() > Duration::from_secs(300) || keys.0.find(&kid).is_none()
            };
            if needs_refresh {
                let response = self.http.get(&self.jwks_url).send().await?;
                if !response.status().is_success() {
                    return Err(Fault::unavailable());
                }
                let fresh = serde_json::from_value(bounded_json(response, 256 << 10).await?)?;
                *self.keys.write().await = (fresh, Instant::now());
            }
        }
        let keys = self.keys.read().await;
        let key = keys.0.find(&kid).ok_or_else(Fault::denied)?;
        let key = DecodingKey::from_jwk(key).map_err(|_| Fault::denied())?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&["https://token.actions.githubusercontent.com"]);
        validation.set_audience(&["https://perfine.api.delino.io"]);
        validation.validate_nbf = true;
        validation.set_required_spec_claims(&["exp", "nbf", "iat", "iss", "aud", "sub"]);
        jsonwebtoken::decode::<Value>(raw, &key, &validation)
            .map(|t| t.claims)
            .map_err(|_| Fault::denied())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Read,
    Experiment,
    Publish,
    Manage,
}
impl Permission {
    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Experiment => "experiment",
            Self::Publish => "publish",
            Self::Manage => "manage",
        }
    }
}
pub async fn authorize(
    authority: &Authority,
    header: &str,
    space: &str,
    permission: Permission,
) -> Result<String> {
    crate::id(space)?;
    if authority.states(&[space.into()]).await? != [TenantState::Active] {
        return Err(Fault::denied());
    }
    let token = format!("Bearer {}", authority.service_token().await?);
    let secret = header
        .strip_prefix("Bearer ")
        .ok_or_else(Fault::unauthenticated)?;
    if secret.starts_with("dlk_") {
        let v = authority
            .call(
                "AuthorizeApiKey",
                &token,
                json!({"secret":secret,"capability":permission.name(),"paid":false}),
            )
            .await?;
        if v["allowed"] != true || v["tenantId"] != space {
            return Err(Fault::denied());
        }
        crate::id(string(&v, "keyId"))?;
        return Ok(format!("key:{}", string(&v, "keyId")));
    }
    let user = authority.identify(header).await?;
    // Delcore's generic role hierarchy must never imply a Perfine permission.
    let v = authority
        .call(
            "Authorize",
            &token,
            json!({"tenantId":space,"userId":user,"capability":permission.name(),"paid":false}),
        )
        .await?;
    if v["allowed"] != true {
        return Err(Fault::denied());
    }
    Ok(user)
}

/// Bind the signed job to the current installation's repository and the exact GitHub attempt.
/// The caller separately rechecks ACTIVE space and the connection principal's experiment grant.
pub fn execution_scope(
    claims: &Value,
    execution: &Value,
    repository: &Value,
    remote: &Value,
) -> Result<()> {
    use crate::integer;
    let rid = integer(repository, "githubRepositoryId");
    let run = integer(execution, "githubRunId");
    let attempt = integer(execution, "runAttempt");
    let repo = format!(
        "{}/{}",
        string(repository, "owner"),
        string(repository, "name")
    );
    let path = string(execution, "workflowPath");
    let reference = string(claims, "ref");
    let source = string(execution, "sourceSha");
    let workflow = string(execution, "workflowSha");
    let full_sha = |s: &str| {
        s.len() == 40
            && s.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    };
    if rid == 0
        || run == 0
        || attempt == 0
        || string(claims, "repository_id") != rid.to_string()
        || claims["repository"] != repo
        || string(claims, "run_id") != run.to_string()
        || string(claims, "run_attempt") != attempt.to_string()
        || !full_sha(source)
        || !full_sha(workflow)
        || claims["sha"] != source
        || claims["workflow_sha"] != workflow
        || claims["event_name"] != "workflow_dispatch"
        || !path.starts_with(".github/workflows/")
        || path.contains("..")
        || path.contains('@')
        || !(path.ends_with(".yml") || path.ends_with(".yaml"))
        || !(reference.starts_with("refs/heads/") || reference.starts_with("refs/tags/"))
        || claims["workflow_ref"] != format!("{repo}/{path}@{reference}")
        || integer(remote, "id") != run
        || integer(remote, "run_attempt") != attempt
        || integer(&remote["repository"], "id") != rid
        || integer(&remote["head_repository"], "id") != rid
        || remote["head_sha"] != source
        || remote["path"] != path
        || remote["event"] != "workflow_dispatch"
        || integer(&remote["actor"], "id") == 0
        || string(claims, "actor_id") != integer(&remote["actor"], "id").to_string()
    {
        return Err(Fault::denied());
    }
    Ok(())
}
