// SPDX-License-Identifier: FSL-1.1-ALv2
//! Direct push providers and durable, account-serialized delivery.
//!
//! Domain entry points use the caller's authorized account transaction. The worker
//! holds the same advisory lock at session scope on a detached connection: attempt
//! reservations survive a crash, while acceptance and revocation cannot overtake
//! a dispatch. The connection is closed on every exit, including cancellation.
use crate::{Fault, Principal, Result, hash};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row};
use std::{sync::Arc, time::SystemTime};
use web_push::{ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushMessageBuilder};

pub const RETRY_SECONDS: [i64; 5] = [30, 120, 600, 1800, 3600];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Apns = 1,
    Fcm = 2,
    WebPush = 3,
}
impl Provider {
    fn parse(value: &Value) -> Result<Self> {
        match value.as_str() {
            Some("PUSH_PROVIDER_APNS") => Ok(Self::Apns),
            Some("PUSH_PROVIDER_FCM") => Ok(Self::Fcm),
            Some("PUSH_PROVIDER_WEB_PUSH") => Ok(Self::WebPush),
            _ => match value.as_i64() {
                Some(1) => Ok(Self::Apns),
                Some(2) => Ok(Self::Fcm),
                Some(3) => Ok(Self::WebPush),
                _ => Err(Fault::invalid("unsupported push provider")),
            },
        }
    }
}

/// Pure policy shared by dispatch and fake-clock tests. `attempt` includes the initial send.
pub fn next_retry(
    attempt: usize,
    failed_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    retry_after: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    let delay = *RETRY_SECONDS.get(attempt.checked_sub(1)?)?;
    let next = (failed_at + Duration::seconds(delay)).max(retry_after.unwrap_or(failed_at));
    (next < expires_at).then_some(next)
}
pub fn retry_after(value: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return Some(
            value
                .parse::<i64>()
                .ok()
                .and_then(Duration::try_seconds)
                .and_then(|delta| now.checked_add_signed(delta))
                .unwrap_or(DateTime::<Utc>::MAX_UTC),
        );
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(DateTime::<Utc>::from)
}

fn is_mcp(p: &Principal) -> bool {
    matches!(p.origin_kind.as_str(), "MCP" | "ORIGIN_KIND_MCP")
}
fn text<'a>(q: &'a Value, name: &str) -> Result<&'a str> {
    q.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Fault::invalid(format!("missing {name}")))
}
fn content(q: &Value) -> Result<i16> {
    match q.get("content") {
        None | Some(Value::Null) => Ok(1),
        Some(v)
            if matches!(
                v.as_str(),
                Some("NOTIFICATION_CONTENT_UNSPECIFIED" | "NOTIFICATION_CONTENT_DETAIL")
            ) || matches!(v.as_i64(), Some(0 | 1)) =>
        {
            Ok(1)
        }
        Some(v) if v.as_str() == Some("NOTIFICATION_CONTENT_GENERIC") || v.as_i64() == Some(2) => {
            Ok(2)
        }
        _ => Err(Fault::invalid("unsupported notification content")),
    }
}
fn web_endpoint(endpoint: &str) -> Result<url::Url> {
    let url = url::Url::parse(endpoint).map_err(|_| Fault::invalid("invalid push endpoint"))?;
    // A caller controls its subscription. Restrict network destinations to known
    // push services; HTTPS alone would permit SSRF through DNS or local addresses.
    let allowed = matches!(
        url.host_str(),
        Some("fcm.googleapis.com" | "updates.push.services.mozilla.com" | "web.push.apple.com")
    );
    if url.scheme() != "https"
        || !allowed
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Fault::invalid("unsupported web push endpoint"));
    }
    Ok(url)
}
fn registration(q: &Value) -> Result<(Provider, String, String, String, bool, i16)> {
    let provider = Provider::parse(&q["provider"])?;
    let enabled = q.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let endpoint = q
        .get("endpoint")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut key = String::new();
    let mut auth = String::new();
    if enabled {
        if endpoint.is_empty() || endpoint.len() > 4096 {
            return Err(Fault::invalid("invalid push endpoint"));
        }
        match provider {
            Provider::Apns if !endpoint.bytes().all(|b| b.is_ascii_hexdigit()) => {
                return Err(Fault::invalid("invalid APNs device token"));
            }
            Provider::Fcm
                if endpoint
                    .bytes()
                    .any(|b| b.is_ascii_control() || b.is_ascii_whitespace()) =>
            {
                return Err(Fault::invalid("invalid FCM device token"));
            }
            Provider::WebPush => {
                web_endpoint(&endpoint)?;
                let decode = |name| -> Result<Vec<u8>> {
                    let value = text(q, name)?;
                    STANDARD
                        .decode(value)
                        .or_else(|_| URL_SAFE_NO_PAD.decode(value))
                        .map_err(|_| Fault::invalid("invalid subscription key"))
                };
                let k = decode("publicKey")?;
                let a = decode("authSecret")?;
                if k.len() != 65 || k[0] != 4 || a.len() != 16 {
                    return Err(Fault::invalid("invalid subscription key"));
                }
                key = URL_SAFE_NO_PAD.encode(k);
                auth = URL_SAFE_NO_PAD.encode(a);
            }
            _ => (),
        }
    }
    Ok((provider, endpoint, key, auth, enabled, content(q)?))
}

pub async fn register(db: &mut PgConnection, p: &Principal, q: &Value) -> Result<Value> {
    if text(q, "deviceId")? != p.device_id {
        return Err(Fault::denied());
    }
    let row = sqlx::query("SELECT name,last_sync_at FROM lane_devices WHERE account_id=$1 AND id=$2 AND disconnected_at IS NULL")
        .bind(&p.account_id).bind(&p.device_id).fetch_optional(&mut *db).await?.ok_or_else(Fault::denied)?;
    let (provider, endpoint, key, auth, enabled, content) = registration(q)?;
    sqlx::query("INSERT INTO lane_push_registrations(account_id,device_id,provider,endpoint,public_key,auth_secret,enabled,content) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(account_id,device_id) DO UPDATE SET provider=$3,endpoint=$4,public_key=$5,auth_secret=$6,enabled=$7,content=$8")
        .bind(&p.account_id).bind(&p.device_id).bind(provider as i16).bind(endpoint).bind(key).bind(auth).bind(enabled).bind(content).execute(&mut *db).await?;
    if !enabled {
        sqlx::query("UPDATE lane_push_deliveries SET state='cancelled',repository_name='',title='' WHERE account_id=$1 AND device_id=$2 AND state='pending'")
            .bind(&p.account_id).bind(&p.device_id).execute(&mut *db).await?;
    }
    Ok(
        json!({"device":{"id":p.device_id,"name":row.get::<String,_>("name"),"lastSyncAt":row.get::<Option<DateTime<Utc>>,_>("last_sync_at")}}),
    )
}

