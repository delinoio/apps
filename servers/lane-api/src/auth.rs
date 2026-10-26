// SPDX-License-Identifier: FSL-1.1-ALv2
//! Lane credentials and Logto browser approval. Domain functions use the caller's transaction.
use crate::{Fault, Principal, Result, account_lock, canonical_id, hash, id, string};
use axum::{
    Form, Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use server_common::authority::{Authority, bounded_json};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool, Row, postgres::PgRow};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use url::Url;

const COOKIE: &str = "__Host-lane_session";
const FLOW_COOKIE: &str = "__Host-lane_flow";
const ACCESS_SECONDS: i64 = 900;
const REFRESH_SECONDS: i64 = 30 * 24 * 3600;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Client {
    name: String,
    redirect_uris: Vec<String>,
    origin_kind: String,
    resource: String,
}
#[derive(Clone)]
struct Config {
    public_url: String,
    issuer: String,
    logto_client: String,
    logto_secret: String,
    delcore_resource: String,
    return_urls: Vec<String>,
    clients: HashMap<String, Client>,
    auth_per_minute: i64,
}
/// No credentials are cached as authorization decisions. Delcore is checked for every operation.
pub struct Auth {
    pool: PgPool,
    config: Config,
    authority: Authority,
    http: reqwest::Client,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}
impl Auth {
    pub async fn from_env(pool: PgPool) -> anyhow::Result<Self> {
        use server_common::config::{required, safe_url};
        let public_url = required("LANE_PUBLIC_URL")?
            .trim_end_matches('/')
            .to_owned();
        let issuer = required("LOGTO_ISSUER")?;
        let return_urls: Vec<String> = serde_json::from_str(&required("LANE_WEB_RETURN_URLS")?)?;
        let clients: HashMap<String, Client> =
            serde_json::from_str(&required("LANE_OAUTH_CLIENTS")?)?;
        anyhow::ensure!(
            safe_url(&public_url) && safe_url(&issuer),
            "invalid public URL or Logto issuer"
        );
        anyhow::ensure!(
            !return_urls.is_empty() && !clients.is_empty(),
            "web URLs and OAuth clients are required"
        );
        for url in &return_urls {
            let u = Url::parse(url)?;
            anyhow::ensure!(
                safe_url(url) && u.fragment().is_none() && u.query().is_none(),
                "invalid web return URL"
            );
        }
        for (key, client) in &clients {
            anyhow::ensure!(
                !key.is_empty()
                    && key != "__web"
                    && !client.name.trim().is_empty()
                    && !client.redirect_uris.is_empty(),
                "invalid OAuth client"
            );
            anyhow::ensure!(
                matches!(
                    client.origin_kind.as_str(),
                    "ORIGIN_KIND_APP" | "ORIGIN_KIND_CLI" | "ORIGIN_KIND_MCP"
                ),
                "invalid client origin"
            );
            let expected = if client.origin_kind == "ORIGIN_KIND_MCP" {
                format!("{public_url}/mcp")
            } else {
                public_url.clone()
            };
            anyhow::ensure!(
                client.resource == expected,
                "client resource does not match Lane"
            );
            for redirect in &client.redirect_uris {
                let u = Url::parse(redirect)?;
                anyhow::ensure!(
                    u.fragment().is_none()
                        && u.query().is_none()
                        && u.username().is_empty()
                        && u.password().is_none(),
                    "invalid redirect"
                );
                anyhow::ensure!(
                    safe_url(redirect)
                        || matches!(u.scheme(), "lane" | "lane-dev")
                            && u.host_str() == Some("auth")
                            && u.path() == "/callback",
                    "unsafe redirect"
                );
            }
        }
        let auth_per_minute = required("AUTH_REQUESTS_PER_MINUTE")?.parse()?;
        anyhow::ensure!(
            auth_per_minute > 0,
            "AUTH_REQUESTS_PER_MINUTE must be positive"
        );
        let config = Config {
            public_url,
            issuer,
            return_urls,
            clients,
            auth_per_minute,
            logto_client: required("LANE_LOGTO_CLIENT_ID")?,
            logto_secret: required("LANE_LOGTO_CLIENT_SECRET")?,
            delcore_resource: required("DELCORE_RESOURCE")?,
        };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let response = http
            .get(format!(
                "{}/.well-known/openid-configuration",
                config.issuer.trim_end_matches('/')
            ))
            .send()
            .await?
            .error_for_status()?;
        let metadata = bounded_json(response, 64 << 10).await?;
        anyhow::ensure!(
            string(&metadata, "issuer") == config.issuer,
            "OIDC issuer mismatch"
        );
        for key in ["authorization_endpoint", "token_endpoint", "jwks_uri"] {
            anyhow::ensure!(safe_url(string(&metadata, key)), "invalid OIDC endpoint");
        }
        Ok(Self {
            pool,
            authority: Authority::from_env(Duration::from_secs(10))?,
            http,
            authorization_endpoint: string(&metadata, "authorization_endpoint").into(),
            token_endpoint: string(&metadata, "token_endpoint").into(),
            jwks_uri: string(&metadata, "jwks_uri").into(),
            config,
        })
    }

    fn registered_redirect(&self, client_id: &str, redirect: &str) -> Result<()> {
        let allowed = if client_id == "__web" {
            self.config.return_urls.iter().any(|u| u == redirect)
        } else {
            self.config
                .clients
                .get(client_id)
                .is_some_and(|c| c.redirect_uris.iter().any(|u| u == redirect))
        };
        if !allowed {
            return Err(Fault::denied());
        }
        Ok(())
    }

    fn validate_approval(&self, request: &Value) -> Result<()> {
        match string(request, "kind") {
            "web" => self.registered_redirect("__web", string(request, "return_to")),
            "oauth" => {
                let client_id = string(request, "client_id");
                self.registered_redirect(client_id, string(request, "redirect_uri"))?;
                let client = self
                    .config
                    .clients
                    .get(client_id)
                    .ok_or_else(Fault::denied)?;
                // Persisted flows can outlive a rollout that changes a client registration.
                if string(request, "resource") != client.resource
                    || string(request, "origin_kind") != client.origin_kind
                {
                    return Err(Fault::denied());
                }
                Ok(())
            }
            _ => Err(Fault::denied()),
        }
    }

    pub fn public_url(&self) -> &str {
        &self.config.public_url
    }

    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal> {
        self.authenticate_resource(headers, &self.config.public_url)
            .await
    }

    /// MCP callers use the registered `/mcp` resource; never forward their token to Delcore.
    pub async fn authenticate_resource(
        &self,
        headers: &HeaderMap,
        resource: &str,
    ) -> Result<Principal> {
        let (token, kind) = credential(headers)?;
        let row = sqlx::query("SELECT s.*,d.name FROM lane_credentials c JOIN lane_sessions s ON s.id=c.session_id JOIN lane_devices d ON d.account_id=s.account_id AND d.id=s.device_id WHERE c.digest=$1 AND c.kind=$2 AND c.expires_at>clock_timestamp() AND c.consumed_at IS NULL AND s.revoked_at IS NULL AND d.disconnected_at IS NULL AND s.resource=$3 AND s.issuer=$4")
            .bind(hash(token.as_bytes())).bind(kind).bind(resource).bind(&self.config.public_url)
            .fetch_optional(&self.pool).await?.ok_or_else(Fault::unauthenticated)?;
        let account: String = row.get("account_id");
        let tenant: String =
            sqlx::query_scalar("SELECT tenant_id FROM lane_accounts WHERE account_id=$1")
                .bind(&account)
                .fetch_one(&self.pool)
                .await?;
        Ok(principal(&row, tenant))
    }

