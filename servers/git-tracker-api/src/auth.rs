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
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};

/// Maximum age of a successfully fetched JWKS snapshot that may authorize a token. Failed or
/// suppressed refreshes never extend it.
const SNAPSHOT_FRESHNESS: Duration = Duration::from_secs(300);
/// Minimum time between two JWKS refresh attempt starts in this process. It covers successful
/// and failed attempts and every unknown `kid`, so an unauthenticated token header cannot
/// amplify outbound requests. A rotated key becomes usable on the first eligible attempt.
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);
/// Upper bound for a JWKS response body.
const JWKS_BODY_LIMIT: usize = 256 << 10;

/// Monotonic time source. Production uses [`Instant::now`]; tests substitute a controlled clock.
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Outcome category of one refresh decision, used for bounded structured logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefreshOutcome {
    Refreshed,
    Suppressed,
    TransportFailed,
    StatusFailed,
    BodyFailed,
    ParseFailed,
}
impl RefreshOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Refreshed => "refreshed",
            Self::Suppressed => "suppressed",
            Self::TransportFailed => "transport_failed",
            Self::StatusFailed => "status_failed",
            Self::BodyFailed => "body_failed",
            Self::ParseFailed => "parse_failed",
        }
    }
}

/// Last successfully fetched key set and the time it was fetched.
struct Snapshot {
    set: JwkSet,
    fetched: Instant,
}

/// Single-flight refresh coordination, kept separate from the key cache so that a network wait
/// never blocks verification of a token whose key is fresh in the cache.
struct Refresh {
    /// Start of the previous refresh attempt, whatever its result. `None` until the first attempt.
    last_attempt: Option<Instant>,
}