/// Call only on first creation of a PENDING proposal, in the creating transaction.
pub async fn enqueue(
    db: &mut PgConnection,
    p: &Principal,
    proposal_id: &str,
    repository_name: &str,
    title: &str,
) -> Result<()> {
    if !is_mcp(p) {
        return Ok(());
    }
    sqlx::query("INSERT INTO lane_push_deliveries(account_id,proposal_id,device_id,repository_name,title) SELECT r.account_id,$2,r.device_id,$4,$5 FROM lane_push_registrations r JOIN lane_devices d ON d.account_id=r.account_id AND d.id=r.device_id WHERE r.account_id=$1 AND r.device_id<>$3 AND r.enabled AND d.disconnected_at IS NULL ON CONFLICT DO NOTHING")
        .bind(&p.account_id).bind(proposal_id).bind(&p.device_id).bind(repository_name).bind(title).execute(db).await?;
    Ok(())
}
pub async fn cancel(db: &mut PgConnection, account: &str, proposal_id: &str) -> Result<()> {
    sqlx::query("UPDATE lane_push_deliveries SET state='cancelled',repository_name='',title='' WHERE account_id=$1 AND proposal_id=$2 AND state='pending'")
        .bind(account).bind(proposal_id).execute(db).await?;
    Ok(())
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
struct WallClock;
impl Clock for WallClock {
    fn now(&self) -> DateTime<Utc> {
        SystemTime::now().into()
    }
}

#[derive(Clone)]
struct Apns {
    key: Arc<EncodingKey>,
    key_id: String,
    team_id: String,
    topic: String,
    endpoint: String,
}
#[derive(Clone)]
struct Fcm {
    key: Arc<EncodingKey>,
    email: String,
    project: String,
    token_endpoint: String,
    endpoint: String,
}
#[derive(Clone)]
struct WebPush {
    key: String,
    subject: String,
}
#[derive(Clone, Default)]
struct Providers {
    apns: Option<Apns>,
    fcm: Option<Fcm>,
    web: Option<WebPush>,
}
#[derive(Clone)]
pub struct Push {
    db: PgPool,
    http: reqwest::Client,
    providers: Providers,
    clock: Arc<dyn Clock>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Success,
    TransientFailure,
    PermanentFailure,
}
impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::TransientFailure => "transient_failure",
            Self::PermanentFailure => "permanent_failure",
        }
    }
}
struct Delivery {
    account: String,
    proposal: String,
    device: String,
    repository: String,
    title: String,
    expires: DateTime<Utc>,
    provider: Provider,
    endpoint: String,
    key: String,
    auth: String,
    generic: bool,
}
impl Delivery {
    fn collapse(&self) -> String {
        hash(format!("{}:{}:{}", self.account, self.proposal, self.device).as_bytes())
    }
    fn payload(&self) -> Value {
        let (title, body) = if self.generic {
            (
                "Lane".to_owned(),
                "A new proposal is ready to review.".to_owned(),
            )
        } else {
            (self.repository.clone(), self.title.clone())
        };
        json!({"title":title,"body":body,"proposalId":self.proposal,"tag":self.collapse()})
    }
}
fn env(name: &str) -> Result<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Fault::precondition(format!("missing {name}")))
}
fn enabled(name: &str) -> Result<bool> {
    match std::env::var(name).as_deref() {
        Ok("true") => Ok(true),
        Ok("false") | Err(std::env::VarError::NotPresent) => Ok(false),
        _ => Err(Fault::precondition(format!("{name} must be true or false"))),
    }
}
impl Push {
    /// Uncompressed P-256 public key for browser subscription registration.
    pub fn public_key(&self) -> Option<String> {
        let config = self.providers.web.as_ref()?;
        let key = VapidSignatureBuilder::from_pem_no_sub(config.key.as_bytes()).ok()?;
        Some(URL_SAFE_NO_PAD.encode(key.get_public_key()))
    }
    pub fn from_env(db: PgPool) -> Result<Self> {
        let mut providers = Providers::default();
        if enabled("LANE_APNS_ENABLED")? {
            providers.apns = Some(Apns {
                key: Arc::new(
                    EncodingKey::from_ec_pem(env("LANE_APNS_PRIVATE_KEY")?.as_bytes())
                        .map_err(|_| Fault::precondition("invalid APNs key"))?,
                ),
                key_id: env("LANE_APNS_KEY_ID")?,
                team_id: env("LANE_APNS_TEAM_ID")?,
                topic: env("LANE_APNS_TOPIC")?,
                endpoint: if enabled("LANE_APNS_SANDBOX")? {
                    "https://api.sandbox.push.apple.com"
                } else {
                    "https://api.push.apple.com"
                }
                .to_owned(),
            });
        }
        if enabled("LANE_FCM_ENABLED")? {
            providers.fcm = Some(Fcm {
                key: Arc::new(
                    EncodingKey::from_rsa_pem(env("LANE_FCM_PRIVATE_KEY")?.as_bytes())
                        .map_err(|_| Fault::precondition("invalid FCM key"))?,
                ),
                email: env("LANE_FCM_CLIENT_EMAIL")?,
                project: env("LANE_FCM_PROJECT_ID")?,
                token_endpoint: "https://oauth2.googleapis.com/token".into(),
                endpoint: "https://fcm.googleapis.com".into(),
            });
        }
        if enabled("LANE_WEB_PUSH_ENABLED")? {
            let key = env("LANE_WEB_PUSH_PRIVATE_KEY")?;
            VapidSignatureBuilder::from_pem_no_sub(key.as_bytes())
                .map_err(|_| Fault::precondition("invalid VAPID key"))?;
            let subject = env("LANE_WEB_PUSH_SUBJECT")?;
            let parsed = url::Url::parse(&subject)
                .map_err(|_| Fault::precondition("invalid VAPID subject"))?;
            if !matches!(parsed.scheme(), "mailto" | "https") {
                return Err(Fault::precondition("invalid VAPID subject"));
            }
            providers.web = Some(WebPush { key, subject });
        }
        tracing::info!(
            apns = providers.apns.is_some(),
            fcm = providers.fcm.is_some(),
            web_push = providers.web.is_some(),
            "push providers configured"
        );
        Ok(Self {
            db,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .build()?,
            providers,
            clock: Arc::new(WallClock),
        })
    }
    fn available(&self, provider: Provider) -> bool {
        match provider {
            Provider::Apns => self.providers.apns.is_some(),
            Provider::Fcm => self.providers.fcm.is_some(),
            Provider::WebPush => self.providers.web.is_some(),
        }
    }