    /// The caller holds the account advisory lock in this transaction. Recheck after that lock.
    pub async fn authorize(&self, db: &mut PgConnection, p: &Principal) -> Result<()> {
        let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM lane_sessions s JOIN lane_devices d ON d.account_id=s.account_id AND d.id=s.device_id JOIN lane_accounts a ON a.account_id=s.account_id WHERE s.id=$1 AND s.account_id=$2 AND s.device_id=$3 AND a.tenant_id=$4 AND s.epoch=a.epoch AND s.revoked_at IS NULL AND d.disconnected_at IS NULL)")
            .bind(&p.session_id).bind(&p.account_id).bind(&p.device_id).bind(&p.tenant_id).fetch_one(db).await?;
        if !active {
            return Err(Fault::unauthenticated());
        }
        self.allow_account(&p.account_id, &p.tenant_id).await
    }

    /// Signed webhook intake calls this after acquiring the account lock.
    pub async fn authorize_account(&self, db: &mut PgConnection, account: &str) -> Result<()> {
        let tenant: String =
            sqlx::query_scalar("SELECT tenant_id FROM lane_accounts WHERE account_id=$1")
                .bind(account)
                .fetch_optional(db)
                .await?
                .ok_or_else(Fault::denied)?;
        self.allow_account(account, &tenant).await
    }

    async fn allow_account(&self, account: &str, tenant: &str) -> Result<()> {
        let token = self
            .authority
            .service_token()
            .await
            .map_err(|_| Fault::unavailable())?;
        let authorization = format!("Bearer {token}");
        let allowed = self
            .authority
            .call(
                "Authorize",
                &authorization,
                json!({"tenantId":tenant,"userId":account,"capability":"manage","paid":false}),
            )
            .await
            .map_err(|_| Fault::unavailable())?;
        let states = self
            .authority
            .call(
                "GetTenantStates",
                &authorization,
                json!({"tenantIds":[tenant]}),
            )
            .await
            .map_err(|_| Fault::unavailable())?;
        let entries = states["states"].as_array().ok_or_else(Fault::unavailable)?;
        if entries.len() != 1 || string(&entries[0], "tenantId") != tenant {
            return Err(Fault::unavailable());
        }
        if allowed["allowed"] != true
            || !matches!(entries[0]["state"].as_str(), Some("TENANT_STATE_ACTIVE"))
        {
            return Err(Fault::denied());
        }
        Ok(())
    }

    async fn personal_account(&self, token: &str) -> Result<(String, String)> {
        let mut account = String::new();
        let mut tenant = None;
        let mut cursor = String::new();
        let mut seen = HashSet::new();
        loop {
            let page = self
                .authority
                .call(
                    "GetCurrentUser",
                    &format!("Bearer {token}"),
                    json!({"page":{"size":100,"afterId":cursor}}),
                )
                .await?;
            let user = string(&page, "userId");
            if !canonical_id(user) || (!account.is_empty() && account != user) {
                return Err(Fault::unavailable());
            }
            account = user.into();
            for entry in page["tenants"].as_array().ok_or_else(Fault::unavailable)? {
                if entry["personal"] == true && string(entry, "ownerId") == account {
                    let found = string(entry, "id");
                    if !canonical_id(found) || tenant.is_some() {
                        return Err(Fault::denied());
                    }
                    tenant = Some(found.to_owned());
                }
            }
            cursor = string(&page, "nextAfterId").into();
            if cursor.is_empty() {
                break;
            }
            if !seen.insert(cursor.clone()) {
                return Err(Fault::unavailable());
            }
        }
        Ok((account, tenant.ok_or_else(Fault::denied)?))
    }

    async fn csrf(&self, headers: &HeaderMap, p: &Principal) -> Result<()> {
        if headers.contains_key(header::AUTHORIZATION) {
            return Ok(());
        }
        let origin = headers
            .get(header::ORIGIN)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(Fault::denied)?;
        if !self
            .config
            .return_urls
            .iter()
            .any(|u| Url::parse(u).is_ok_and(|u| u.origin().ascii_serialization() == origin))
        {
            return Err(Fault::denied());
        }
        let csrf = headers
            .get("x-lane-csrf")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(Fault::denied)?;
        let valid: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM lane_sessions WHERE id=$1 AND csrf_hash=$2)",
        )
        .bind(&p.session_id)
        .bind(hash(csrf.as_bytes()))
        .fetch_one(&self.pool)
        .await?;
        if !valid {
            return Err(Fault::denied());
        }
        Ok(())
    }

    /// Connect mutation adapters must call this before entering the domain transaction.
    pub async fn check_csrf(&self, headers: &HeaderMap, p: &Principal) -> Result<()> {
        self.csrf(headers, p).await
    }

    async fn rate(&self) -> Result<()> {
        // A global database bucket bounds unauthenticated work without trusting proxy IP headers.
        let count: i64 = sqlx::query_scalar("INSERT INTO lane_auth_limits(bucket,minute,count) VALUES('auth',floor(extract(epoch FROM clock_timestamp())/60)::bigint,1) ON CONFLICT(bucket,minute) DO UPDATE SET count=lane_auth_limits.count+1 RETURNING count")
            .fetch_one(&self.pool).await?;
        sqlx::query("DELETE FROM lane_auth_limits WHERE minute<floor(extract(epoch FROM clock_timestamp())/60)::bigint-2").execute(&self.pool).await?;
        sqlx::query("DELETE FROM lane_auth_flows WHERE expires_at<=clock_timestamp()")
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM lane_auth_consents WHERE expires_at<=clock_timestamp()")
            .execute(&self.pool)
            .await?;
        if count > self.config.auth_per_minute {
            return Err(Fault::quota());
        }
        Ok(())
    }
}
fn principal(row: &PgRow, tenant: String) -> Principal {
    Principal {
        account_id: row.get("account_id"),
        tenant_id: tenant,
        session_id: row.get("id"),
        device_id: row.get("device_id"),
        device_name: row.get("name"),
        origin_kind: row.get("origin_kind"),
    }
}
fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
fn pkce(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut values = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .filter_map(|s| s.trim().split_once('='))
        .filter(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned());
    let first = values.next()?;
    if values.next().is_some() {
        None
    } else {
        Some(first)
    }
}
fn credential(headers: &HeaderMap) -> Result<(String, &'static str)> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        let token = value
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|v| v.len() == 43)
            .ok_or_else(Fault::unauthenticated)?;
        return Ok((token.into(), "access"));
    }
    Ok((
        cookie(headers, COOKIE)
            .filter(|v| v.len() == 43)
            .ok_or_else(Fault::unauthenticated)?,
        "cookie",
    ))
}
fn stamp(row: &PgRow, key: &str, v: &mut Value, json_key: &str) {
    if let Some(at) = row.get::<Option<DateTime<Utc>>, _>(key) {
        v[json_key] = json!(at.to_rfc3339());
    }
}
fn device(row: &PgRow) -> Value {
    let mut v = json!({"id":row.get::<String,_>("id"),"name":row.get::<String,_>("name")});
    stamp(row, "last_sync_at", &mut v, "lastSyncAt");
    stamp(row, "disconnected_at", &mut v, "disconnectedAt");
    v
}
pub async fn epoch(db: &mut PgConnection, p: &Principal) -> Result<String> {
    Ok(
        sqlx::query_scalar("SELECT epoch FROM lane_accounts WHERE account_id=$1 AND tenant_id=$2")
            .bind(&p.account_id)
            .bind(&p.tenant_id)
            .fetch_one(db)
            .await?,
    )
}
pub async fn get_device(db: &mut PgConnection, p: &Principal) -> Result<Value> {
    Ok(device(
        &sqlx::query("SELECT * FROM lane_devices WHERE account_id=$1 AND id=$2")
            .bind(&p.account_id)
            .bind(&p.device_id)
            .fetch_one(db)
            .await?,
    ))
}
/// DeleteAccountData calls this before deleting any data, in the same transaction.
pub async fn consume_deletion_proof(db: &mut PgConnection, p: &Principal, q: &Value) -> Result<()> {
    let mode = deletion_mode(q)?;
    if string(q, "confirmedAccountId") != p.account_id {
        return Err(Fault::denied());
    }
    let changed=sqlx::query("UPDATE lane_reauth_challenges SET consumed_at=clock_timestamp() WHERE account_id=$1 AND session_id=$2 AND mode=$3 AND proof_hash=$4 AND consumed_at IS NULL AND authenticated_at IS NOT NULL AND proof_expires_at>clock_timestamp()")
        .bind(&p.account_id).bind(&p.session_id).bind(mode).bind(hash(string(q,"reauthProof").as_bytes())).execute(db).await?.rows_affected();
    if changed != 1 {
        return Err(Fault::unauthenticated());
    }
    Ok(())
}
fn deletion_mode(q: &Value) -> Result<&str> {
    match string(q, "mode") {
        v @ ("DATA_DELETION_MODE_DELETE_EVERYWHERE"
        | "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL") => Ok(v),
        _ => Err(Fault::invalid("invalid deletion mode")),
    }
}
/// Revoke all sessions and devices during account clearing. The caller owns deletion markers/epoch.
pub async fn disconnect_all(db: &mut PgConnection, p: &Principal) -> Result<()> {
    sqlx::query("UPDATE lane_sessions SET revoked_at=COALESCE(revoked_at,clock_timestamp()) WHERE account_id=$1").bind(&p.account_id).execute(&mut *db).await?;
    sqlx::query("UPDATE lane_devices SET disconnected_at=COALESCE(disconnected_at,clock_timestamp()) WHERE account_id=$1").bind(&p.account_id).execute(db).await?;
    Ok(())
}

pub async fn dispatch(
    db: &mut PgConnection,
    p: &Principal,
    method: &str,
    q: &Value,
) -> Result<Value> {
    match method {
        "ListDevices" | "ListConnections" => {
            let size = q["page"]["size"].as_u64().unwrap_or(100);
            let size = if size == 0 { 100 } else { size.min(1000) } as i64;
            let cursor = string(&q["page"], "cursor");
            let after = if cursor.is_empty() {
                String::new()
            } else {
                decode_cursor(cursor, &p.account_id, method)?
            };
            let rows = if method == "ListDevices" {
                sqlx::query(
                    "SELECT * FROM lane_devices WHERE account_id=$1 AND id>$2 ORDER BY id LIMIT $3",
                )
                .bind(&p.account_id)
                .bind(&after)
                .bind(size + 1)
                .fetch_all(db)
                .await?
            } else {
                sqlx::query("SELECT s.*,d.name FROM lane_sessions s JOIN lane_devices d ON d.account_id=s.account_id AND d.id=s.device_id WHERE s.account_id=$1 AND s.origin_kind='ORIGIN_KIND_MCP' AND s.id>$2 ORDER BY s.id LIMIT $3").bind(&p.account_id).bind(&after).bind(size+1).fetch_all(db).await?
            };
            let more = rows.len() > size as usize;
            let rows = &rows[..rows.len().min(size as usize)];
            let items: Vec<Value> = rows
                .iter()
                .map(|r| {
                    if method == "ListDevices" {
                        device(r)
                    } else {
                        connection(r)
                    }
                })
                .collect();
            let next = if more {
                encode_cursor(
                    &p.account_id,
                    method,
                    &rows.last().unwrap().get::<String, _>("id"),
                )
            } else {
                String::new()
            };
            Ok(json!({"items":items,"nextCursor":next}))
        }
        "DisconnectDevice" => {
            let device_id = string(q, "deviceId");
            let row=sqlx::query("UPDATE lane_devices SET disconnected_at=COALESCE(disconnected_at,clock_timestamp()) WHERE account_id=$1 AND id=$2 RETURNING *").bind(&p.account_id).bind(device_id).fetch_one(&mut *db).await?;
            sqlx::query("UPDATE lane_sessions SET revoked_at=COALESCE(revoked_at,clock_timestamp()) WHERE account_id=$1 AND device_id=$2").bind(&p.account_id).bind(device_id).execute(db).await?;
            Ok(json!({"device":device(&row)}))
        }
        "RevokeConnection" => {
            let row=sqlx::query("UPDATE lane_sessions s SET revoked_at=COALESCE(revoked_at,clock_timestamp()) FROM lane_devices d WHERE s.account_id=$1 AND s.id=$2 AND s.origin_kind='ORIGIN_KIND_MCP' AND d.account_id=s.account_id AND d.id=s.device_id RETURNING s.*,d.name").bind(&p.account_id).bind(string(q,"connectionId")).fetch_one(db).await?;
            Ok(json!({"connection":connection(&row)}))
        }
        "BeginDataDeletion" => {
            let mode = deletion_mode(q)?;
            if string(q, "confirmedAccountId") != p.account_id {
                return Err(Fault::denied());
            }
            let challenge = id();
            sqlx::query("INSERT INTO lane_reauth_challenges(id,account_id,session_id,mode,expires_at) VALUES($1,$2,$3,$4,clock_timestamp()+interval '10 minutes')").bind(&challenge).bind(&p.account_id).bind(&p.session_id).bind(mode).execute(db).await?;
            let public = server_common::config::required("LANE_PUBLIC_URL")
                .map_err(|_| Fault::unavailable())?;
            Ok(
                json!({"challengeId":challenge,"authorizationUrl":format!("{}/auth/reauth?challenge_id={challenge}",public.trim_end_matches('/'))}),
            )
        }
        _ => Err(Fault::invalid("unknown auth method")),
    }
}
fn connection(row: &PgRow) -> Value {
    let mut v = json!({"id":row.get::<String,_>("id"),"name":row.get::<String,_>("name"),"approvedAt":row.get::<DateTime<Utc>,_>("created_at").to_rfc3339()});
    stamp(row, "revoked_at", &mut v, "revokedAt");
    v
}
fn encode_cursor(account: &str, method: &str, last: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{account}\n{method}\n{last}"))
}
fn decode_cursor(cursor: &str, account: &str, method: &str) -> Result<String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| Fault::invalid("invalid cursor"))?;
    let s = String::from_utf8(bytes).map_err(|_| Fault::invalid("invalid cursor"))?;
    s.strip_prefix(&format!("{account}\n{method}\n"))
        .filter(|v| canonical_id(v))
        .map(str::to_owned)
        .ok_or_else(|| Fault::invalid("cursor belongs to another query"))
}

impl Auth {
    pub async fn dispatch(
        &self,
        db: &mut PgConnection,
        p: &Principal,
        method: &str,
        q: &Value,
    ) -> Result<Value> {
        if method == "DeleteAccountData" {
            consume_deletion_proof(&mut *db, p, q).await?;
            let old = epoch(&mut *db, p).await?;
            crate::clear_account_content(&mut *db, &p.account_id).await?;
            let new_epoch = id();
            sqlx::query("UPDATE lane_accounts SET epoch=$2 WHERE account_id=$1")
                .bind(&p.account_id)
                .bind(&new_epoch)
                .execute(&mut *db)
                .await?;
            sqlx::query("INSERT INTO lane_account_deletions(account_id,old_epoch,new_epoch,mode) VALUES($1,$2,$3,$4)").bind(&p.account_id).bind(old).bind(&new_epoch).bind(deletion_mode(q)?).execute(&mut *db).await?;
            disconnect_all(&mut *db, p).await?;
            // Keep only the identities needed for stale-device deletion directives.
            sqlx::query("UPDATE lane_devices SET name='',last_sync_at=NULL WHERE account_id=$1")
                .bind(&p.account_id)
                .execute(&mut *db)
                .await?;
            sqlx::query("UPDATE lane_sessions SET csrf_hash='',redirect_uri='',client_id='' WHERE account_id=$1").bind(&p.account_id).execute(&mut *db).await?;
            sqlx::query("DELETE FROM lane_reauth_challenges WHERE account_id=$1")
                .bind(&p.account_id)
                .execute(&mut *db)
                .await?;
            sqlx::query("DELETE FROM lane_auth_consents WHERE account_id=$1")
                .bind(&p.account_id)
                .execute(db)
                .await?;
            return Ok(json!({"newEpoch":new_epoch,"disconnected":true}));
        }
        dispatch(db, p, method, q).await
    }
    async fn control_principal(&self, headers: &HeaderMap) -> Result<Principal> {
        let (token, _) = credential(headers)?;
        let resource:Option<String>=sqlx::query_scalar("SELECT s.resource FROM lane_credentials c JOIN lane_sessions s ON s.id=c.session_id WHERE c.digest=$1").bind(hash(token.as_bytes())).fetch_optional(&self.pool).await?;
        let resource = resource.ok_or_else(Fault::unauthenticated)?;
        if resource != self.config.public_url
            && resource != format!("{}/mcp", self.config.public_url)
        {
            return Err(Fault::unauthenticated());
        }
        self.authenticate_resource(headers, &resource).await
    }
}

