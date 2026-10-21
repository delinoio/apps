// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::Server;
use delino_protocol::gittracker::v1::{Context, Repository};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use serde_json::Value;
use server_common::{
    Fault, Result,
    authority::{Role, bounded_json},
    integer, string,
};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
pub struct Actions {
    http: reqwest::Client,
    jwks_url: String,
    keys: RwLock<(JwkSet, Instant)>,
}
impl Actions {
    pub fn new(http: reqwest::Client, jwks_url: String, keys: JwkSet) -> Self {
        Self {
            http,
            jwks_url,
            keys: RwLock::new((keys, Instant::now())),
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
        })
    }
    pub async fn verify(&self, raw: &str) -> Result<Value> {
        let header = jsonwebtoken::decode_header(raw).map_err(|_| Fault::denied())?;
        if header.alg != Algorithm::RS256 {
            return Err(Fault::denied());
        }
        let kid = header.kid.ok_or_else(Fault::denied)?;
        let read = self.keys.read().await;
        if read.1.elapsed() > Duration::from_secs(300) || read.0.find(&kid).is_none() {
            drop(read);
            let mut keys = self.keys.write().await;
            if keys.1.elapsed() > Duration::from_secs(300) || keys.0.find(&kid).is_none() {
                let response = self.http.get(&self.jwks_url).send().await?;
                if !response.status().is_success() {
                    return Err(Fault::unavailable());
                }
                *keys = (
                    serde_json::from_value(bounded_json(response, 256 << 10).await?)?,
                    Instant::now(),
                );
            }
        } else {
            drop(read)
        }
        let keys = self.keys.read().await;
        let key = keys.0.find(&kid).ok_or_else(Fault::denied)?;
        let key = DecodingKey::from_jwk(key).map_err(|_| Fault::denied())?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&["https://token.actions.githubusercontent.com"]);
        validation.set_audience(&["https://git-tracker.api.delino.io"]);
        validation.validate_nbf = true;
        jsonwebtoken::decode::<Value>(raw, &key, &validation)
            .map(|t| t.claims)
            .map_err(|_| Fault::denied())
    }
}
impl Server {
    pub async fn authorize(
        &self,
        db: &mut PgConnection,
        h: &str,
        r: &Repository,
        role: Role,
        c: Option<&Context>,
    ) -> Result<()> {
        if role == Role::View && r.public && r.connected {
            return Ok(());
        }
        if h.is_empty() {
            return Err(Fault::unauthenticated());
        }
        let token = h.strip_prefix("Bearer ").unwrap_or(h);
        if token.starts_with("gt_") {
            if role != Role::Edit || !r.connected {
                return Err(Fault::denied());
            }
            let ok:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM git_tracker_tokens WHERE digest=$1 AND repository_id=$2 AND NOT revoked AND expires_at>$3)").bind(Sha256::digest(token).to_vec()).bind(server_common::uuid(&r.id)?).bind(server_common::now()).fetch_one(db).await?;
            return if ok {
                Ok(())
            } else {
                Err(Fault::unauthenticated())
            };
        }
        if role == Role::Edit
            && let (Some(raw), Some(c)) = (h.strip_prefix("Actions "), c)
        {
            return self.verify_actions(raw, r, c).await;
        }
        self.authority.allow(h, &r.space_id, role).await
    }
    async fn verify_actions(&self, raw: &str, r: &Repository, c: &Context) -> Result<()> {
        if !r.connected {
            return Err(Fault::denied());
        }
        let claims = self.actions.verify(raw).await?;
        let (rid, run, attempt) = (
            string(&claims, "repository_id"),
            string(&claims, "run_id"),
            string(&claims, "run_attempt"),
        );
        if rid.is_empty()
            || run.is_empty()
            || attempt.is_empty()
            || c.execution_id != format!("{rid}/{run}/{attempt}")
        {
            return Err(Fault::denied());
        }
        if c.pull_request == 0 {
            return if rid == r.github_repository_id
                && claims["sha"] == c.commit
                && claims["ref"] == format!("refs/heads/{}", c.branch)
                && claims["event_name"] == "push"
            {
                Ok(())
            } else {
                Err(Fault::denied())
            };
        }
        if claims["event_name"] != "pull_request" {
            return Err(Fault::denied());
        }
        let p = self.github.pull(r, c.pull_request).await?;
        if c.branch != string(&p["base"], "ref")
            || !c.baseline_commit.is_empty()
            || (rid != r.github_repository_id
                && rid != integer(&p["head"]["repo"], "id").to_string())
        {
            return Err(Fault::denied());
        }
        self.github
            .verify_merge(r, string(&claims, "sha"), &c.commit)
            .await?;
        if claims["ref"] != format!("refs/pull/{}/merge", c.pull_request)
            || !string(&claims, "sub").ends_with(":pull_request")
        {
            return Err(Fault::denied());
        }
        Ok(())
    }
}