    async fn dispatch(&self, d: &Delivery) -> (Outcome, Option<DateTime<Utc>>) {
        let now = self.clock.now();
        if now >= d.expires {
            return (Outcome::PermanentFailure, None);
        }
        match self.request(d, now).await {
            Ok(response) => {
                let status = response.status();
                let after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| retry_after(v, self.clock.now()));
                let result = if status.is_success() {
                    Outcome::Success
                } else if status.is_server_error()
                    || status.as_u16() == 429
                    || status.as_u16() == 408
                {
                    Outcome::TransientFailure
                } else {
                    Outcome::PermanentFailure
                };
                (result, after)
            }
            Err(error)
                if error.code == connectrpc::ErrorCode::InvalidArgument
                    || error.code == connectrpc::ErrorCode::FailedPrecondition =>
            {
                (Outcome::PermanentFailure, None)
            }
            Err(_) => (Outcome::TransientFailure, None),
        }
    }
    async fn request(&self, d: &Delivery, now: DateTime<Utc>) -> Result<reqwest::Response> {
        let request = self.prepare(d, now).await?;
        if self.clock.now() >= d.expires {
            return Err(Fault::precondition("push expired"));
        }
        Ok(request.send().await?)
    }
    async fn prepare(&self, d: &Delivery, now: DateTime<Utc>) -> Result<reqwest::RequestBuilder> {
        let payload = d.payload();
        let ttl = (d.expires - now).num_seconds().max(0);
        let request = match d.provider {
            Provider::Apns => {
                let c = self
                    .providers
                    .apns
                    .as_ref()
                    .ok_or_else(Fault::unavailable)?;
                let mut header = Header::new(Algorithm::ES256);
                header.kid = Some(c.key_id.clone());
                let jwt = jsonwebtoken::encode(
                    &header,
                    &json!({"iss":c.team_id,"iat":now.timestamp()}),
                    &c.key,
                )
                .map_err(|_| Fault::unavailable())?;
                self.http.post(format!("{}/3/device/{}",c.endpoint,d.endpoint))
                    .version(reqwest::Version::HTTP_2).bearer_auth(jwt)
                    .header("apns-topic",&c.topic).header("apns-push-type","background").header("apns-priority","5")
                    .header("apns-expiration",d.expires.timestamp().to_string()).header("apns-collapse-id",d.collapse())
                    .json(&json!({"aps":{"content-available":1},"type":"lane.proposal","proposalId":d.proposal,"accountId":d.account}))
            }
            Provider::Fcm => {
                let c = self.providers.fcm.as_ref().ok_or_else(Fault::unavailable)?;
                let jwt = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &json!({"iss":c.email,"scope":"https://www.googleapis.com/auth/firebase.messaging","aud":c.token_endpoint,"iat":now.timestamp(),"exp":now.timestamp()+3600}), &c.key).map_err(|_| Fault::unavailable())?;
                let token_response = self
                    .http
                    .post(&c.token_endpoint)
                    .form(&[
                        ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                        ("assertion", jwt.as_str()),
                    ])
                    .send()
                    .await?;
                if !token_response.status().is_success() {
                    return Err(Fault::unavailable());
                }
                let token: Value = token_response.json().await?;
                // OAuth may take time. Never dispatch a notification after its deadline.
                if self.clock.now() >= d.expires {
                    return Err(Fault::precondition("push expired"));
                }
                let ttl = (d.expires - self.clock.now()).num_seconds().max(0);
                self.http.post(format!("{}/v1/projects/{}/messages:send",c.endpoint,c.project))
                    .bearer_auth(text(&token,"access_token")?)
                    .json(&json!({"message":{"token":d.endpoint,"data":{"proposalId":d.proposal,"type":"lane.proposal","accountId":d.account},"android":{"ttl":format!("{ttl}s"),"collapse_key":d.collapse(),"priority":"HIGH"}}}))
            }
            Provider::WebPush => {
                let c = self.providers.web.as_ref().ok_or_else(Fault::unavailable)?;
                web_endpoint(&d.endpoint)?;
                let subscription =
                    SubscriptionInfo::new(d.endpoint.clone(), d.key.clone(), d.auth.clone());
                let mut signature =
                    VapidSignatureBuilder::from_pem(c.key.as_bytes(), &subscription)
                        .map_err(|_| Fault::invalid("invalid VAPID configuration"))?;
                signature.add_claim("sub", c.subject.clone());
                let signature = signature
                    .build()
                    .map_err(|_| Fault::invalid("invalid subscription"))?;
                let bytes = serde_json::to_vec(&payload)?;
                let mut message = WebPushMessageBuilder::new(&subscription);
                message.set_ttl(ttl as u32);
                message.set_topic(d.collapse()[..32].to_owned());
                message.set_vapid_signature(signature);
                message.set_payload(ContentEncoding::Aes128Gcm, &bytes);
                let message = message
                    .build()
                    .map_err(|_| Fault::invalid("invalid subscription"))?;
                let encrypted = message.payload.ok_or_else(Fault::unavailable)?;
                let mut request = self
                    .http
                    .post(&d.endpoint)
                    .header("ttl", ttl.to_string())
                    .header("topic", d.collapse()[..32].to_owned())
                    .header("content-encoding", encrypted.content_encoding.to_str())
                    .header("content-type", "application/octet-stream");
                for (key, value) in encrypted.crypto_headers {
                    request = request.header(key, value);
                }
                request.body(encrypted.content)
            }
        };
        Ok(request)
    }

    /// Process a bounded batch. The host schedules subsequent ticks.
    pub async fn tick(&self) -> Result<usize> {
        let rows = sqlx::query("SELECT account_id,proposal_id,device_id FROM lane_push_deliveries WHERE state='pending' AND (next_attempt_at<=$1 OR expires_at<=$1) ORDER BY next_attempt_at LIMIT 100")
            .bind(self.clock.now()).fetch_all(&self.db).await?;
        let mut sent = 0;
        for row in rows {
            let account: String = row.get("account_id");
            // Detaching is essential: never return a session advisory lock to the pool.
            let mut db = self.db.acquire().await?.detach();
            let locked: bool =
                sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1,0))")
                    .bind(&account)
                    .fetch_one(&mut db)
                    .await?;
            if !locked {
                db.close().await?;
                continue;
            }
            let result = self
                .deliver(
                    &mut db,
                    &account,
                    row.get("proposal_id"),
                    row.get("device_id"),
                )
                .await;
            let closed = db.close().await;
            sent += usize::from(result?);
            closed?;
        }
        Ok(sent)
    }

    async fn deliver(
        &self,
        db: &mut PgConnection,
        account: &str,
        proposal: &str,
        device: &str,
    ) -> Result<bool> {
        let Some(row) = sqlx::query("SELECT * FROM lane_push_deliveries WHERE account_id=$1 AND proposal_id=$2 AND device_id=$3 AND state='pending'")
            .bind(account).bind(proposal).bind(device).fetch_optional(&mut *db).await? else { return Ok(false); };
        let expires: DateTime<Utc> = row.get("expires_at");
        if self.clock.now() >= expires {
            self.terminal(db, account, proposal, device, "expired")
                .await?;
            return Ok(false);
        }
        // Session lock serializes this read and send with all domain mutations.
        if !live_proposal(db, account, proposal).await? {
            self.terminal(db, account, proposal, device, "cancelled")
                .await?;
            return Ok(false);
        }
        let registration = sqlx::query("SELECT r.* FROM lane_push_registrations r JOIN lane_devices d ON d.account_id=r.account_id AND d.id=r.device_id WHERE r.account_id=$1 AND r.device_id=$2 AND r.enabled AND d.disconnected_at IS NULL")
            .bind(account).bind(device).fetch_optional(&mut *db).await?;
        let Some(registration) = registration else {
            self.terminal(db, account, proposal, device, "cancelled")
                .await?;
            return Ok(false);
        };
        let d = Delivery {
            account: account.to_owned(),
            proposal: proposal.to_owned(),
            device: device.to_owned(),
            repository: row.get("repository_name"),
            title: row.get("title"),
            expires,
            provider: Provider::parse(&json!(registration.get::<i16, _>("provider")))?,
            endpoint: registration.get("endpoint"),
            key: registration.get("public_key"),
            auth: registration.get("auth_secret"),
            generic: registration.get::<i16, _>("content") == 2,
        };
        let last = sqlx::query("SELECT * FROM lane_push_attempts WHERE account_id=$1 AND proposal_id=$2 AND device_id=$3 ORDER BY attempt DESC LIMIT 1")
            .bind(account).bind(proposal).bind(device).fetch_optional(&mut *db).await?;
        let mut attempt = 1;
        if let Some(last) = last {
            let number: i32 = last.get("attempt");
            attempt = number + 1;
            let result: Option<String> = last.get("result");
            if result.as_deref() == Some("success")
                || result.as_deref() == Some("permanent_failure")
            {
                self.terminal(
                    db,
                    account,
                    proposal,
                    device,
                    result.as_deref().unwrap_or("permanent_failure"),
                )
                .await?;
                return Ok(false);
            }
            let failed_at: DateTime<Utc> = last
                .get::<Option<DateTime<Utc>>, _>("finished_at")
                .unwrap_or_else(|| last.get("started_at"));
            if result.is_none() {
                // A crash can occur after the provider accepts a message but before
                // its response is durable. Retry conservatively with the same provider
                // collapse identity. Providers do not offer exactly-once receipts.
                sqlx::query("WITH changed AS (UPDATE lane_push_attempts SET result='transient_failure',finished_at=$5 WHERE account_id=$1 AND proposal_id=$2 AND device_id=$3 AND attempt=$4 AND result IS NULL RETURNING result) UPDATE lane_push_counters SET total=total+(SELECT count(*) FROM changed) WHERE result='transient_failure'")
                    .bind(account).bind(proposal).bind(device).bind(number).bind(failed_at).execute(&mut *db).await?;
            }
            match next_retry(number as usize, failed_at, expires, last.get("retry_after")) {
                None => {
                    self.terminal(
                        db,
                        account,
                        proposal,
                        device,
                        if number >= 6 { "exhausted" } else { "expired" },
                    )
                    .await?;
                    return Ok(false);
                }
                Some(next) if next > self.clock.now() => {
                    self.schedule(db, account, proposal, device, next).await?;
                    return Ok(false);
                }
                _ => (),
            }
        }
        if !self.available(d.provider) {
            self.schedule(
                db,
                account,
                proposal,
                device,
                (self.clock.now() + Duration::seconds(60)).min(expires),
            )
            .await?;
            return Ok(false);
        }
        if self.clock.now() >= expires {
            self.terminal(db, account, proposal, device, "expired")
                .await?;
            return Ok(false);
        }
        // Autocommit the reservation before any provider I/O. The session lock
        // remains held through this commit and the subsequent result write.
        sqlx::query("INSERT INTO lane_push_attempts(account_id,proposal_id,device_id,attempt,started_at) VALUES($1,$2,$3,$4,$5)")
            .bind(account).bind(proposal).bind(device).bind(attempt).bind(self.clock.now()).execute(&mut *db).await?;
        let (outcome, after) = self.dispatch(&d).await;
        let finished = self.clock.now();
        sqlx::query("WITH changed AS (UPDATE lane_push_attempts SET result=$5,finished_at=$6,retry_after=$7 WHERE account_id=$1 AND proposal_id=$2 AND device_id=$3 AND attempt=$4 AND result IS NULL RETURNING result) UPDATE lane_push_counters SET total=total+(SELECT count(*) FROM changed) WHERE result=$5")
            .bind(account).bind(proposal).bind(device).bind(attempt).bind(outcome.label()).bind(finished).bind(after).execute(&mut *db).await?;
        tracing::info!(result = outcome.label(), attempt, "push attempt completed");
        match outcome {
            Outcome::TransientFailure => {
                match next_retry(attempt as usize, finished, expires, after) {
                    Some(next) => self.schedule(db, account, proposal, device, next).await?,
                    None => {
                        self.terminal(
                            db,
                            account,
                            proposal,
                            device,
                            if attempt >= 6 { "exhausted" } else { "expired" },
                        )
                        .await?
                    }
                }
            }
            _ => {
                self.terminal(db, account, proposal, device, outcome.label())
                    .await?
            }
        }
        Ok(true)
    }
    async fn terminal(
        &self,
        db: &mut PgConnection,
        account: &str,
        proposal: &str,
        device: &str,
        state: &str,
    ) -> Result<()> {
        sqlx::query("UPDATE lane_push_deliveries SET state=$4,repository_name='',title='' WHERE account_id=$1 AND proposal_id=$2 AND device_id=$3 AND state='pending'")
            .bind(account).bind(proposal).bind(device).bind(state).execute(db).await?;
        Ok(())
    }
    async fn schedule(
        &self,
        db: &mut PgConnection,
        account: &str,
        proposal: &str,
        device: &str,
        next: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query("UPDATE lane_push_deliveries SET next_attempt_at=$4 WHERE account_id=$1 AND proposal_id=$2 AND device_id=$3 AND state='pending'")
            .bind(account).bind(proposal).bind(device).bind(next).execute(db).await?;
        Ok(())
    }
    /// Durable counters and queue gauges, with no account/device labels.
    pub async fn metrics(&self) -> Result<String> {
        let counts = sqlx::query("SELECT result,total AS n FROM lane_push_counters")
            .fetch_all(&self.db)
            .await?;
        let mut out = "# TYPE lane_push_attempts_total counter\n".to_owned();
        for outcome in [
            Outcome::Success,
            Outcome::TransientFailure,
            Outcome::PermanentFailure,
        ] {
            let count = counts
                .iter()
                .find(|r| r.get::<String, _>("result") == outcome.label())
                .map(|r| r.get::<i64, _>("n"))
                .unwrap_or(0);
            out.push_str(&format!(
                "lane_push_attempts_total{{result=\"{}\"}} {count}\n",
                outcome.label()
            ));
        }
        let row = sqlx::query("SELECT count(*) AS n,min(created_at) AS oldest FROM lane_push_deliveries WHERE state='pending'").fetch_one(&self.db).await?;
        let n: i64 = row.get("n");
        let age = row
            .get::<Option<DateTime<Utc>>, _>("oldest")
            .map(|t| (self.clock.now() - t).num_seconds().max(0))
            .unwrap_or(0);
        out.push_str(&format!("# TYPE lane_push_pending_count gauge\nlane_push_pending_count {n}\n# TYPE lane_push_oldest_pending_age_seconds gauge\nlane_push_oldest_pending_age_seconds {age}\n"));
        Ok(out)
    }
}