pub struct Actions {
    http: reqwest::Client,
    jwks_url: String,
    keys: RwLock<Snapshot>,
    /// Held across a refresh fetch, so at most one fetch is in flight. Dropping the holder
    /// (including by cancellation) releases it; `last_attempt` was already recorded, so a
    /// cancelled attempt does not allow an immediate retry.
    refresh: Mutex<Refresh>,
    clock: Clock,
}
impl Actions {
    pub fn new(http: reqwest::Client, jwks_url: String, keys: JwkSet) -> Self {
        Self::build(http, jwks_url, keys, None, Arc::new(Instant::now))
    }
    fn build(
        http: reqwest::Client,
        jwks_url: String,
        keys: JwkSet,
        last_attempt: Option<Instant>,
        clock: Clock,
    ) -> Self {
        let fetched = clock();
        Self {
            http,
            jwks_url,
            keys: RwLock::new(Snapshot { set: keys, fetched }),
            refresh: Mutex::new(Refresh { last_attempt }),
            clock,
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
        let attempt = Instant::now();
        let keys = bounded_json(
            http.get(&jwks_url).send().await?.error_for_status()?,
            JWKS_BODY_LIMIT,
        )
        .await?;
        // The startup fetch counts as the first refresh attempt for the retry interval.
        Ok(Self::build(
            http,
            jwks_url,
            serde_json::from_value(keys)?,
            Some(attempt),
            Arc::new(Instant::now),
        ))
    }
    #[cfg(test)]
    pub(crate) fn with_clock(
        http: reqwest::Client,
        jwks_url: String,
        keys: JwkSet,
        clock: Clock,
    ) -> Self {
        Self::build(http, jwks_url, keys, None, clock)
    }
    pub async fn verify(&self, raw: &str) -> Result<Value> {
        let header = jsonwebtoken::decode_header(raw).map_err(|_| Fault::denied())?;
        if header.alg != Algorithm::RS256 {
            return Err(Fault::denied());
        }
        let kid = header.kid.ok_or_else(Fault::denied)?;
        let key = self.signing_key(&kid).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&["https://token.actions.githubusercontent.com"]);
        validation.set_audience(&["https://git-tracker.api.delino.io"]);
        validation.validate_nbf = true;
        jsonwebtoken::decode::<Value>(raw, &key, &validation)
            .map(|t| t.claims)
            .map_err(|_| Fault::denied())
    }
    /// Resolves the decoding key for `kid` from a fresh snapshot, refreshing at most once per
    /// [`REFRESH_INTERVAL`] when the key is absent or the snapshot has expired.
    async fn signing_key(&self, kid: &str) -> Result<DecodingKey> {
        if let Some(key) = self.cached_key(kid).await? {
            return Ok(key);
        }
        self.refresh_for(kid).await?;
        match self.cached_key(kid).await? {
            Some(key) => Ok(key),
            // An expired snapshot fails closed as a dependency failure; an unknown key in a
            // fresh snapshot is a credential the provider does not publish.
            None if self.snapshot_expired().await => Err(Fault::unavailable()),
            None => Err(Fault::denied()),
        }
    }
    /// Returns the key only when the snapshot is within [`SNAPSHOT_FRESHNESS`]. The read lock is
    /// held only for the in-memory lookup.
    async fn cached_key(&self, kid: &str) -> Result<Option<DecodingKey>> {
        let snapshot = self.keys.read().await;
        if self.expired(&snapshot) {
            return Ok(None);
        }
        snapshot
            .set
            .find(kid)
            .map(|jwk| DecodingKey::from_jwk(jwk).map_err(|_| Fault::denied()))
            .transpose()
    }
    async fn snapshot_expired(&self) -> bool {
        self.expired(&*self.keys.read().await)
    }
    fn expired(&self, snapshot: &Snapshot) -> bool {
        (self.clock)().saturating_duration_since(snapshot.fetched) > SNAPSHOT_FRESHNESS
    }
    /// Joins or starts the single refresh. Callers queue on the coordinator (not the cache lock),
    /// then re-evaluate the shared result: a refresh that already supplied the key, or a recent
    /// attempt, means no further fetch. Returns an error only when this call's own fetch failed.
    async fn refresh_for(&self, kid: &str) -> Result<()> {
        let mut refresh = self.refresh.lock().await;
        if self.cached_key(kid).await?.is_some() {
            return Ok(());
        }
        let now = (self.clock)();
        if let Some(last) = refresh.last_attempt {
            let since = now.saturating_duration_since(last);
            if since < REFRESH_INTERVAL {
                tracing::debug!(
                    outcome = RefreshOutcome::Suppressed.as_str(),
                    retry_after_ms = (REFRESH_INTERVAL - since).as_millis() as u64,
                    "Actions JWKS refresh suppressed"
                );
                return Ok(());
            }
        }
        // Record the attempt before the first await so cancellation cannot permit a retry storm.
        refresh.last_attempt = Some(now);
        match self.fetch_keys().await {
            Ok(set) => {
                let key_count = set.keys.len();
                *self.keys.write().await = Snapshot {
                    set,
                    fetched: (self.clock)(),
                };
                tracing::info!(
                    outcome = RefreshOutcome::Refreshed.as_str(),
                    key_count,
                    "Actions JWKS refreshed"
                );
                Ok(())
            }
            Err(outcome) => {
                tracing::warn!(
                    outcome = outcome.as_str(),
                    "Actions JWKS refresh failed; keeping the previous snapshot"
                );
                Err(Fault::unavailable())
            }
        }
    }
    /// Fetches and parses the key set without holding the cache lock.
    async fn fetch_keys(&self) -> std::result::Result<JwkSet, RefreshOutcome> {
        let response = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|_| RefreshOutcome::TransportFailed)?;
        if !response.status().is_success() {
            return Err(RefreshOutcome::StatusFailed);
        }
        let body = bounded_json(response, JWKS_BODY_LIMIT)
            .await
            .map_err(|_| RefreshOutcome::BodyFailed)?;
        serde_json::from_value(body).map_err(|_| RefreshOutcome::ParseFailed)
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