/// HTTP adapter errors expose categories and a correlation ID, never upstream response bodies.
struct HttpFault(Fault);
impl From<Fault> for HttpFault {
    fn from(value: Fault) -> Self {
        Self(value)
    }
}
impl From<sqlx::Error> for HttpFault {
    fn from(value: sqlx::Error) -> Self {
        Self(value.into())
    }
}
impl IntoResponse for HttpFault {
    fn into_response(self) -> Response {
        use connectrpc::ErrorCode;
        let (status, error) = match self.0.code {
            ErrorCode::Unauthenticated => (StatusCode::UNAUTHORIZED, "invalid_grant"),
            ErrorCode::PermissionDenied => (StatusCode::FORBIDDEN, "access_denied"),
            ErrorCode::ResourceExhausted => {
                (StatusCode::TOO_MANY_REQUESTS, "temporarily_unavailable")
            }
            ErrorCode::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable"),
            _ => (StatusCode::BAD_REQUEST, "invalid_request"),
        };
        let correlation = id();
        tracing::warn!(correlation_id=%correlation,category=error,"Lane authentication request failed");
        (
            status,
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({"error":error,"correlation_id":correlation})),
        )
            .into_response()
    }
}
type HttpResult = std::result::Result<Response, HttpFault>;
fn json_response(value: Value) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response()
}
fn with_cookie(mut response: Response, name: &str, value: &str, max_age: i64) -> Response {
    let cookie =
        format!("{name}={value}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age}");
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("generated cookie"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}
fn redirect_query(base: &str, key: &str, value: &str, state: Option<&str>) -> Result<Response> {
    let mut url = Url::parse(base).map_err(|_| Fault::invalid("invalid redirect URI"))?;
    url.query_pairs_mut().append_pair(key, value);
    if let Some(state) = state {
        url.query_pairs_mut().append_pair("state", state);
    }
    Ok(Redirect::to(url.as_str()).into_response())
}

pub fn router(auth: Arc<Auth>) -> Router {
    Router::new()
        .route("/oauth/authorize", get(oauth_authorize))
        .route("/oauth/token", post(oauth_token))
        .route("/oauth/revoke", post(oauth_revoke))
        .route("/oauth/consent", post(consent))
        .route("/auth/login", get(web_login))
        .route("/auth/callback", get(callback))
        .route("/auth/session", get(web_session))
        .route("/auth/refresh", post(web_refresh))
        .route("/auth/logout", post(web_logout))
        .route("/auth/reauth", get(reauth_start))
        .route("/auth/reauth/proof", post(reauth_proof))
        .route("/auth/deletion-status", get(deletion_status))
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(resource_metadata),
        )
        .layer(DefaultBodyLimit::max(16384))
        .with_state(auth)
}
async fn authorization_metadata(State(a): State<Arc<Auth>>) -> Response {
    let u = &a.config.public_url;
    json_response(
        json!({"issuer":u,"authorization_endpoint":format!("{u}/oauth/authorize"),"token_endpoint":format!("{u}/oauth/token"),"revocation_endpoint":format!("{u}/oauth/revoke"),"response_types_supported":["code"],"grant_types_supported":["authorization_code","refresh_token"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"scopes_supported":["manage"]}),
    )
}
async fn resource_metadata(State(a): State<Arc<Auth>>) -> Response {
    json_response(
        json!({"resource":format!("{}/mcp",a.config.public_url),"authorization_servers":[a.config.public_url],"scopes_supported":["manage"],"bearer_methods_supported":["header"]}),
    )
}

async fn oauth_authorize(
    State(a): State<Arc<Auth>>,
    Query(q): Query<HashMap<String, String>>,
) -> HttpResult {
    a.rate().await?;
    let get = |key: &str| q.get(key).map(String::as_str).unwrap_or("");
    let client = a
        .config
        .clients
        .get(get("client_id"))
        .ok_or_else(|| Fault::invalid("unregistered client"))?;
    if get("response_type") != "code"
        || get("code_challenge_method") != "S256"
        || get("code_challenge").len() != 43
        || URL_SAFE_NO_PAD.decode(get("code_challenge")).is_err()
        || get("state").is_empty()
        || get("state").len() > 1024
        || !client
            .redirect_uris
            .iter()
            .any(|v| v == get("redirect_uri"))
    {
        return Err(Fault::invalid("invalid authorization request").into());
    }
    // An explicit resource is mandatory for MCP. A client cannot select another audience.
    if (client.origin_kind == "ORIGIN_KIND_MCP" || !get("resource").is_empty())
        && get("resource") != client.resource
    {
        return Err(Fault::invalid("invalid resource").into());
    }
    if !get("scope").is_empty() && get("scope") != "manage" {
        return Err(Fault::invalid("invalid scope").into());
    }
    let device_id = if get("device_id").is_empty() {
        id()
    } else {
        get("device_id").into()
    };
    if !canonical_id(&device_id) || get("device_name").len() > 256 {
        return Err(Fault::invalid("invalid device").into());
    }
    let name = if get("device_name").trim().is_empty() {
        client.name.as_str()
    } else {
        get("device_name")
    };
    a.start_flow(json!({"kind":"oauth","client_id":get("client_id"),"redirect_uri":get("redirect_uri"),"state":get("state"),"challenge":get("code_challenge"),"device_id":device_id,"device_name":name,"resource":client.resource,"origin_kind":client.origin_kind}),false).await
}
async fn web_login(
    State(a): State<Arc<Auth>>,
    Query(q): Query<HashMap<String, String>>,
) -> HttpResult {
    a.rate().await?;
    let url = q
        .get("return_to")
        .filter(|v| a.config.return_urls.contains(v))
        .ok_or_else(|| Fault::invalid("unregistered return URL"))?;
    a.start_flow(
        json!({"kind":"web","return_to":url,"device_id":id(),"device_name":"Lane web"}),
        false,
    )
    .await
}
impl Auth {
    async fn start_flow(&self, request: Value, reauth: bool) -> HttpResult {
        let state = random_token();
        let browser = random_token();
        let nonce = random_token();
        let verifier = random_token();
        sqlx::query("INSERT INTO lane_auth_flows(state_hash,browser_hash,nonce,verifier,request,expires_at) VALUES($1,$2,$3,$4,$5,clock_timestamp()+interval '10 minutes')")
            .bind(hash(state.as_bytes())).bind(hash(browser.as_bytes())).bind(&nonce).bind(&verifier).bind(request).execute(&self.pool).await?;
        let mut url = Url::parse(&self.authorization_endpoint).map_err(|_| Fault::unavailable())?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", self.config.logto_client.as_str()),
            (
                "redirect_uri",
                &format!("{}/auth/callback", self.config.public_url),
            ),
            ("response_type", "code"),
            ("scope", "openid"),
            ("resource", self.config.delcore_resource.as_str()),
            ("state", &state),
            ("nonce", &nonce),
            ("code_challenge", &pkce(&verifier)),
            ("code_challenge_method", "S256"),
        ]);
        if reauth {
            url.query_pairs_mut()
                .append_pair("prompt", "login")
                .append_pair("max_age", "0");
        }
        Ok(with_cookie(
            Redirect::to(url.as_str()).into_response(),
            FLOW_COOKIE,
            &browser,
            600,
        ))
    }
    async fn validate_jwt(&self, token: &str, audience: &str) -> Result<Value> {
        let header = decode_header(token).map_err(|_| Fault::unauthenticated())?;
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256) {
            return Err(Fault::unauthenticated());
        }
        let response = self.http.get(&self.jwks_uri).send().await?;
        if !response.status().is_success() {
            return Err(Fault::unavailable());
        }
        let set: JwkSet = serde_json::from_value(bounded_json(response, 256 << 10).await?)
            .map_err(|_| Fault::unavailable())?;
        let key = set
            .find(header.kid.as_deref().ok_or_else(Fault::unauthenticated)?)
            .ok_or_else(Fault::unauthenticated)?;
        let key = DecodingKey::from_jwk(key).map_err(|_| Fault::unavailable())?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.leeway = 0;
        validation.validate_nbf = true;
        Ok(decode::<Value>(token, &key, &validation)
            .map_err(|_| Fault::unauthenticated())?
            .claims)
    }
    async fn issue_session(
        &self,
        db: &mut PgConnection,
        account: &str,
        tenant: &str,
        request: &Value,
    ) -> Result<Principal> {
        let (client_id, origin, resource) = if string(request, "kind") == "web" {
            ("__web", "ORIGIN_KIND_APP", self.config.public_url.as_str())
        } else {
            let key = string(request, "client_id");
            let c = self.config.clients.get(key).ok_or_else(Fault::denied)?;
            (key, c.origin_kind.as_str(), c.resource.as_str())
        };
        sqlx::query("INSERT INTO lane_accounts(account_id,tenant_id,epoch) VALUES($1,$2,$3) ON CONFLICT(account_id) DO NOTHING").bind(account).bind(tenant).bind(id()).execute(&mut *db).await?;
        let row = sqlx::query("SELECT tenant_id,epoch FROM lane_accounts WHERE account_id=$1")
            .bind(account)
            .fetch_one(&mut *db)
            .await?;
        if row.get::<String, _>("tenant_id") != tenant {
            return Err(Fault::denied());
        }
        let device_id = string(request, "device_id");
        sqlx::query("INSERT INTO lane_devices(account_id,id,name) VALUES($1,$2,$3) ON CONFLICT(account_id,id) DO UPDATE SET name=excluded.name,disconnected_at=NULL").bind(account).bind(device_id).bind(string(request,"device_name")).execute(&mut *db).await?;
        let session = id();
        sqlx::query("INSERT INTO lane_sessions(id,account_id,device_id,client_id,origin_kind,resource,issuer,csrf_hash,epoch,redirect_uri) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(&session).bind(account).bind(device_id).bind(client_id).bind(origin).bind(resource).bind(&self.config.public_url).bind(hash(random_token().as_bytes())).bind(row.get::<String,_>("epoch")).bind(if client_id=="__web" {string(request,"return_to")}else{string(request,"redirect_uri")}).execute(db).await?;
        Ok(Principal {
            account_id: account.into(),
            tenant_id: tenant.into(),
            session_id: session,
            device_id: device_id.into(),
            device_name: string(request, "device_name").into(),
            origin_kind: origin.into(),
        })
    }
    async fn credential(
        &self,
        db: &mut PgConnection,
        session: &str,
        kind: &str,
        lifetime: i64,
    ) -> Result<String> {
        let token = random_token();
        sqlx::query("INSERT INTO lane_credentials(digest,session_id,kind,expires_at) VALUES($1,$2,$3,clock_timestamp()+make_interval(secs=>$4::double precision))").bind(hash(token.as_bytes())).bind(session).bind(kind).bind(lifetime as f64).execute(db).await?;
        Ok(token)
    }
    async fn finish_approval(
        &self,
        db: &mut PgConnection,
        account: &str,
        tenant: &str,
        request: &Value,
    ) -> Result<Response> {
        self.validate_approval(request)?;
        self.allow_account(account, tenant).await?;
        let p = self
            .issue_session(&mut *db, account, tenant, request)
            .await?;
        if string(request, "kind") == "web" {
            let token = self
                .credential(db, &p.session_id, "cookie", REFRESH_SECONDS)
                .await?;
            Ok(with_cookie(
                Redirect::to(string(request, "return_to")).into_response(),
                COOKIE,
                &token,
                REFRESH_SECONDS,
            ))
        } else {
            let code = random_token();
            sqlx::query("INSERT INTO lane_auth_codes(digest,session_id,redirect_uri,challenge,expires_at) VALUES($1,$2,$3,$4,clock_timestamp()+interval '60 seconds')").bind(hash(code.as_bytes())).bind(&p.session_id).bind(string(request,"redirect_uri")).bind(string(request,"challenge")).execute(db).await?;
            redirect_query(
                string(request, "redirect_uri"),
                "code",
                &code,
                Some(string(request, "state")),
            )
        }
    }
}