async fn live_proposal(db: &mut PgConnection, account: &str, proposal: &str) -> Result<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM lane_entities e JOIN lane_versions v ON v.account_id=e.account_id AND v.entity_id=e.id AND v.version_id=e.accepted_version_id JOIN lane_entities r ON r.account_id=e.account_id AND r.id=e.repository_id AND r.kind=1 AND NOT r.deleted WHERE e.account_id=$1 AND e.id=$2 AND e.kind=2 AND NOT e.deleted AND v.envelope->'task'->>'proposal'='PROPOSAL_PENDING')")
        .bind(account).bind(proposal).fetch_one(db).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, AtomicU16, AtomicUsize, Ordering};
    // Synthetic, test-only signing keys; never use these for provider accounts.
    const RSA: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDm9mQyZr0FED99
JdaGDucZyQ+SNKruuuEfZJCke+0I5xld9MldI0QxcseLJliUWz8SnBlkjuvwHvpV
wUY+CCDVQrdZNrb1xAT9UURzLGuwtVBr/boqNXork70lUyjDj8nY7AqEjz34mMvV
PhJhLeMNWESqaLuCBsycyE2rd9Bz2Qw72WYhBVK8TfxvmgHEaO3iSaGCif6kprNP
UoA+UNtTI4pIhsH+yi4lxtWjyqW+Vc44ylJsAFSvGr4ZDKQ51i9QKTPSGDhUIXq2
UniNOijaZmRsAhEMF8swcsQziG1WLrombsbBfCMOdjAL+Osx2QLE6s4fPD7/fbuQ
NDcWJcynAgMBAAECggEBAI4XIZd8a2d1EooswCjjfDv53VGGg26xnBRTK0V2VGK4
wntyQHvDWY3OXoL2CWLkLA6H6HtcDj1A4aPmbw90srZhACmrpq3WdXdfCuyMdIhX
U+3MQD79fXbXwnL3EUoW6QhCV0osUeTiI+zB7k7JCznHhx8LpegdxL3j4RzPbngF
8bPhSLAEgao4t9kZYHulHYbefXfdq7odMgeAjErnVQ9/VldHVw3OlgI6XGrPoyNb
Fa9dpmorG1QYx97luvAdI+Izyri5T+AtVjAhLkuMR7JQbMak/5uch0KxAvuGnYok
FNdHaj5nang72pftrNpfwXBcVQeVFlm+XMdpHzBg8SkCgYEA/YV12XnuCZJj/m+r
SilDikiSjyeTFTqfqgndaPOQti+FinaHkGHdlZInjg2+vcaTkPibM9FVf/IEEOE0
yLKYfTQxbKj6uqlJ1WKhSdZRZk+GHLMdOYI8++4MH09iOmf7dIpkwjwGwIxujv/Z
muiABp8RklsWix5qIsagGMYcQSMCgYEA6Th35uGwITPazddJ7bCVhIqCH0lmugYd
SJHuivy7Yh6XocUFUZ6DimSysr/lEuB5Zitk5Mm7WVGCt7WWqc6yPQMZd9qRJ0f5
nnKV9cZuBNHZOBb0bzOuJ8SjKxaD0eUKYdLlYOY82dIwPz/eMIh01z6eb5V6jLRn
fNbF8CSrmK0CgYByN6yOnEZH/bIbMYhEUy7goC2XXUcAq3a/js43uHQcAHDzns8Y
l17TQhOcBklHCq31BGw3/jm8+PYn/K5hNUVKMzeJMuAzNI3VGyh7MZletLlBONkT
zb9X6TmA6azhj3NUX/1hHNYV7a7psg8BoXn8gbHS9ELFzqAWptH+F1MnbQKBgQCU
P4PgLJgkEu1EKVTiFXyFmvHp52O4XgihR7pFeS+3yFCE0VN3NXbSMv5wp5B6tlwU
+QKSeDVyKmEYXAKggDZGmEDJMhNQz+y5robqiy8GO0qD3/uS8FwFBXbe/dnfQ2Od
GU0bkl8d6wEmQ3f+i88Rv8lAjtkutcKO1YtPAlBdpQKBgGIz/OJDhBnWPkZCMDQq
2fatQ4cJU1FhQT2BioLVRR7OAiAK+fUeptYN94w4bdfWzahNbMPyRHC5TKOq9quS
jvlhaPb0/9XgZ6HqExfd9329NkgGAMYBtfMYo99WEinrwaFL4a5KQ+ldcWayeGRX
o2VxEuESnVYjhlmhqPw83AlW
-----END PRIVATE KEY-----
"#;
    const EC: &str = r#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgTPE7qIDC9ma3UN+R
H05S1kFxHMFDjXyvaZvYrasEsgyhRANCAAQ8iEDlBQ2Kx/vBTXBBMn12cjOU+QNg
WKgZtSNaVvVIh0CUGOnHJip/8DwH7dR6SS1rtc/UT0nkE4YvhMu4j4S8
-----END PRIVATE KEY-----
"#;
    struct FakeClock(AtomicI64);
    impl FakeClock {
        fn at(now: DateTime<Utc>) -> Self {
            Self(AtomicI64::new(now.timestamp()))
        }
        fn advance(&self, seconds: i64) {
            self.0.fetch_add(seconds, Ordering::SeqCst);
        }
    }
    impl Clock for FakeClock {
        fn now(&self) -> DateTime<Utc> {
            DateTime::from_timestamp(self.0.load(Ordering::SeqCst), 0).unwrap()
        }
    }
    fn principal(origin: &str, device: &str) -> Principal {
        Principal {
            account_id: "account".into(),
            tenant_id: "tenant".into(),
            session_id: "session".into(),
            device_id: device.into(),
            device_name: "Device".into(),
            origin_kind: origin.into(),
        }
    }
    fn delivery(now: DateTime<Utc>, provider: Provider) -> Delivery {
        Delivery {
            account: "account".into(),
            proposal: "proposal".into(),
            device: "device".into(),
            repository: "Private repository".into(),
            title: "Private task".into(),
            expires: now + Duration::hours(2),
            provider,
            endpoint: "abc123".into(),
            key: "".into(),
            auth: "".into(),
            generic: false,
        }
    }
    fn push(db: PgPool, clock: Arc<FakeClock>, base: &str) -> Push {
        Push {
            db,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            clock,
            providers: Providers {
                apns: Some(Apns {
                    key: Arc::new(EncodingKey::from_ec_pem(EC.as_bytes()).unwrap()),
                    key_id: "key-id".into(),
                    team_id: "team-id".into(),
                    topic: "io.delino.lane".into(),
                    endpoint: base.into(),
                }),
                fcm: Some(Fcm {
                    key: Arc::new(EncodingKey::from_rsa_pem(RSA.as_bytes()).unwrap()),
                    email: "test@example.invalid".into(),
                    project: "test-project".into(),
                    token_endpoint: format!("{base}/token"),
                    endpoint: base.into(),
                }),
                web: Some(WebPush {
                    key: EC.into(),
                    subject: "mailto:test@example.invalid".into(),
                }),
            },
        }
    }
    fn lazy_db() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap()
    }
    #[test]
    fn shared_retry_and_content_vectors_execute_runtime_policy() {
        let vectors: Value =
            serde_json::from_str(include_str!("../../../protos/lane/v1/fixtures/push.json"))
                .unwrap();
        assert_eq!(vectors["formatVersion"], 1);
        let mut count = 0;
        for case in vectors["cases"].as_array().unwrap() {
            let input = &case["input"];
            let expected = &case["expected"];
            match input["operation"].as_str().unwrap() {
                "scheduleRetry" => {
                    let seconds = |key: &str| input[key].as_str().unwrap().parse::<i64>().unwrap();
                    let at =
                        DateTime::from_timestamp(seconds("previousAttemptSeconds"), 0).unwrap();
                    let expiry =
                        DateTime::from_timestamp(seconds("createdAtSeconds") + 7200, 0).unwrap();
                    let next = if input["failure"] == "PERMANENT" {
                        None
                    } else {
                        next_retry(
                            input["retryIndex"].as_u64().unwrap() as usize + 1,
                            at,
                            expiry,
                            input["retryAfter"]
                                .as_str()
                                .and_then(|s| retry_after(s, at)),
                        )
                    };
                    assert_eq!(
                        next.map(|v| v.timestamp().to_string()),
                        expected["nextAttemptSeconds"].as_str().map(str::to_owned),
                        "{}",
                        case["id"]
                    );
                    count += 1;
                }
                "renderNotification" => {
                    let mut d = delivery(Utc::now(), Provider::Fcm);
                    d.generic = content(input).unwrap() == 2;
                    let output = d.payload().to_string();
                    assert_eq!(
                        output.contains("Private repository"),
                        expected["includesRepositoryName"].as_bool().unwrap()
                    );
                    assert_eq!(
                        output.contains("Private task"),
                        expected["includesTaskTitle"].as_bool().unwrap()
                    );
                    count += 1;
                }
                _ => (), // Database and adapter vectors run in their owning integration tests.
            }
        }
        assert_eq!(count, 14);
    }
    #[test]
    fn retry_boundaries_and_invalid_headers() {
        let at = DateTime::from_timestamp(0, 0).unwrap();
        assert_eq!(next_retry(0, at, at + Duration::hours(2), None), None);
        assert_eq!(
            next_retry(
                1,
                at,
                at + Duration::hours(2),
                retry_after("18446744073709551615", at)
            ),
            None
        );
        assert_eq!(next_retry(6, at, at + Duration::hours(2), None), None);
        for bad in ["-1", "invalid"] {
            assert!(retry_after(bad, at).is_none());
        }
        assert_eq!(
            next_retry(1, at, at + Duration::hours(2), retry_after("0", at)),
            Some(at + Duration::seconds(30))
        );
    }
    #[test]
    fn registration_opt_in_and_ssrf() {
        assert!(
            !registration(&json!({"provider":"PUSH_PROVIDER_FCM","endpoint":"token"}))
                .unwrap()
                .4
        );
        assert_eq!(
            registration(&json!({"provider":2,"endpoint":"token","enabled":true}))
                .unwrap()
                .5,
            1
        );
        for url in [
            "http://fcm.googleapis.com/send",
            "https://127.0.0.1/send",
            "https://fcm.googleapis.com.evil.invalid/send",
            "https://fcm.googleapis.com:8443/send",
            "https://user@fcm.googleapis.com/send",
        ] {
            assert!(web_endpoint(url).is_err(), "{url}");
        }
        assert!(web_endpoint("https://updates.push.services.mozilla.com/wpush/v2/token").is_ok());
        assert!(registration(&json!({"provider":9,"enabled":true})).is_err());
        assert!(registration(&json!({"provider":1,"endpoint":"abc/def","enabled":true})).is_err());
        assert!(registration(&json!({"provider":3,"endpoint":"https://fcm.googleapis.com/send","publicKey":"AA==","authSecret":"AA==","enabled":true})).is_err());
        assert!(is_mcp(&principal("ORIGIN_KIND_MCP", "origin")));
        assert!(!is_mcp(&principal("ORIGIN_KIND_APP", "origin")));
        assert!(!is_mcp(&principal("ORIGIN_KIND_CLI", "origin")));
    }
    struct Mock {
        base: String,
        status: Arc<AtomicU16>,
        after: Arc<std::sync::Mutex<Option<String>>>,
        sends: Arc<AtomicUsize>,
        requests: Arc<std::sync::Mutex<Vec<(String, axum::http::HeaderMap, Vec<u8>)>>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn mock() -> Mock {
        let status = Arc::new(AtomicU16::new(503));
        let after = Arc::new(std::sync::Mutex::new(None::<String>));
        let sends = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (s, n, r, a) = (
            status.clone(),
            sends.clone(),
            requests.clone(),
            after.clone(),
        );
        let router = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let (s, n, r, a) = (s.clone(), n.clone(), r.clone(), a.clone());
            async move {
                let (parts, body) = request.into_parts();
                let body = axum::body::to_bytes(body, 65536).await.unwrap();
                r.lock()
                    .unwrap()
                    .push((parts.uri.path().to_owned(), parts.headers, body.to_vec()));
                if parts.uri.path() == "/token" {
                    axum::response::Response::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(
                            "{\"access_token\":\"synthetic-token\"}",
                        ))
                        .unwrap()
                } else {
                    n.fetch_add(1, Ordering::SeqCst);
                    axum::response::Response::builder()
                        .header("Retry-After", a.lock().unwrap().clone().unwrap_or_default())
                        .status(s.load(Ordering::SeqCst))
                        .body(axum::body::Body::empty())
                        .unwrap()
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Mock {
            base,
            status,
            after,
            sends,
            requests,
            task,
        }
    }
    #[tokio::test]
    async fn provider_requests_and_mocked_failure_classes() {
        let server = mock().await;
        let clock = Arc::new(FakeClock::at(Utc::now()));
        let worker = push(lazy_db(), clock.clone(), &server.base);
        let d = delivery(clock.now(), Provider::Apns);
        let request = worker
            .prepare(&d, clock.now())
            .await
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(request.version(), reqwest::Version::HTTP_2);
        assert_eq!(request.headers()["apns-topic"], "io.delino.lane");
        assert_eq!(
            request.headers()["apns-expiration"],
            d.expires.timestamp().to_string()
        );
        assert_eq!(request.headers()["apns-collapse-id"], d.collapse());
        let jwt = request.headers()["authorization"]
            .to_str()
            .unwrap()
            .strip_prefix("Bearer ")
            .unwrap();
        let claims: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(jwt.split('.').nth(1).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["iss"], "team-id");
        let apns: Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(apns["aps"]["content-available"], 1);
        assert!(apns["aps"].get("alert").is_none());
        assert_eq!(request.headers()["apns-push-type"], "background");
        assert_eq!(request.headers()["apns-priority"], "5");
        let d = delivery(clock.now(), Provider::Fcm);
        for (status, expected) in [
            (200, Outcome::Success),
            (429, Outcome::TransientFailure),
            (503, Outcome::TransientFailure),
            (410, Outcome::PermanentFailure),
            (400, Outcome::PermanentFailure),
        ] {
            server.status.store(status, Ordering::SeqCst);
            assert_eq!(worker.dispatch(&d).await.0, expected);
        }
        let requests = server.requests.lock().unwrap();
        let (path, headers, body) = requests
            .iter()
            .find(|r| r.0.contains("messages:send"))
            .unwrap();
        assert_eq!(path, "/v1/projects/test-project/messages:send");
        assert_eq!(headers["authorization"], "Bearer synthetic-token");
        let body: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(body["message"]["android"]["ttl"], "7200s");
        assert!(body["message"].get("notification").is_none());
        assert_eq!(body["message"]["data"]["proposalId"], "proposal");
        drop(requests);
        let before = server.sends.load(Ordering::SeqCst);
        clock.advance(7200);
        assert_eq!(worker.dispatch(&d).await.0, Outcome::PermanentFailure);
        assert_eq!(server.sends.load(Ordering::SeqCst), before);
    }
    #[tokio::test]
    async fn web_push_encrypts_and_authenticates_payload() {
        let clock = Arc::new(FakeClock::at(Utc::now()));
        let worker = push(lazy_db(), clock.clone(), "http://localhost");
        let mut d = delivery(clock.now(), Provider::WebPush);
        d.endpoint = "https://fcm.googleapis.com/fcm/send/synthetic".into();
        // Public test subscription from the Web Push RFC-compatible key format.
        d.key="BLMbF9ffKBiWQLCKvTHb6LO8Nb6dcUh6TItC455vu2kElga6PQvUmaFyCdykxY2nOSSL3yKgfbmFLRTUaGv4yV8".into();
        d.auth = "xS03Fi5ErfTNH_l9WHE9Ig".into();
        let request = worker
            .prepare(&d, clock.now())
            .await
            .unwrap()
            .build()
            .unwrap();
        let public = URL_SAFE_NO_PAD
            .decode(worker.public_key().unwrap())
            .unwrap();
        assert_eq!(public.len(), 65);
        assert_eq!(public[0], 4);
        assert_eq!(request.headers()["content-encoding"], "aes128gcm");
        assert_eq!(request.headers()["ttl"], "7200");
        assert!(
            request.headers()["authorization"]
                .to_str()
                .unwrap()
                .starts_with("vapid ")
        );
        let bytes = request.body().unwrap().as_bytes().unwrap();
        assert!(
            !bytes
                .windows(b"Private task".len())
                .any(|w| w == b"Private task")
        );
        assert!(bytes.len() > 100);
    }
    async fn database() -> (PgPool, PgPool, String) {
        let url = std::env::var("LANE_PUSH_TEST_DATABASE_URL")
            .expect("set disposable LANE_PUSH_TEST_DATABASE_URL");
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("push_{}", uuid::Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = schema.clone();
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |conn, _| {
                let query = format!("SET search_path TO {search}");
                Box::pin(async move {
                    sqlx::query(&query).execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE lane_devices(account_id text,id text,name text,last_sync_at timestamptz,disconnected_at timestamptz,PRIMARY KEY(account_id,id)); CREATE TABLE lane_entities(account_id text,id text,kind smallint,deleted boolean DEFAULT false,repository_id text,accepted_version_id text,PRIMARY KEY(account_id,id)); CREATE TABLE lane_versions(account_id text,entity_id text,version_id text,envelope jsonb,PRIMARY KEY(account_id,version_id));").execute(&db).await.unwrap();
        sqlx::raw_sql(include_str!("../migrations/00004_push.sql"))
            .execute(&db)
            .await
            .unwrap();
        (db, admin, schema)
    }
    async fn seed(db: &PgPool, id: &str, now: DateTime<Utc>) {
        sqlx::query("INSERT INTO lane_entities(account_id,id,kind,repository_id,accepted_version_id) VALUES('account',$1,2,'repository',$1)").bind(id).execute(db).await.unwrap();
        sqlx::query("INSERT INTO lane_versions(account_id,entity_id,version_id,envelope) VALUES('account',$1,$1,'{\"task\":{\"proposal\":\"PROPOSAL_PENDING\"}}')").bind(id).execute(db).await.unwrap();
        let mut tx = db.begin().await.unwrap();
        crate::account_lock(&mut tx, "account").await.unwrap();
        enqueue(
            &mut tx,
            &principal("ORIGIN_KIND_MCP", "origin"),
            id,
            "Private repository",
            "Private task",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        sqlx::query("UPDATE lane_push_deliveries SET created_at=$2,expires_at=$2+interval '2 hours',next_attempt_at=$2 WHERE proposal_id=$1").bind(id).bind(now).execute(db).await.unwrap();
    }
    async fn state(db: &PgPool, id: &str) -> String {
        sqlx::query_scalar("SELECT state FROM lane_push_deliveries WHERE proposal_id=$1")
            .bind(id)
            .fetch_one(db)
            .await
            .unwrap()
    }
    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL; set LANE_PUSH_TEST_DATABASE_URL"]
    async fn durable_worker_fake_clock_database_and_mock_provider() {
        let (db, admin, schema) = database().await;
        let server = mock().await;
        let clock = Arc::new(FakeClock::at(Utc::now()));
        let worker = push(db.clone(), clock.clone(), &server.base);
        sqlx::raw_sql("INSERT INTO lane_devices(account_id,id,name) VALUES('account','origin','Origin'),('account','device','Device'),('other','device','Other'); INSERT INTO lane_entities(account_id,id,kind,accepted_version_id) VALUES('account','repository',1,'repo-version');").execute(&db).await.unwrap();
        for device in ["origin", "device"] {
            let mut tx = db.begin().await.unwrap();
            crate::account_lock(&mut tx, "account").await.unwrap();
            let response=register(&mut tx,&principal("ORIGIN_KIND_APP",device),&json!({"deviceId":device,"provider":2,"endpoint":"synthetic-token","enabled":true})).await.unwrap();
            assert_eq!(response["device"]["id"], device);
            tx.commit().await.unwrap();
        }
        let mut tx = db.begin().await.unwrap();
        assert!(
            register(
                &mut tx,
                &principal("ORIGIN_KIND_APP", "origin"),
                &json!({"deviceId":"device","provider":2,"enabled":false})
            )
            .await
            .is_err()
        );
        tx.rollback().await.unwrap();
        seed(&db, "retry", clock.now()).await;
        // Replayed enqueue neither duplicates work nor schedules the origin device.
        let mut tx = db.begin().await.unwrap();
        enqueue(
            &mut tx,
            &principal("ORIGIN_KIND_MCP", "origin"),
            "retry",
            "changed",
            "changed",
        )
        .await
        .unwrap();
        enqueue(
            &mut tx,
            &principal("ORIGIN_KIND_APP", "origin"),
            "app",
            "repo",
            "task",
        )
        .await
        .unwrap();
        enqueue(
            &mut tx,
            &principal("ORIGIN_KIND_CLI", "origin"),
            "cli",
            "repo",
            "task",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM lane_push_deliveries")
                .fetch_one(&db)
                .await
                .unwrap(),
            1
        );
        assert_eq!(worker.tick().await.unwrap(), 1);
        for delay in RETRY_SECONDS {
            assert_eq!(worker.tick().await.unwrap(), 0);
            clock.advance(delay - 1);
            assert_eq!(worker.tick().await.unwrap(), 0);
            clock.advance(1);
            assert_eq!(worker.tick().await.unwrap(), 1);
        }
        assert_eq!(state(&db, "retry").await, "exhausted");
        assert_eq!(server.sends.load(Ordering::SeqCst), 6);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM lane_push_attempts WHERE result='transient_failure'"
            )
            .fetch_one(&db)
            .await
            .unwrap(),
            6
        );
        assert!(
            worker
                .metrics()
                .await
                .unwrap()
                .contains("lane_push_attempts_total{result=\"transient_failure\"} 6")
        );
        assert!(
            worker
                .metrics()
                .await
                .unwrap()
                .contains("lane_push_oldest_pending_age_seconds 0")
        );
        for disposition in ["PROPOSAL_ACCEPTED", "PROPOSAL_REJECTED"] {
            seed(&db, disposition, clock.now()).await;
            sqlx::query("UPDATE lane_versions SET envelope=jsonb_build_object('task',jsonb_build_object('proposal',$1::text)) WHERE version_id=$1").bind(disposition).execute(&db).await.unwrap();
            assert_eq!(worker.tick().await.unwrap(), 0);
            assert_eq!(state(&db, disposition).await, "cancelled");
        }
        seed(&db, "deleted", clock.now()).await;
        sqlx::query("UPDATE lane_entities SET deleted=true WHERE id='deleted'")
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(worker.tick().await.unwrap(), 0);
        assert_eq!(state(&db, "deleted").await, "cancelled");
        seed(&db, "expired", clock.now()).await;
        clock.advance(7200);
        assert_eq!(worker.tick().await.unwrap(), 0);
        assert_eq!(state(&db, "expired").await, "expired");
        seed(&db, "revoked", clock.now()).await;
        sqlx::query("UPDATE lane_devices SET disconnected_at=now() WHERE account_id='account' AND id='device'").execute(&db).await.unwrap();
        assert_eq!(worker.tick().await.unwrap(), 0);
        assert_eq!(state(&db, "revoked").await, "cancelled");
        sqlx::query("UPDATE lane_devices SET disconnected_at=NULL WHERE account_id='account' AND id='device'").execute(&db).await.unwrap();
        seed(&db, "cancel", clock.now()).await;
        let mut tx = db.begin().await.unwrap();
        crate::account_lock(&mut tx, "account").await.unwrap();
        cancel(&mut tx, "other", "cancel").await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(state(&db, "cancel").await, "pending");
        let mut tx = db.begin().await.unwrap();
        crate::account_lock(&mut tx, "account").await.unwrap();
        cancel(&mut tx, "account", "cancel").await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(worker.tick().await.unwrap(), 0);
        assert_eq!(state(&db, "cancel").await, "cancelled");
        seed(&db, "disabled", clock.now()).await;
        let mut disabled = worker.clone();
        disabled.providers = Providers::default();
        assert_eq!(disabled.tick().await.unwrap(), 0);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM lane_push_attempts WHERE proposal_id='disabled'"
            )
            .fetch_one(&db)
            .await
            .unwrap(),
            0
        );
        let mut tx = db.begin().await.unwrap();
        register(
            &mut tx,
            &principal("ORIGIN_KIND_APP", "device"),
            &json!({"deviceId":"device","provider":2,"enabled":false}),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(state(&db, "disabled").await, "cancelled");
        seed(&db, "opt-out", clock.now()).await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM lane_push_deliveries WHERE proposal_id='opt-out'"
            )
            .fetch_one(&db)
            .await
            .unwrap(),
            0
        );
        let mut tx = db.begin().await.unwrap();
        register(&mut tx,&principal("ORIGIN_KIND_APP","device"),&json!({"deviceId":"device","provider":2,"endpoint":"token","enabled":true,"content":2})).await.unwrap();
        tx.commit().await.unwrap();
        server.status.store(200, Ordering::SeqCst);
        seed(&db, "success", clock.now()).await;
        assert_eq!(worker.tick().await.unwrap(), 1);
        assert_eq!(state(&db, "success").await, "success");
        let captured = server.requests.lock().unwrap();
        let body: Value = serde_json::from_slice(&captured.last().unwrap().2).unwrap();
        assert!(body["message"].get("notification").is_none());
        assert_eq!(body["message"]["data"]["proposalId"], "success");
        drop(captured);
        // A committed successful attempt with an interrupted queue update is recovered without another send.
        sqlx::query("UPDATE lane_push_deliveries SET state='pending' WHERE proposal_id='success'")
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(worker.tick().await.unwrap(), 0);
        assert_eq!(state(&db, "success").await, "success");
        seed(&db, "crash", clock.now()).await;
        sqlx::query("INSERT INTO lane_push_attempts(account_id,proposal_id,device_id,attempt,started_at) VALUES('account','crash','device',1,$1)").bind(clock.now()).execute(&db).await.unwrap();
        assert_eq!(worker.tick().await.unwrap(), 0);
        clock.advance(30);
        assert_eq!(worker.tick().await.unwrap(), 1);
        assert_eq!(state(&db, "crash").await, "success");
        server.status.store(410, Ordering::SeqCst);
        seed(&db, "permanent", clock.now()).await;
        assert_eq!(worker.tick().await.unwrap(), 1);
        assert_eq!(state(&db, "permanent").await, "permanent_failure");
        clock.advance(7200);
        assert_eq!(worker.tick().await.unwrap(), 0);
        server.status.store(503, Ordering::SeqCst);
        *server.after.lock().unwrap() = Some(httpdate::fmt_http_date(
            (clock.now() + Duration::seconds(300)).into(),
        ));
        seed(&db, "http-date", clock.now()).await;
        assert_eq!(worker.tick().await.unwrap(), 1);
        clock.advance(299);
        assert_eq!(worker.tick().await.unwrap(), 0);
        server.status.store(200, Ordering::SeqCst);
        clock.advance(1);
        assert_eq!(worker.tick().await.unwrap(), 1);
        assert_eq!(state(&db, "http-date").await, "success");
        server.status.store(429, Ordering::SeqCst);
        *server.after.lock().unwrap() = Some("7200".into());
        seed(&db, "after-expiry", clock.now()).await;
        assert_eq!(worker.tick().await.unwrap(), 1);
        assert_eq!(state(&db, "after-expiry").await, "expired");
        let sends = server.sends.load(Ordering::SeqCst);
        clock.advance(7200);
        assert_eq!(worker.tick().await.unwrap(), 0);
        assert_eq!(server.sends.load(Ordering::SeqCst), sends);
        *server.after.lock().unwrap() = None;
        server.status.store(200, Ordering::SeqCst);
        seed(&db, "concurrent", clock.now()).await;
        let (first, second) = tokio::join!(worker.tick(), worker.tick());
        assert_eq!(first.unwrap() + second.unwrap(), 1);
        seed(&db, "locked-cancel", clock.now()).await;
        let mut tx = db.begin().await.unwrap();
        crate::account_lock(&mut tx, "account").await.unwrap();
        assert_eq!(worker.tick().await.unwrap(), 0);
        cancel(&mut tx, "account", "locked-cancel").await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(worker.tick().await.unwrap(), 0);
        let before = worker.metrics().await.unwrap();
        sqlx::query("DELETE FROM lane_push_deliveries WHERE account_id='account'")
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(worker.metrics().await.unwrap(), before);
        db.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}