async fn callback(
    State(a): State<Arc<Auth>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> HttpResult {
    a.rate().await?;
    let state = q.get("state").ok_or_else(Fault::unauthenticated)?;
    let browser = cookie(&headers, FLOW_COOKIE).ok_or_else(Fault::unauthenticated)?;
    // Consume state before exchanging the upstream code; failures require a fresh login.
    let flow=sqlx::query("UPDATE lane_auth_flows SET consumed_at=clock_timestamp() WHERE state_hash=$1 AND browser_hash=$2 AND consumed_at IS NULL AND expires_at>clock_timestamp() RETURNING *")
        .bind(hash(state.as_bytes())).bind(hash(browser.as_bytes())).fetch_optional(&a.pool).await?.ok_or_else(Fault::unauthenticated)?;
    let code = q.get("code").ok_or_else(Fault::unauthenticated)?;
    let response = a
        .http
        .post(&a.token_endpoint)
        .basic_auth(&a.config.logto_client, Some(&a.config.logto_secret))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            (
                "redirect_uri",
                &format!("{}/auth/callback", a.config.public_url),
            ),
            ("code_verifier", &flow.get::<String, _>("verifier")),
            ("resource", a.config.delcore_resource.as_str()),
        ])
        .send()
        .await
        .map_err(Fault::from)?;
    if !response.status().is_success() {
        return Err(if response.status().is_server_error() {
            Fault::unavailable()
        } else {
            Fault::unauthenticated()
        }
        .into());
    }
    let tokens = bounded_json(response, 64 << 10).await?;
    let claims = a
        .validate_jwt(string(&tokens, "id_token"), &a.config.logto_client)
        .await?;
    if string(&claims, "nonce") != flow.get::<String, _>("nonce")
        || string(&claims, "sub").is_empty()
        || ((claims.get("azp").is_some() || claims["aud"].as_array().is_some_and(|v| v.len() > 1))
            && string(&claims, "azp") != a.config.logto_client)
    {
        return Err(Fault::unauthenticated().into());
    }
    let access = a
        .validate_jwt(string(&tokens, "access_token"), &a.config.delcore_resource)
        .await?;
    if string(&claims, "sub") != string(&access, "sub") {
        return Err(Fault::unauthenticated().into());
    }
    let (account, tenant) = a.personal_account(string(&tokens, "access_token")).await?;
    let request: Value = flow.get("request");
    if string(&request, "kind") != "reauth" {
        a.validate_approval(&request)?;
    }
    let mut db = a.pool.begin().await?;
    account_lock(&mut db, &account).await?;
    a.allow_account(&account, &tenant).await?;
    let response = if string(&request, "kind") == "reauth" {
        a.registered_redirect(string(&request, "client_id"), string(&request, "return_to"))?;
        let challenge = string(&request, "challenge_id");
        let auth_time = claims["auth_time"]
            .as_i64()
            .ok_or_else(Fault::unauthenticated)?;
        // Compare seconds because OIDC auth_time has whole-second precision. Future values fail.
        let count=sqlx::query("UPDATE lane_reauth_challenges c SET authenticated_at=to_timestamp($3::double precision) FROM lane_sessions s JOIN lane_devices d ON d.account_id=s.account_id AND d.id=s.device_id WHERE c.id=$1 AND c.account_id=$2 AND c.session_id=s.id AND c.consumed_at IS NULL AND c.authenticated_at IS NULL AND c.expires_at>clock_timestamp() AND $3>=ceil(extract(epoch FROM c.created_at))::bigint AND $3<=floor(extract(epoch FROM clock_timestamp()))::bigint AND s.revoked_at IS NULL AND d.disconnected_at IS NULL")
            .bind(challenge).bind(&account).bind(auth_time).execute(&mut *db).await?.rows_affected();
        if count != 1 {
            return Err(Fault::unauthenticated().into());
        }
        redirect_query(
            string(&request, "return_to"),
            "reauth_challenge",
            challenge,
            None,
        )?
    } else if string(&request, "kind") == "oauth" {
        let consent = random_token();
        sqlx::query("INSERT INTO lane_auth_consents(digest,browser_hash,account_id,tenant_id,request,expires_at) VALUES($1,$2,$3,$4,$5,clock_timestamp()+interval '5 minutes')").bind(hash(consent.as_bytes())).bind(hash(browser.as_bytes())).bind(&account).bind(&tenant).bind(&request).execute(&mut *db).await?;
        let client = a
            .config
            .clients
            .get(string(&request, "client_id"))
            .ok_or_else(Fault::denied)?;
        let html = format!(
            "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>Approve Lane connection</title><main><h1>Approve connection</h1><p>{} requests full management access to all repositories, tasks, prompts, history, webhook settings, devices and connections in Lane account {}.</p><p>You can revoke this connection in Lane settings.</p><form method=\"post\" action=\"/oauth/consent\"><input type=\"hidden\" name=\"consent\" value=\"{}\"><button name=\"decision\" value=\"approve\">Approve full account access</button><button name=\"decision\" value=\"deny\">Cancel</button></form></main></html>",
            escape_html(&client.name),
            escape_html(&account),
            consent
        );
        let mut response = Html(html).into_response();
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'none'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
            ),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response.headers_mut().insert(
            header::REFERRER_POLICY,
            // Origin-only: the same-origin consent POST keeps a serializable API Origin
            // (`no-referrer` would make browsers send `Origin: null`), while the callback path
            // and query never appear in Referer.
            HeaderValue::from_static("strict-origin"),
        );
        response
    } else {
        a.finish_approval(&mut db, &account, &tenant, &request)
            .await?
    };
    db.commit().await?;
    Ok(response)
}
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
async fn consent(
    State(a): State<Arc<Auth>>,
    headers: HeaderMap,
    Form(q): Form<HashMap<String, String>>,
) -> HttpResult {
    a.rate().await?;
    let browser = cookie(&headers, FLOW_COOKIE).ok_or_else(Fault::unauthenticated)?;
    let token = q.get("consent").ok_or_else(Fault::unauthenticated)?;
    let mut db = a.pool.begin().await?;
    let row=sqlx::query("UPDATE lane_auth_consents SET consumed_at=clock_timestamp() WHERE digest=$1 AND browser_hash=$2 AND consumed_at IS NULL AND expires_at>clock_timestamp() RETURNING *").bind(hash(token.as_bytes())).bind(hash(browser.as_bytes())).fetch_optional(&mut *db).await?.ok_or_else(Fault::unauthenticated)?;
    let request: Value = row.get("request");
    a.validate_approval(&request)?;
    let account: String = row.get("account_id");
    account_lock(&mut db, &account).await?;
    let response = if q.get("decision").map(String::as_str) == Some("approve") {
        a.finish_approval(
            &mut db,
            &account,
            &row.get::<String, _>("tenant_id"),
            &request,
        )
        .await?
    } else {
        redirect_query(
            string(&request, "redirect_uri"),
            "error",
            "access_denied",
            Some(string(&request, "state")),
        )?
    };
    db.commit().await?;
    Ok(with_cookie(response, FLOW_COOKIE, "", 0))
}
async fn oauth_token(
    State(a): State<Arc<Auth>>,
    Form(q): Form<HashMap<String, String>>,
) -> HttpResult {
    a.rate().await?;
    let get = |key: &str| q.get(key).map(String::as_str).unwrap_or("");
    let client = a
        .config
        .clients
        .get(get("client_id"))
        .ok_or_else(|| Fault::invalid("unregistered client"))?;
    if !get("resource").is_empty() && get("resource") != client.resource {
        return Err(Fault::invalid("invalid resource").into());
    }
    let refresh = get("grant_type") == "refresh_token";
    if !refresh && get("grant_type") != "authorization_code" {
        return Err(Fault::invalid("unsupported grant type").into());
    }
    let digest = hash(
        if refresh {
            get("refresh_token")
        } else {
            get("code")
        }
        .as_bytes(),
    );
    let account: Option<String> = if refresh {
        sqlx::query_scalar("SELECT s.account_id FROM lane_credentials c JOIN lane_sessions s ON s.id=c.session_id WHERE c.digest=$1 AND c.kind='refresh'").bind(&digest).fetch_optional(&a.pool).await?
    } else {
        sqlx::query_scalar("SELECT s.account_id FROM lane_auth_codes c JOIN lane_sessions s ON s.id=c.session_id WHERE c.digest=$1").bind(&digest).fetch_optional(&a.pool).await?
    };
    let account = account.ok_or_else(Fault::unauthenticated)?;
    let mut db = a.pool.begin().await?;
    account_lock(&mut db, &account).await?;
    let row = if refresh {
        sqlx::query("SELECT c.session_id,c.consumed_at,c.expires_at,s.client_id FROM lane_credentials c JOIN lane_sessions s ON s.id=c.session_id WHERE c.digest=$1 AND c.kind='refresh'").bind(&digest).fetch_one(&mut *db).await?
    } else {
        sqlx::query("SELECT c.*,s.client_id FROM lane_auth_codes c JOIN lane_sessions s ON s.id=c.session_id WHERE c.digest=$1").bind(&digest).fetch_one(&mut *db).await?
    };
    if row.get::<String, _>("client_id") != get("client_id") {
        return Err(Fault::unauthenticated().into());
    }
    let session: String = row.get("session_id");
    let binding =
        sqlx::query("SELECT redirect_uri,resource,origin_kind FROM lane_sessions WHERE id=$1")
            .bind(&session)
            .fetch_one(&mut *db)
            .await?;
    a.registered_redirect(get("client_id"), &binding.get::<String, _>("redirect_uri"))?;
    if binding.get::<String, _>("resource") != client.resource
        || binding.get::<String, _>("origin_kind") != client.origin_kind
    {
        return Err(Fault::denied().into());
    }

    if row.get::<Option<DateTime<Utc>>, _>("consumed_at").is_some() {
        // Refresh replay revokes the family, including the replacement credential.
        if refresh {
            sqlx::query("UPDATE lane_sessions SET revoked_at=COALESCE(revoked_at,clock_timestamp()) WHERE id=$1").bind(&session).execute(&mut *db).await?;
            db.commit().await?;
        }
        return Err(Fault::unauthenticated().into());
    }
    let valid: bool = sqlx::query_scalar("SELECT $1::timestamptz>clock_timestamp()")
        .bind(row.get::<DateTime<Utc>, _>("expires_at"))
        .fetch_one(&mut *db)
        .await?;
    if !valid {
        return Err(Fault::unauthenticated().into());
    }
    if !refresh
        && (row.get::<String, _>("redirect_uri") != get("redirect_uri")
            || !valid_verifier(get("code_verifier"))
            || row.get::<String, _>("challenge") != pkce(get("code_verifier")))
    {
        return Err(Fault::unauthenticated().into());
    }
    let p = session_principal(&mut db, &session).await?;
    a.authorize(&mut db, &p).await?;
    let query = if refresh {
        "UPDATE lane_credentials SET consumed_at=clock_timestamp() WHERE digest=$1"
    } else {
        "UPDATE lane_auth_codes SET consumed_at=clock_timestamp() WHERE digest=$1"
    };
    sqlx::query(query).bind(&digest).execute(&mut *db).await?;
    let access = a
        .credential(&mut db, &session, "access", ACCESS_SECONDS)
        .await?;
    let refresh = a
        .credential(&mut db, &session, "refresh", REFRESH_SECONDS)
        .await?;
    db.commit().await?;
    let mut value = identity(&p, &a.config.public_url);
    value["access_token"] = json!(access);
    value["refresh_token"] = json!(refresh);
    value["expires_in"] = json!(ACCESS_SECONDS);
    value["token_type"] = json!("Bearer");
    Ok(json_response(value))
}
fn valid_verifier(v: &str) -> bool {
    (43..=128).contains(&v.len())
        && v.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._~".contains(&c))
}
async fn session_principal(db: &mut PgConnection, session: &str) -> Result<Principal> {
    let row=sqlx::query("SELECT s.*,d.name,a.tenant_id FROM lane_sessions s JOIN lane_devices d ON d.account_id=s.account_id AND d.id=s.device_id JOIN lane_accounts a ON a.account_id=s.account_id WHERE s.id=$1").bind(session).fetch_one(db).await?;
    Ok(principal(&row, row.get("tenant_id")))
}
fn identity(p: &Principal, issuer: &str) -> Value {
    json!({"issuer":issuer,"account_id":p.account_id,"tenant_id":p.tenant_id,"device_id":p.device_id,"session_id":p.session_id})
}
async fn oauth_revoke(
    State(a): State<Arc<Auth>>,
    Form(q): Form<HashMap<String, String>>,
) -> HttpResult {
    a.rate().await?;
    let token = q
        .get("token")
        .ok_or_else(|| Fault::invalid("token required"))?;
    let client = q
        .get("client_id")
        .ok_or_else(|| Fault::invalid("client required"))?;
    let row=sqlx::query("SELECT s.id,s.account_id FROM lane_credentials c JOIN lane_sessions s ON s.id=c.session_id WHERE c.digest=$1 AND s.client_id=$2 AND c.kind IN ('access','refresh')").bind(hash(token.as_bytes())).bind(client).fetch_optional(&a.pool).await?;
    if let Some(row) = row {
        let mut db = a.pool.begin().await?;
        account_lock(&mut db, &row.get::<String, _>("account_id")).await?;
        sqlx::query("UPDATE lane_sessions SET revoked_at=COALESCE(revoked_at,clock_timestamp()) WHERE id=$1").bind(row.get::<String,_>("id")).execute(&mut *db).await?;
        db.commit().await?;
    }
    Ok(StatusCode::OK.into_response())
}
async fn web_session(State(a): State<Arc<Auth>>, headers: HeaderMap) -> HttpResult {
    a.rate().await?;
    let p = a.authenticate(&headers).await?;
    let mut db = a.pool.begin().await?;
    account_lock(&mut db, &p.account_id).await?;
    a.authorize(&mut db, &p).await?;
    // The readable CSRF token is delivered only through configured credentialed CORS origins.
    let (session_token, _) = credential(&headers)?;
    // All tabs sharing this cookie get the same CSRF value until cookie rotation.
    let csrf = hash(format!("lane-csrf:{session_token}").as_bytes());
    sqlx::query("UPDATE lane_sessions SET csrf_hash=$2 WHERE id=$1")
        .bind(&p.session_id)
        .bind(hash(csrf.as_bytes()))
        .execute(&mut *db)
        .await?;
    let preferences: Option<(bool, i16)> = sqlx::query_as(
        "SELECT enabled,content FROM lane_push_registrations WHERE account_id=$1 AND device_id=$2",
    )
    .bind(&p.account_id)
    .bind(&p.device_id)
    .fetch_optional(&mut *db)
    .await?;
    db.commit().await?;
    let mut v = identity(&p, &a.config.public_url);
    v["csrf_token"] = json!(csrf);
    let (enabled, content) = preferences.unwrap_or((false, 1));
    v["notification_preferences"] = json!({"enabled":enabled,"content":content});
    Ok(json_response(v))
}
async fn web_refresh(State(a): State<Arc<Auth>>, headers: HeaderMap) -> HttpResult {
    a.rate().await?;
    let p = a.authenticate(&headers).await?;
    a.csrf(&headers, &p).await?;
    let (old, kind) = credential(&headers)?;
    if kind != "cookie" {
        return Err(Fault::invalid("web cookie required").into());
    }
    let mut db = a.pool.begin().await?;
    account_lock(&mut db, &p.account_id).await?;
    a.authorize(&mut db, &p).await?;
    let changed=sqlx::query("UPDATE lane_credentials SET consumed_at=clock_timestamp() WHERE digest=$1 AND consumed_at IS NULL AND expires_at>clock_timestamp()").bind(hash(old.as_bytes())).execute(&mut *db).await?.rows_affected();
    if changed != 1 {
        return Err(Fault::unauthenticated().into());
    }
    let token = a
        .credential(&mut db, &p.session_id, "cookie", REFRESH_SECONDS)
        .await?;
    db.commit().await?;
    Ok(with_cookie(
        json_response(identity(&p, &a.config.public_url)),
        COOKIE,
        &token,
        REFRESH_SECONDS,
    ))
}
async fn web_logout(State(a): State<Arc<Auth>>, headers: HeaderMap) -> HttpResult {
    a.rate().await?;
    let p = a.authenticate(&headers).await?;
    a.csrf(&headers, &p).await?;
    let mut db = a.pool.begin().await?;
    account_lock(&mut db, &p.account_id).await?;
    sqlx::query("UPDATE lane_sessions SET revoked_at=COALESCE(revoked_at,clock_timestamp()) WHERE id=$1 AND account_id=$2").bind(&p.session_id).bind(&p.account_id).execute(&mut *db).await?;
    db.commit().await?;
    Ok(with_cookie(
        StatusCode::NO_CONTENT.into_response(),
        COOKIE,
        "",
        0,
    ))
}
async fn reauth_start(
    State(a): State<Arc<Auth>>,
    Query(q): Query<HashMap<String, String>>,
) -> HttpResult {
    a.rate().await?;
    let challenge = q.get("challenge_id").ok_or_else(Fault::unauthenticated)?;
    let row=sqlx::query("SELECT s.redirect_uri,s.client_id FROM lane_reauth_challenges c JOIN lane_sessions s ON s.id=c.session_id WHERE c.id=$1 AND c.expires_at>clock_timestamp() AND c.consumed_at IS NULL AND c.authenticated_at IS NULL AND s.revoked_at IS NULL").bind(challenge).fetch_optional(&a.pool).await?.ok_or_else(Fault::unauthenticated)?;
    let return_to: String = row.get("redirect_uri");
    a.registered_redirect(&row.get::<String, _>("client_id"), &return_to)?;
    a.start_flow(
        json!({"kind":"reauth","challenge_id":challenge,"return_to":return_to,"client_id":row.get::<String,_>("client_id")}),
        true,
    )
    .await
}
async fn reauth_proof(
    State(a): State<Arc<Auth>>,
    headers: HeaderMap,
    Json(q): Json<Value>,
) -> HttpResult {
    a.rate().await?;
    let p = a.control_principal(&headers).await?;
    a.csrf(&headers, &p).await?;
    let mut db = a.pool.begin().await?;
    account_lock(&mut db, &p.account_id).await?;
    a.authorize(&mut db, &p).await?;
    let proof = random_token();
    let changed=sqlx::query("UPDATE lane_reauth_challenges SET proof_hash=$4,proof_expires_at=LEAST(expires_at,authenticated_at+interval '5 minutes') WHERE id=$1 AND account_id=$2 AND session_id=$3 AND authenticated_at IS NOT NULL AND authenticated_at+interval '5 minutes'>clock_timestamp() AND expires_at>clock_timestamp() AND proof_hash IS NULL AND consumed_at IS NULL")
        .bind(string(&q,"challenge_id")).bind(&p.account_id).bind(&p.session_id).bind(hash(proof.as_bytes())).execute(&mut *db).await?.rows_affected();
    if changed != 1 {
        return Err(Fault::unauthenticated().into());
    }
    db.commit().await?;
    Ok(json_response(json!({"reauth_proof":proof})))
}
async fn deletion_status(State(a): State<Arc<Auth>>, headers: HeaderMap) -> HttpResult {
    a.rate().await?;
    let (token, kind) = credential(&headers)?;
    // This route deliberately permits revoked, expired credentials only for a minimal deletion
    // directive. It cannot authenticate a principal or return any retained account content.
    let row=sqlx::query("SELECT d.mode,d.old_epoch,d.new_epoch,s.account_id,s.device_id FROM lane_credentials c JOIN lane_sessions s ON s.id=c.session_id JOIN lane_account_deletions d ON d.account_id=s.account_id AND d.old_epoch=s.epoch WHERE c.digest=$1 AND c.kind=$2 AND s.issuer=$3").bind(hash(token.as_bytes())).bind(kind).bind(&a.config.public_url).fetch_optional(&a.pool).await?.ok_or_else(Fault::unauthenticated)?;
    Ok(json_response(
        json!({"mode":row.get::<String,_>("mode"),"old_epoch":row.get::<String,_>("old_epoch"),"new_epoch":row.get::<String,_>("new_epoch"),"account_id":row.get::<String,_>("account_id"),"device_id":row.get::<String,_>("device_id"),"disconnected":true}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use connectrpc::ErrorCode;
    use std::sync::Mutex;

    #[test]
    fn pkce_and_account_scoped_cursor_vectors() {
        assert_eq!(
            pkce("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert!(valid_verifier(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        ));
        assert!(!valid_verifier(&"!".repeat(43)));
        let account = id();
        let last = id();
        let cursor = encode_cursor(&account, "ListDevices", &last);
        assert_eq!(
            decode_cursor(&cursor, &account, "ListDevices").unwrap(),
            last
        );
        assert!(decode_cursor(&cursor, &id(), "ListDevices").is_err());
        assert!(decode_cursor(&cursor, &account, "ListConnections").is_err());
    }
    #[test]
    fn cookies_and_bearers_cannot_be_confused() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{COOKIE}={}", "a".repeat(43))).unwrap(),
        );
        assert_eq!(credential(&h).unwrap().1, "cookie");
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer invalid"),
        );
        assert!(credential(&h).is_err());
        h.remove(header::AUTHORIZATION);
        h.append(
            header::COOKIE,
            HeaderValue::from_str(&format!("{COOKIE}={}", "b".repeat(43))).unwrap(),
        );
        assert!(credential(&h).is_err());
        assert_eq!(
            escape_html("<script>'&\""),
            "&lt;script&gt;&#39;&amp;&quot;"
        );
    }

    // Public test fixture only; this key never signs production credentials.
    const TEST_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQglAnLdw1E7sKU4Od0
KUR6w73q0kPwV2ZuIrCYCc5O+1+hRANCAAS7WL2yuRlkebV9eg5DC8EgeL5sLCc+
SWa6SCqGYyxWx3gNrhl069h8P2zMhX5WKLrOwiNGwCHc0xVxIsdq2G49
-----END PRIVATE KEY-----
"#;
    fn test_jwks() -> Value {
        json!({"keys":[{"kty":"EC","crv":"P-256","alg":"ES256","use":"sig","kid":"fixture","x":"u1i9srkZZHm1fXoOQwvBIHi-bCwnPklmukgqhmMsVsc","y":"eA2uGXTr2Hw_bMyFflYous7CI0bAIdzTFXEix2rYbj0"}]})
    }
    fn sign_claims(claims: Value) -> String {
        let mut h = jsonwebtoken::Header::new(Algorithm::ES256);
        h.kid = Some("fixture".into());
        jsonwebtoken::encode(
            &h,
            &claims,
            &jsonwebtoken::EncodingKey::from_ec_pem(TEST_KEY.as_bytes()).unwrap(),
        )
        .unwrap()
    }
    struct Mock {
        pages: Mutex<Vec<Value>>,
        calls: Mutex<Vec<(String, String, Value)>>,
        allowed: Mutex<bool>,
        state: Mutex<String>,
        fail_m2m: Mutex<bool>,
        oidc_tokens: Mutex<Value>,
    }
    async fn mock_authority() -> (Authority, Arc<Mock>, String) {
        let mock = Arc::new(Mock {
            pages: Mutex::new(vec![]),
            calls: Mutex::new(vec![]),
            allowed: Mutex::new(true),
            state: Mutex::new("TENANT_STATE_ACTIVE".into()),
            fail_m2m: Mutex::new(false),
            oidc_tokens: Mutex::new(json!({})),
        });
        let m = mock.clone();
        let app = Router::new().route(
            "/token",
            post(move |Form(form): Form<HashMap<String, String>>| {
                let m = m.clone();
                async move {
                    if form.get("grant_type").map(String::as_str) == Some("authorization_code") {
                        return (StatusCode::OK, Json(m.oidc_tokens.lock().unwrap().clone()));
                    }
                    if *m.fail_m2m.lock().unwrap() {
                        (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({"error":"invalid_client"})),
                        )
                    } else {
                        (
                            StatusCode::OK,
                            Json(json!({"access_token":"service-only","expires_in":0})),
                        )
                    }
                }
            }),
        );
        let m = mock.clone();
        let app = app.route("/jwks", get(|| async { Json(test_jwks()) }));
        let app=app.route("/delcore.v1.DelcoreService/{method}",post(move |axum::extract::Path(method):axum::extract::Path<String>,headers:HeaderMap,Json(q):Json<Value>| {let m=m.clone();async move {
            m.calls.lock().unwrap().push((method.clone(),headers[header::AUTHORIZATION].to_str().unwrap().into(),q.clone()));
            let v=match method.as_str() {
                "GetCurrentUser"=>m.pages.lock().unwrap().remove(0),
                "Authorize"=>json!({"allowed":*m.allowed.lock().unwrap()}),
                "GetTenantStates"=>json!({"states":[{"tenantId":q["tenantIds"][0],"state":m.state.lock().unwrap().clone()}]}),
                _=>panic!("unexpected mock method"),
            };Json(v)
        }}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (
            Authority::new(
                reqwest::Client::new(),
                url.clone(),
                format!("{url}/token"),
                "delcore".into(),
                "lane".into(),
                "fixture".into(),
            ),
            mock,
            url,
        )
    }
    fn fixture_auth(pool: PgPool, authority: Authority, url: &str) -> Auth {
        Auth {
            pool,
            authority,
            http: reqwest::Client::new(),
            authorization_endpoint: format!("{url}/authorize"),
            token_endpoint: format!("{url}/token"),
            jwks_uri: format!("{url}/jwks"),
            config: Config {
                public_url: "https://lane.api.example.test".into(),
                issuer: url.into(),
                logto_client: "browser".into(),
                logto_secret: "fixture".into(),
                delcore_resource: "delcore".into(),
                return_urls: vec!["https://lane.example.test/callback".into()],
                clients: HashMap::from([(
                    "lane-cli".into(),
                    Client {
                        name: "Lane CLI".into(),
                        redirect_uris: vec!["http://127.0.0.1:46326/callback".into()],
                        origin_kind: "ORIGIN_KIND_CLI".into(),
                        resource: "https://lane.api.example.test".into(),
                    },
                )]),
                auth_per_minute: 1000,
            },
        }
    }
    #[tokio::test]
    async fn personal_tenant_uses_all_pages_and_service_tokens() {
        let (authority, m, url) = mock_authority().await;
        let a = fixture_auth(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://localhost/unused")
                .unwrap(),
            authority,
            &url,
        );
        let user = id();
        let org = id();
        let tenant = id();
        *m.pages.lock().unwrap() = vec![
            json!({"userId":user,"tenants":[{"id":org,"ownerId":user,"personal":false}],"nextAfterId":"second"}),
            json!({"userId":user,"tenants":[{"id":tenant,"ownerId":user,"personal":true}]}),
        ];
        assert_eq!(
            a.personal_account("delcore-user-token").await.unwrap(),
            (user.clone(), tenant.clone())
        );
        a.allow_account(&user, &tenant).await.unwrap();
        {
            let calls = m.calls.lock().unwrap();
            assert_eq!(calls[1].2["page"]["afterId"], "second");
            assert_eq!(calls[0].1, "Bearer delcore-user-token");
            assert_eq!(calls[2].1, "Bearer service-only");
            assert_eq!(
                calls[2].2,
                json!({"tenantId":tenant,"userId":user,"capability":"manage","paid":false})
            );
        }
        for state in [
            "TENANT_STATE_BLOCKED",
            "TENANT_STATE_DELETING",
            "TENANT_STATE_DELETED",
            "TENANT_STATE_UNKNOWN",
        ] {
            *m.state.lock().unwrap() = state.into();
            assert_eq!(
                a.allow_account(&user, &tenant).await.unwrap_err().code,
                ErrorCode::PermissionDenied
            );
        }
        *m.fail_m2m.lock().unwrap() = true;
        assert_eq!(
            a.allow_account(&user, &tenant).await.unwrap_err().code,
            ErrorCode::Unavailable
        );
        *m.pages.lock().unwrap() = vec![
            json!({"userId":user,"tenants":[{"id":tenant,"ownerId":user,"personal":true},{"id":id(),"ownerId":user,"personal":true}]}),
        ];
        assert_eq!(
            a.personal_account("user").await.unwrap_err().code,
            ErrorCode::PermissionDenied
        );
    }

    #[tokio::test]
    async fn pending_approval_rechecks_exact_registration() {
        let (authority, _, url) = mock_authority().await;
        let mut a = fixture_auth(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://localhost/unused")
                .unwrap(),
            authority,
            &url,
        );
        let request = json!({"kind":"oauth","client_id":"lane-cli","redirect_uri":"http://127.0.0.1:46326/callback","resource":"https://lane.api.example.test","origin_kind":"ORIGIN_KIND_CLI"});
        assert!(a.validate_approval(&request).is_ok());
        let mut other = request.clone();
        other["redirect_uri"] = json!("http://127.0.0.1:46326/callback/");
        assert!(a.validate_approval(&other).is_err());
        a.config.clients.get_mut("lane-cli").unwrap().origin_kind = "ORIGIN_KIND_MCP".into();
        assert!(a.validate_approval(&request).is_err());
        a.config.clients.remove("lane-cli");
        assert!(a.validate_approval(&request).is_err());
        assert!(
            a.validate_approval(
                &json!({"kind":"web","return_to":"https://lane.example.test/callback?next=evil"})
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn oidc_issuer_audience_expiry_and_signature_are_verified() {
        let (authority, _, url) = mock_authority().await;
        let a = fixture_auth(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://localhost/unused")
                .unwrap(),
            authority,
            &url,
        );
        let valid = json!({"iss":url,"aud":"browser","sub":"logto-subject","exp":Utc::now().timestamp()+60});
        assert!(
            a.validate_jwt(&sign_claims(valid.clone()), "browser")
                .await
                .is_ok()
        );
        for (key, value) in [
            ("iss", json!("https://wrong.example")),
            ("aud", json!("mcp")),
            ("exp", json!(Utc::now().timestamp() - 1)),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            assert_eq!(
                a.validate_jwt(&sign_claims(invalid), "browser")
                    .await
                    .unwrap_err()
                    .code,
                ErrorCode::Unauthenticated
            );
        }
        let token = sign_claims(valid);
        let mut pieces: Vec<&str> = token.split('.').collect();
        pieces[2] = "AAAAAAAA";
        assert!(a.validate_jwt(&pieces.join("."), "browser").await.is_err());
    }

    async fn reauth_callback_fixture(
        a: Arc<Auth>,
        m: &Mock,
        challenge: &str,
        account: &str,
        tenant: &str,
        auth_time: i64,
    ) -> HttpResult {
        let start = reauth_start(
            State(a.clone()),
            Query(HashMap::from([("challenge_id".into(), challenge.into())])),
        )
        .await?;
        let location = Url::parse(start.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let query: HashMap<String, String> = location
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(query["prompt"], "login");
        assert_eq!(query["max_age"], "0");
        let flow_cookie = start.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let claims = json!({"iss":a.config.issuer,"aud":"browser","sub":"logto-subject","exp":Utc::now().timestamp()+60,"nonce":query["nonce"],"auth_time":auth_time});
        *m.oidc_tokens.lock().unwrap() = json!({"id_token":sign_claims(claims),"access_token":sign_claims(json!({"iss":a.config.issuer,"aud":"delcore","sub":"logto-subject","exp":Utc::now().timestamp()+60}))});
        *m.pages.lock().unwrap() = vec![
            json!({"userId":account,"tenants":[{"id":tenant,"ownerId":account,"personal":true}]}),
        ];
        let mut browser = HeaderMap::new();
        browser.insert(header::COOKIE, HeaderValue::from_str(&flow_cookie).unwrap());
        callback(
            State(a),
            browser,
            Query(HashMap::from([
                ("state".into(), query["state"].clone()),
                ("code".into(), "logto-code".into()),
            ])),
        )
        .await
    }

    #[tokio::test]
    #[ignore = "requires disposable TEST_DATABASE_URL; uses an isolated schema and mocked Delcore"]
    async fn postgres_auth_binding_rotation_revocation_and_proof_atomicity() {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("lane_auth_{}", id().replace('-', ""));
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .after_connect(move |c, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {search}"))
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql(concat!(
            include_str!("../migrations/00001_auth.sql"),
            "\n",
            include_str!("../migrations/00004_push.sql")
        ))
        .execute(&pool)
        .await
        .unwrap();
        let (authority, m, mock_url) = mock_authority().await;
        let a = Arc::new(fixture_auth(pool.clone(), authority, &mock_url));
        let account = id();
        let tenant = id();
        let device_id = id();
        let verifier = random_token();
        let request = json!({"kind":"oauth","client_id":"lane-cli","device_id":device_id,"device_name":"Fixture CLI","redirect_uri":"http://127.0.0.1:46326/callback","challenge":pkce(&verifier),"state":"client-state","resource":"https://lane.api.example.test","origin_kind":"ORIGIN_KIND_CLI"});
        let start = a
            .start_flow(request.clone(), false)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        let location = Url::parse(start.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let query: HashMap<String, String> = location
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(query["resource"], "delcore");
        let flow_cookie = start.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let claims = json!({"iss":mock_url,"aud":"browser","sub":"logto-subject","exp":Utc::now().timestamp()+60,"nonce":query["nonce"]});
        *m.oidc_tokens.lock().unwrap() = json!({"id_token":sign_claims(claims),"access_token":sign_claims(json!({"iss":mock_url,"aud":"delcore","sub":"logto-subject","exp":Utc::now().timestamp()+60}))});
        *m.pages.lock().unwrap() = vec![
            json!({"userId":account,"tenants":[{"id":tenant,"ownerId":account,"personal":true}]}),
        ];
        let mut browser_headers = HeaderMap::new();
        browser_headers.insert(header::COOKIE, HeaderValue::from_str(&flow_cookie).unwrap());
        let callback_query = HashMap::from([
            ("state".into(), query["state"].clone()),
            ("code".into(), "logto-code".into()),
        ]);
        let response = callback(
            State(a.clone()),
            browser_headers.clone(),
            Query(callback_query.clone()),
        )
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
        assert!(
            callback(
                State(a.clone()),
                browser_headers.clone(),
                Query(callback_query)
            )
            .await
            .is_err()
        );
        let html = String::from_utf8(
            axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("Approve full account access"));
        let consent_code = html
            .split("name=\"consent\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let response = consent(
            State(a.clone()),
            browser_headers,
            Form(HashMap::from([
                ("consent".into(), consent_code.into()),
                ("decision".into(), "approve".into()),
            ])),
        )
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
        let redirect = Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let code = redirect
            .query_pairs()
            .find(|(k, _)| k == "code")
            .unwrap()
            .1
            .into_owned();
        let form = HashMap::from([
            ("grant_type".into(), "authorization_code".into()),
            ("client_id".into(), "lane-cli".into()),
            (
                "redirect_uri".into(),
                "http://127.0.0.1:46326/callback".into(),
            ),
            ("code".into(), code),
            ("code_verifier".into(), verifier),
        ]);
        let mut wrong = form.clone();
        wrong.insert("code_verifier".into(), random_token());
        assert!(oauth_token(State(a.clone()), Form(wrong)).await.is_err());
        let token = oauth_token(State(a.clone()), Form(form.clone()))
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        let token: Value = serde_json::from_slice(
            &axum::body::to_bytes(token.into_body(), 65536)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(token["device_id"], device_id);
        assert_eq!(token["account_id"], account);
        assert!(oauth_token(State(a.clone()), Form(form)).await.is_err());
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", string(&token, "access_token"))).unwrap(),
        );
        let p = a.authenticate(&headers).await.unwrap();
        assert!(
            a.authenticate_resource(&headers, "https://lane.api.example.test/mcp")
                .await
                .is_err()
        );
        let mut db = pool.begin().await.unwrap();
        account_lock(&mut db, &account).await.unwrap();
        a.authorize(&mut db, &p).await.unwrap();
        let challenge = id();
        sqlx::query("INSERT INTO lane_reauth_challenges(id,account_id,session_id,mode,created_at,expires_at) VALUES($1,$2,$3,'DATA_DELETION_MODE_DELETE_EVERYWHERE',clock_timestamp()-interval '3 seconds',clock_timestamp()+interval '5 minutes')").bind(&challenge).bind(&account).bind(&p.session_id).execute(&mut *db).await.unwrap();
        db.commit().await.unwrap();
        assert!(
            reauth_callback_fixture(
                a.clone(),
                &m,
                &challenge,
                &account,
                &tenant,
                Utc::now().timestamp() - 60
            )
            .await
            .is_err()
        );
        assert!(
            reauth_callback_fixture(
                a.clone(),
                &m,
                &challenge,
                &id(),
                &tenant,
                Utc::now().timestamp()
            )
            .await
            .is_err()
        );
        let response = reauth_callback_fixture(
            a.clone(),
            &m,
            &challenge,
            &account,
            &tenant,
            Utc::now().timestamp(),
        )
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
        assert!(
            response.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .starts_with("http://127.0.0.1:46326/callback?reauth_challenge=")
        );
        let response = reauth_proof(
            State(a.clone()),
            headers.clone(),
            Json(json!({"challenge_id":challenge})),
        )
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
        let value: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap(),
        )
        .unwrap();
        let proof = string(&value, "reauth_proof");
        assert!(
            reauth_proof(
                State(a.clone()),
                headers.clone(),
                Json(json!({"challenge_id":challenge}))
            )
            .await
            .is_err()
        );
        let q = json!({"confirmedAccountId":account,"mode":"DATA_DELETION_MODE_DELETE_EVERYWHERE","reauthProof":proof});
        let mut db = pool.begin().await.unwrap();
        account_lock(&mut db, &account).await.unwrap();
        consume_deletion_proof(&mut db, &p, &q).await.unwrap();
        db.rollback().await.unwrap();
        let mut db = pool.begin().await.unwrap();
        account_lock(&mut db, &account).await.unwrap();
        let mut wrong = q.clone();
        wrong["mode"] = json!("DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL");
        assert!(consume_deletion_proof(&mut db, &p, &wrong).await.is_err());
        consume_deletion_proof(&mut db, &p, &q).await.unwrap();
        assert!(consume_deletion_proof(&mut db, &p, &q).await.is_err());
        db.commit().await.unwrap();
        let mut db = pool.begin().await.unwrap();
        account_lock(&mut db, &account).await.unwrap();
        let web=a.finish_approval(&mut db,&account,&tenant,&json!({"kind":"web","return_to":"https://lane.example.test/callback","device_id":id(),"device_name":"Web"})).await.unwrap();
        db.commit().await.unwrap();
        let mut wh = HeaderMap::new();
        wh.insert(
            header::COOKIE,
            HeaderValue::from_str(
                web.headers()[header::SET_COOKIE]
                    .to_str()
                    .unwrap()
                    .split(';')
                    .next()
                    .unwrap(),
            )
            .unwrap(),
        );
        let session = web_session(State(a.clone()), wh.clone())
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        let value: Value = serde_json::from_slice(
            &axum::body::to_bytes(session.into_body(), 65536)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(web_refresh(State(a.clone()), wh.clone()).await.is_err());
        wh.insert(
            "x-lane-csrf",
            HeaderValue::from_str(string(&value, "csrf_token")).unwrap(),
        );
        wh.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://attacker.example"),
        );
        assert!(web_refresh(State(a.clone()), wh.clone()).await.is_err());
        wh.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://lane.example.test"),
        );
        assert!(web_refresh(State(a.clone()), wh.clone()).await.is_ok());
        assert!(a.authenticate(&wh).await.is_err());
        let refresh = HashMap::from([
            ("grant_type".into(), "refresh_token".into()),
            ("client_id".into(), "lane-cli".into()),
            (
                "refresh_token".into(),
                string(&token, "refresh_token").into(),
            ),
        ]);
        assert!(
            oauth_token(State(a.clone()), Form(refresh.clone()))
                .await
                .is_ok()
        );
        assert!(oauth_token(State(a.clone()), Form(refresh)).await.is_err());
        assert!(a.authenticate(&headers).await.is_err());
        let mut db = pool.begin().await.unwrap();
        account_lock(&mut db, &account).await.unwrap();
        assert!(a.authorize(&mut db, &p).await.is_err());
        db.rollback().await.unwrap();
        assert!(
            deletion_status(State(a.clone()), headers.clone())
                .await
                .is_err()
        );
        let old_epoch: String = sqlx::query_scalar("SELECT epoch FROM lane_sessions WHERE id=$1")
            .bind(&p.session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let new_epoch = id();
        sqlx::query("INSERT INTO lane_account_deletions(account_id,old_epoch,new_epoch,mode) VALUES($1,$2,$3,'DATA_DELETION_MODE_DELETE_EVERYWHERE')")
            .bind(&account).bind(&old_epoch).bind(&new_epoch).execute(&pool).await.unwrap();
        let response = deletion_status(State(a.clone()), headers.clone())
            .await
            .unwrap_or_else(|_| panic!("expected deletion directive"));
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let directive: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            directive,
            json!({"mode":"DATA_DELETION_MODE_DELETE_EVERYWHERE","old_epoch":old_epoch,"new_epoch":new_epoch,"account_id":account,"device_id":device_id,"disconnected":true})
        );
        assert!(a.authenticate(&headers).await.is_err());
        assert!(!m.calls.lock().unwrap().is_empty());
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }

    // ---- Assembled-router Origin policy and API-hosted consent (issue: consent through Origin policy) ----
    const API_ORIGIN: &str = "https://lane.api.example.test";
    const WEB_ORIGIN: &str = "https://lane.example.test";
    const CLI_REDIRECT: &str = "http://127.0.0.1:46326/callback";

    /// The complete production router, built from the fixture Auth with distinct API and web origins.
    fn assembled_router(pool: PgPool, auth: Auth) -> Router {
        let server = Arc::new(crate::Server {
            db: pool.clone(),
            auth: Arc::new(auth),
            config: Arc::new(crate::config::Config {
                listen: "127.0.0.1:0".into(),
                public_url: API_ORIGIN.into(),
                web_origin: WEB_ORIGIN.into(),
                metrics_token: "synthetic-metrics-token-at-least-32-characters".into(),
                max_item_bytes: 4096,
                max_webhook_bytes: 4096,
                requests_per_minute: 1000,
                webhook_requests_per_minute: 1000,
            }),
            metrics: Arc::default(),
            push: Arc::new(crate::push::Push::from_env(pool).unwrap()),
            webhooks: Arc::new(
                crate::webhooks::WebhookConfig::new(
                    &URL_SAFE_NO_PAD.encode([7; 32]),
                    "https://lane.api.example.test/hooks",
                    1024,
                )
                .unwrap(),
            ),
        });
        crate::http::router(server)
    }
    fn http_request(
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Option<String>,
    ) -> axum::http::Request<axum::body::Body> {
        let mut builder = axum::http::Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder
            .body(axum::body::Body::from(body.unwrap_or_default()))
            .unwrap()
    }
    async fn send(app: &Router, request: axum::http::Request<axum::body::Body>) -> Response {
        use tower::ServiceExt;
        app.clone().oneshot(request).await.unwrap()
    }
    async fn text(response: Response) -> String {
        String::from_utf8(
            axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn router_origin_policy_keeps_api_origin_out_of_every_route_but_consent_post() {
        let (authority, _, url) = mock_authority().await;
        // The pool is never used: every request below is answered by the Origin policy itself.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap();
        let app = assembled_router(pool.clone(), fixture_auth(pool, authority, &url));
        let api = [("origin", API_ORIGIN)];
        for (method, uri) in [
            ("POST", "/lane.v1.LaneService/ListTasks"),
            ("POST", "/lane.v1.SyncService/GetSyncInfo"),
            ("POST", "/mcp"),
            ("OPTIONS", "/mcp"),
            ("POST", "/auth/refresh"),
            ("POST", "/auth/logout"),
            ("POST", "/oauth/token"),
            ("POST", "/oauth/revoke"),
            ("GET", "/auth/session"),
            ("GET", "/health/live"),
            // The exception is exact in method and path.
            ("GET", "/oauth/consent"),
            ("OPTIONS", "/oauth/consent"),
            ("PUT", "/oauth/consent"),
            ("POST", "/oauth/consent/"),
            ("POST", "/oauth/consent/extra"),
        ] {
            let response = send(&app, http_request(method, uri, &api, None)).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
            assert!(
                response
                    .headers()
                    .get("access-control-allow-origin")
                    .is_none(),
                "{method} {uri}"
            );
        }
        // Hostile and opaque origins are rejected everywhere, including the consent POST.
        for origin in [
            "https://attacker.example",
            "null",
            "https://lane.api.example.test.attacker.example",
        ] {
            for uri in ["/oauth/consent", "/mcp", "/auth/refresh"] {
                let response =
                    send(&app, http_request("POST", uri, &[("origin", origin)], None)).await;
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin} {uri}");
            }
        }
        // Configured frontend CORS stays credentialed for the web origin only.
        let response = send(
            &app,
            http_request("OPTIONS", "/oauth/consent", &[("origin", WEB_ORIGIN)], None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            WEB_ORIGIN
        );
        assert_eq!(
            response.headers()["access-control-allow-credentials"],
            "true"
        );
        let response = send(
            &app,
            http_request("GET", "/health/live", &[("origin", WEB_ORIGIN)], None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            WEB_ORIGIN
        );
        // Absent-Origin clients keep their previous behavior.
        let response = send(&app, http_request("GET", "/health/live", &[], None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            WEB_ORIGIN
        );
        let response = send(&app, http_request("OPTIONS", "/mcp", &[], None)).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    /// Minimal browser fixture for the rendered consent page. It parses the form, derives the
    /// `Origin` and `Referer` headers from the page's Referrer-Policy as Fetch specifies for a
    /// same-origin form POST navigation, and submits the pressed button. It is not a real browser.
    struct ConsentPage {
        url: String,
        policy: String,
        csp: String,
        html: String,
        cookie: String,
    }
    impl ConsentPage {
        fn tag_attribute(tag: &str, name: &str) -> Option<String> {
            Some(
                tag.split(&format!("{name}=\""))
                    .nth(1)?
                    .split('"')
                    .next()?
                    .to_owned(),
            )
        }
        fn hidden_consent_token(&self) -> String {
            self.html
                .split('<')
                .filter(|tag| tag.starts_with("input "))
                .find(|tag| Self::tag_attribute(tag, "name").as_deref() == Some("consent"))
                .and_then(|tag| Self::tag_attribute(tag, "value"))
                .expect("hidden consent input")
        }
        fn submit_with(
            &self,
            decision: &str,
            cookie: Option<&str>,
            token: Option<&str>,
        ) -> axum::http::Request<axum::body::Body> {
            let tags: Vec<&str> = self.html.split('<').collect();
            let form = tags
                .iter()
                .find(|tag| tag.starts_with("form "))
                .expect("form");
            assert_eq!(Self::tag_attribute(form, "method").as_deref(), Some("post"));
            let action = Self::tag_attribute(form, "action").unwrap();
            assert!(
                tags.iter().filter(|tag| tag.starts_with("button ")).any(
                    |tag| Self::tag_attribute(tag, "name").as_deref() == Some("decision")
                        && Self::tag_attribute(tag, "value").as_deref() == Some(decision)
                ),
                "the rendered page must contain a {decision} button"
            );
            let mut body = url::form_urlencoded::Serializer::new(String::new());
            if let Some(token) = token {
                body.append_pair("consent", token);
            }
            body.append_pair("decision", decision);
            let page = Url::parse(&self.url).unwrap();
            let origin = page.origin().ascii_serialization();
            let (origin_header, referer) = match self.policy.as_str() {
                // Fetch serializes the Origin of a non-GET navigation as `null` for no-referrer.
                "no-referrer" => ("null".to_owned(), None),
                "strict-origin" | "origin" | "same-origin" | "strict-origin-when-cross-origin" => {
                    // Origin-only policies strip path and query from Referer.
                    (origin.clone(), Some(format!("{origin}/")))
                }
                other => panic!("unsupported fixture referrer policy {other}"),
            };
            let mut builder = axum::http::Request::builder()
                .method("POST")
                .uri(page.join(&action).unwrap().path())
                .header("origin", origin_header)
                .header("content-type", "application/x-www-form-urlencoded");
            if let Some(referer) = referer {
                builder = builder.header("referer", referer);
            }
            if let Some(cookie) = cookie {
                builder = builder.header("cookie", cookie);
            }
            builder.body(axum::body::Body::from(body.finish())).unwrap()
        }
        fn submit(&self, decision: &str) -> axum::http::Request<axum::body::Body> {
            self.submit_with(
                decision,
                Some(&self.cookie),
                Some(&self.hidden_consent_token()),
            )
        }
    }

    /// Authorize, log in through the mocked identity provider and render the consent page.
    async fn consent_page(
        app: &Router,
        m: &Mock,
        issuer: &str,
        account: &str,
        tenant: &str,
        verifier: &str,
        state: &str,
    ) -> ConsentPage {
        let authorize = format!(
            "/oauth/authorize?{}",
            url::form_urlencoded::Serializer::new(String::new())
                .append_pair("client_id", "lane-cli")
                .append_pair("response_type", "code")
                .append_pair("code_challenge", &pkce(verifier))
                .append_pair("code_challenge_method", "S256")
                .append_pair("state", state)
                .append_pair("redirect_uri", CLI_REDIRECT)
                .finish()
        );
        let start = send(app, http_request("GET", &authorize, &[], None)).await;
        assert!(start.status().is_redirection(), "{}", start.status());
        let cookie = start.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let location = Url::parse(start.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let query: HashMap<String, String> = location
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let claims = json!({"iss":issuer,"aud":"browser","sub":"logto-subject","exp":Utc::now().timestamp()+60,"nonce":query["nonce"]});
        *m.oidc_tokens.lock().unwrap() = json!({"id_token":sign_claims(claims),"access_token":sign_claims(json!({"iss":issuer,"aud":"delcore","sub":"logto-subject","exp":Utc::now().timestamp()+60}))});
        *m.pages.lock().unwrap() = vec![
            json!({"userId":account,"tenants":[{"id":tenant,"ownerId":account,"personal":true}]}),
        ];
        let callback_uri = format!(
            "/auth/callback?{}",
            url::form_urlencoded::Serializer::new(String::new())
                .append_pair("state", &query["state"])
                .append_pair("code", "logto-code")
                .finish()
        );
        let page = send(
            app,
            http_request("GET", &callback_uri, &[("cookie", &cookie)], None),
        )
        .await;
        assert_eq!(page.status(), StatusCode::OK);
        let header_text =
            |name: header::HeaderName| page.headers()[name].to_str().unwrap().to_owned();
        let policy = header_text(header::REFERRER_POLICY);
        let csp = header_text(header::CONTENT_SECURITY_POLICY);
        ConsentPage {
            url: format!("{API_ORIGIN}{callback_uri}"),
            policy,
            csp,
            html: text(page).await,
            cookie,
        }
    }
    async fn grant_counts(pool: &PgPool) -> (i64, i64, i64) {
        let count = |sql: &'static str| async move {
            sqlx::query_scalar::<_, i64>(sql)
                .fetch_one(pool)
                .await
                .unwrap()
        };
        (
            count("SELECT count(*) FROM lane_auth_codes").await,
            count("SELECT count(*) FROM lane_sessions").await,
            count("SELECT count(*) FROM lane_auth_consents WHERE consumed_at IS NULL").await,
        )
    }

    #[tokio::test]
    #[ignore = "requires disposable TEST_DATABASE_URL; uses an isolated schema and mocked identity/authorization providers"]
    async fn postgres_assembled_router_completes_api_hosted_consent_with_distinct_origins() {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("lane_consent_{}", id().replace('-', ""));
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .after_connect(move |c, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {search}"))
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql(concat!(
            include_str!("../migrations/00001_auth.sql"),
            "\n",
            include_str!("../migrations/00004_push.sql")
        ))
        .execute(&pool)
        .await
        .unwrap();
        let (authority, m, issuer) = mock_authority().await;
        let app = assembled_router(pool.clone(), fixture_auth(pool.clone(), authority, &issuer));
        let (account, tenant) = (id(), id());

        // Approve: hostile and opaque origins and a wrong browser binding never consume the token.
        let verifier = random_token();
        let page = consent_page(
            &app,
            &m,
            &issuer,
            &account,
            &tenant,
            &verifier,
            "approve-state",
        )
        .await;
        // The page restricts form submission and exposes only its origin through Referer.
        assert_eq!(page.policy, "strict-origin");
        assert!(page.csp.contains("form-action 'self'"), "{}", page.csp);
        assert!(page.url.contains("code=logto-code"));
        let token = page.hidden_consent_token();
        let before = grant_counts(&pool).await;
        assert_eq!(before, (0, 0, 1));
        for origin in ["https://attacker.example", "null"] {
            let mut request = page.submit("approve");
            request
                .headers_mut()
                .insert("origin", HeaderValue::from_static(origin));
            assert_eq!(
                send(&app, request).await.status(),
                StatusCode::FORBIDDEN,
                "{origin}"
            );
            assert_eq!(
                grant_counts(&pool).await,
                before,
                "{origin} must not consume consent"
            );
        }
        // The fixture derives the emitted Origin and Referer from the page headers.
        let request = page.submit("approve");
        assert_eq!(request.headers()["origin"], API_ORIGIN);
        assert_eq!(request.headers()["referer"], format!("{API_ORIGIN}/"));
        for secret in ["code=", "state=", "/auth/callback", "logto-code"] {
            assert!(
                !request.headers()["referer"]
                    .to_str()
                    .unwrap()
                    .contains(secret)
            );
        }
        // Binding failures at the permitted Origin fail without a grant or consumption.
        for (label, cookie, consent) in [
            ("missing cookie", None, Some(token.as_str())),
            (
                "wrong cookie",
                Some("__Host-lane_flow=wrong"),
                Some(token.as_str()),
            ),
            ("missing token", Some(page.cookie.as_str()), None),
            ("wrong token", Some(page.cookie.as_str()), Some("wrong")),
        ] {
            let response = send(&app, page.submit_with("approve", cookie, consent)).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{label}");
            assert_eq!(grant_counts(&pool).await, before, "{label}");
        }
        let response = send(&app, page.submit("approve")).await;
        assert!(response.status().is_redirection(), "{}", response.status());
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none()
        );
        let redirect = Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        assert!(redirect.as_str().starts_with(CLI_REDIRECT));
        let query: HashMap<String, String> = redirect
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(query["state"], "approve-state");
        assert_eq!(grant_counts(&pool).await, (1, 1, 0));
        // Replay of the consumed token fails and issues nothing.
        let replay = send(&app, page.submit("approve")).await;
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(grant_counts(&pool).await, (1, 1, 0));
        // The code redeems once with the original verifier only.
        let exchange = |verifier: String| {
            http_request(
                "POST",
                "/oauth/token",
                &[("content-type", "application/x-www-form-urlencoded")],
                Some(
                    url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("grant_type", "authorization_code")
                        .append_pair("client_id", "lane-cli")
                        .append_pair("redirect_uri", CLI_REDIRECT)
                        .append_pair("code", &query["code"])
                        .append_pair("code_verifier", &verifier)
                        .finish(),
                ),
            )
        };
        assert!(
            !send(&app, exchange(random_token()))
                .await
                .status()
                .is_success()
        );
        let redeemed = send(&app, exchange(verifier.clone())).await;
        assert_eq!(redeemed.status(), StatusCode::OK);
        assert!(!send(&app, exchange(verifier)).await.status().is_success());

        // Cancel: the registered redirect carries access_denied and the original state; no grant.
        let page = consent_page(
            &app,
            &m,
            &issuer,
            &account,
            &tenant,
            &random_token(),
            "deny-state",
        )
        .await;
        let before = grant_counts(&pool).await;
        assert_eq!(before, (1, 1, 1));
        let response = send(&app, page.submit("deny")).await;
        assert!(response.status().is_redirection(), "{}", response.status());
        let redirect = Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        assert!(redirect.as_str().starts_with(CLI_REDIRECT));
        let query: HashMap<String, String> = redirect
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(query["error"], "access_denied");
        assert_eq!(query["state"], "deny-state");
        assert!(!query.contains_key("code"));
        assert_eq!(grant_counts(&pool).await, (1, 1, 0));

        // Expired consent fails without a grant.
        let page = consent_page(
            &app,
            &m,
            &issuer,
            &account,
            &tenant,
            &random_token(),
            "expired-state",
        )
        .await;
        sqlx::query("UPDATE lane_auth_consents SET expires_at=clock_timestamp()-interval '1 second' WHERE consumed_at IS NULL")
            .execute(&pool)
            .await
            .unwrap();
        let response = send(&app, page.submit("approve")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(grant_counts(&pool).await, (1, 1, 0));

        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}
