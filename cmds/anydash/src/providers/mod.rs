// SPDX-License-Identifier: Apache-2.0
//! Provider adapters. Each adapter reads its credential, calls only the read endpoints of the
//! provider matrix and returns a snapshot. Response bodies stay in memory and are never logged.
use crate::{
    error::Result,
    keys::SecureStore,
    pb::{
        CollectionStatus, FailureClass, MetricKind, MetricObservation, MetricState,
        MetricTemporality, MetricValue, Money, ObservationSource, Provider, ResourceHealth,
        ResourceObservation, Snapshot, metric_value,
    },
    schedule,
    settings::Settings,
    store::ConnectionRow,
    time,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
pub mod claude;
pub mod cloudflare;
pub mod codex;
pub mod grok;
pub mod supabase;
pub mod vercel;
const MAX_BODY: usize = 16 << 20;
/// Provider base URLs; tests point them at a local mock server.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub codex: String,
    pub anthropic: String,
    pub vercel: String,
    pub supabase: String,
    pub cloudflare: String,
    /// The `grok` executable name or path.
    pub grok: String,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            codex: "https://chatgpt.com/backend-api".into(),
            anthropic: "https://api.anthropic.com".into(),
            vercel: "https://api.vercel.com".into(),
            supabase: "https://api.supabase.com".into(),
            cloudflare: "https://api.cloudflare.com/client/v4".into(),
            grok: "grok".into(),
        }
    }
}
impl Endpoints {
    /// Every endpoint at one mock base URL, for tests.
    #[must_use]
    pub fn mock(base: &str, grok: &str) -> Self {
        Self {
            codex: format!("{base}/backend-api"),
            anthropic: base.into(),
            vercel: base.into(),
            supabase: base.into(),
            cloudflare: format!("{base}/client/v4"),
            grok: grok.into(),
        }
    }
}
/// Shared adapter context: one HTTP client with a 30-second timeout, TLS verification and no
/// cookie store.
#[derive(Clone)]
pub struct Ctx {
    pub http: reqwest::Client,
    pub endpoints: Endpoints,
    pub settings: Settings,
    pub os: Arc<dyn SecureStore>,
}
impl Ctx {
    pub fn new(endpoints: Endpoints, settings: Settings, os: Arc<dyn SecureStore>) -> Result<Self> {
        let allow_http = crate::api::loopback(&endpoints.vercel);
        Ok(Self {
            http: crate::api::http_client(allow_http)?,
            endpoints,
            settings,
            os,
        })
    }
}
#[must_use]
pub const fn provider_name(p: Provider) -> &'static str {
    match p {
        Provider::Codex => "codex",
        Provider::ClaudeCode => "claude-code",
        Provider::Grok => "grok",
        Provider::Vercel => "vercel",
        Provider::Supabase => "supabase",
        Provider::Cloudflare => "cloudflare",
        Provider::Unspecified => "unknown",
    }
}
#[must_use]
pub fn parse_provider(name: &str) -> Option<Provider> {
    [
        Provider::Codex,
        Provider::ClaudeCode,
        Provider::Grok,
        Provider::Vercel,
        Provider::Supabase,
        Provider::Cloudflare,
    ]
    .into_iter()
    .find(|p| provider_name(*p) == name)
}
/// Providers whose credential is the local CLI login rather than a stored official token.
#[must_use]
pub const fn uses_local_cli(p: Provider) -> bool {
    matches!(p, Provider::Codex | Provider::ClaudeCode | Provider::Grok)
}
/// Lowercase hex SHA-256 of `<Provider value name>:<account ID>`, or empty without an ID.
#[must_use]
pub fn fingerprint(p: Provider, account: &str) -> String {
    if account.is_empty() {
        return String::new();
    }
    hex::encode(Sha256::digest(format!("{}:{account}", p.as_str_name())))
}
#[must_use]
pub fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
/// A failed provider interaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fail {
    pub class: FailureClass,
    pub http_status: u32,
    pub retry_after: Option<Duration>,
}
impl Fail {
    #[must_use]
    pub const fn new(class: FailureClass) -> Self {
        Self {
            class,
            http_status: 0,
            retry_after: None,
        }
    }
    #[must_use]
    pub const fn status(&self) -> CollectionStatus {
        match self.class {
            FailureClass::RateLimited => CollectionStatus::RateLimited,
            FailureClass::AuthExpired | FailureClass::CredentialMissing => {
                CollectionStatus::AuthExpired
            }
            FailureClass::CliUnavailable => CollectionStatus::NotProvided,
            _ => CollectionStatus::Failed,
        }
    }
}
/// The result of one adapter cycle.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub snapshot: Snapshot,
    pub retry_after: Option<Duration>,
    /// Digest of the local CLI credential used in this cycle, or empty.
    pub marker: String,
    /// No provider request was made because the stored credential is still expired.
    pub skipped: bool,
    /// No provider request was made because the local CLI is signed in to a different account
    /// than the connection's; the device hands the lease to another ready device.
    pub account_mismatch: bool,
}
/// Response of a provider GET: status, `Retry-After` and body.
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}
/// Send a read-only request and classify failures. 2xx responses are returned; 401, 403, 429
/// and other statuses become failures.
pub async fn send(
    endpoint: &str,
    request: reqwest::RequestBuilder,
) -> std::result::Result<Response, Fail> {
    let started = std::time::Instant::now();
    let result = async {
        let mut response = request.send().await.map_err(|e| {
            Fail::new(if e.is_timeout() {
                FailureClass::Timeout
            } else {
                FailureClass::Network
            })
        })?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| schedule::retry_after(v, time::now()));
        let mut body = vec![];
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len() + chunk.len() > MAX_BODY {
                        return Err(Fail::new(FailureClass::ResponseUnreadable));
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(e) => {
                    return Err(Fail::new(if e.is_timeout() {
                        FailureClass::Timeout
                    } else {
                        FailureClass::Network
                    }));
                }
            }
        }
        let class = match status {
            200..=299 => return Ok(Response { status, body }),
            401 => FailureClass::AuthExpired,
            403 => FailureClass::PermissionDenied,
            429 => FailureClass::RateLimited,
            _ => FailureClass::ProviderError,
        };
        Err(Fail {
            class,
            http_status: u32::from(status),
            retry_after,
        })
    }
    .await;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match &result {
        Ok(r) => tracing::debug!(
            event = "provider_request",
            endpoint,
            http_status = r.status,
            duration_ms
        ),
        Err(f) => tracing::info!(
            event = "provider_request",
            endpoint,
            http_status = f.http_status,
            failure_class = f.class.as_str_name(),
            duration_ms
        ),
    }
    result
}
pub fn json(response: &Response) -> std::result::Result<Value, Fail> {
    serde_json::from_slice(&response.body).map_err(|_| Fail::new(FailureClass::ResponseUnreadable))
}
/// Collects metric and resource observations for one snapshot.
#[derive(Default)]
pub struct Builder {
    pub metrics: Vec<MetricObservation>,
    pub resources: Vec<ResourceObservation>,
    partial: Option<Fail>,
}
#[must_use]
pub fn value(state: MetricState, v: Option<metric_value::Value>) -> MetricValue {
    MetricValue {
        state: state.into(),
        value: v,
    }
}
#[must_use]
pub fn observation(kind: MetricKind, dimension: &str, v: MetricValue) -> MetricObservation {
    MetricObservation {
        kind: kind.into(),
        dimension: dimension.to_owned(),
        value: Some(v),
        temporality: MetricTemporality::Instantaneous.into(),
        ..MetricObservation::default()
    }
}
impl Builder {
    pub fn set(&mut self, kind: MetricKind, dimension: &str, v: metric_value::Value) {
        self.metrics.push(observation(
            kind,
            dimension,
            value(MetricState::Value, Some(v)),
        ));
    }
    pub fn not_provided(&mut self, kind: MetricKind, dimension: &str) {
        self.metrics.push(observation(
            kind,
            dimension,
            value(MetricState::NotProvided, None),
        ));
    }
    /// A metric that could not be read; the snapshot becomes partially successful.
    pub fn failed(&mut self, kind: MetricKind, dimension: &str, fail: Fail) {
        self.metrics.push(observation(
            kind,
            dimension,
            value(MetricState::Failed, None),
        ));
        self.partial.get_or_insert(fail);
    }
    pub fn mark_partial(&mut self, fail: Fail) {
        self.partial.get_or_insert(fail);
    }
    /// Keep a failed sub-request as a partial result, except rate limits and expired
    /// authentication, which fail the whole cycle.
    pub fn tolerate(&mut self, fail: Fail) -> std::result::Result<(), Fail> {
        if matches!(
            fail.class,
            FailureClass::RateLimited | FailureClass::AuthExpired
        ) {
            return Err(fail);
        }
        self.mark_partial(fail);
        Ok(())
    }
    pub fn failed_soft(
        &mut self,
        kind: MetricKind,
        dimension: &str,
        fail: Fail,
    ) -> std::result::Result<(), Fail> {
        if matches!(
            fail.class,
            FailureClass::RateLimited | FailureClass::AuthExpired
        ) {
            return Err(fail);
        }
        self.failed(kind, dimension, fail);
        Ok(())
    }
    /// A value or, when it is absent, `NOT_PROVIDED`.
    pub fn maybe(&mut self, kind: MetricKind, dimension: &str, v: Option<metric_value::Value>) {
        match v {
            Some(v) => self.set(kind, dimension, v),
            None => self.not_provided(kind, dimension),
        }
    }
    #[must_use]
    pub fn finish(self, now: i64) -> Snapshot {
        let (status, class, http) = self.partial.map_or(
            (CollectionStatus::Succeeded, FailureClass::Unspecified, 0),
            |f| (CollectionStatus::PartiallySucceeded, f.class, f.http_status),
        );
        Snapshot {
            observed_at: Some(time::timestamp(now)),
            status: status.into(),
            failure_class: class.into(),
            metrics: self.metrics,
            resources: self.resources,
            provider_http_status: http,
            source: ObservationSource::ActivePoll.into(),
            next_attempt_at: None,
        }
    }
}
/// A cumulative quantity over `period` (Unix seconds).
#[must_use]
pub fn cumulative(
    kind: MetricKind,
    dimension: &str,
    v: metric_value::Value,
    unit: &str,
    period: (i64, i64),
) -> MetricObservation {
    let mut o = observation(kind, dimension, value(MetricState::Value, Some(v)));
    unit.clone_into(&mut o.unit);
    o.temporality = MetricTemporality::Cumulative.into();
    o.period_start = Some(time::timestamp(period.0));
    o.period_end = Some(time::timestamp(period.1));
    o
}
/// A failure snapshot for a cycle that failed as a whole.
///
/// Subscription providers carry the failure state for their main keys so that clients show the last successful value with the
/// error. Deployment providers report many usage, cost and resource metrics that no fixed key
/// list covers, so their failure snapshot carries no metrics: the AnyDash API then marks every
/// latest metric of the connection failed and keeps its last successful value.
#[must_use]
pub fn failure_snapshot(provider: Provider, fail: &Fail, now: i64) -> Snapshot {
    let state = match fail.class {
        FailureClass::AuthExpired | FailureClass::CredentialMissing => MetricState::AuthExpired,
        FailureClass::CliUnavailable => MetricState::NotProvided,
        _ => MetricState::Failed,
    };
    let keys: &[(MetricKind, &str)] = match provider {
        Provider::Codex => &[
            (MetricKind::SubscriptionUsedPercent, "five_hour"),
            (MetricKind::SubscriptionUsedPercent, "weekly"),
            (MetricKind::SubscriptionPlan, ""),
        ],
        Provider::ClaudeCode => &[
            (MetricKind::SubscriptionUsedPercent, "five_hour"),
            (MetricKind::SubscriptionUsedPercent, "seven_day"),
        ],
        Provider::Grok => &[
            (MetricKind::SubscriptionUsedPercent, "credits"),
            (MetricKind::SubscriptionPlan, ""),
        ],
        Provider::Vercel | Provider::Supabase | Provider::Cloudflare | Provider::Unspecified => &[],
    };
    Snapshot {
        observed_at: Some(time::timestamp(now)),
        status: fail.status().into(),
        failure_class: fail.class.into(),
        metrics: keys
            .iter()
            .map(|(k, d)| observation(*k, d, value(state, None)))
            .collect(),
        resources: vec![],
        provider_http_status: fail.http_status,
        source: ObservationSource::ActivePoll.into(),
        next_attempt_at: None,
    }
}
#[must_use]
pub fn outcome(
    provider: Provider,
    result: std::result::Result<Snapshot, Fail>,
    now: i64,
) -> Outcome {
    match result {
        Ok(snapshot) => Outcome {
            snapshot,
            ..Outcome::default()
        },
        Err(fail) => Outcome {
            retry_after: fail.retry_after,
            snapshot: failure_snapshot(provider, &fail, now),
            ..Outcome::default()
        },
    }
}
// Typed value helpers.
#[must_use]
pub fn percent(v: &Value) -> Option<metric_value::Value> {
    v.as_f64().map(metric_value::Value::Percent)
}
#[must_use]
pub fn text(v: &Value) -> Option<metric_value::Value> {
    v.as_str()
        .filter(|s| !s.is_empty())
        .map(|s| metric_value::Value::Text(s.to_owned()))
}
#[must_use]
pub fn unix_time(v: &Value) -> Option<metric_value::Value> {
    v.as_i64()
        .map(|s| metric_value::Value::Time(time::timestamp(s)))
}
#[must_use]
pub fn rfc3339(v: &Value) -> Option<metric_value::Value> {
    v.as_str()
        .and_then(time::parse)
        .map(|s| metric_value::Value::Time(time::timestamp(s)))
}
#[must_use]
pub const fn seconds(seconds: i64) -> metric_value::Value {
    metric_value::Value::Duration(prost_types::Duration { seconds, nanos: 0 })
}
#[must_use]
pub fn money(currency: &str, amount: String) -> metric_value::Value {
    metric_value::Value::Money(Money {
        currency_code: currency.to_owned(),
        amount,
    })
}
/// Decimal arithmetic in units of 10^-9 so sums keep exact decimal strings.
pub const SCALE: i128 = 1_000_000_000;
#[must_use]
pub fn decimal_units(v: &Value) -> Option<i128> {
    let text = match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().to_owned(),
        _ => return None,
    };
    let (negative, text) = text
        .strip_prefix('-')
        .map_or((false, text.as_str()), |t| (true, t));
    if text.contains(['e', 'E']) {
        let f = text.parse::<f64>().ok()?;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "Exponent notation is rounded to 10^-9 units."
        )]
        let units = (f * 1e9).round() as i128;
        return Some(if negative { -units } else { units });
    }
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let whole: i128 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let mut digits: String = fraction.chars().take(9).collect();
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    while digits.len() < 9 {
        digits.push('0');
    }
    let units = whole.checked_mul(SCALE)? + digits.parse::<i128>().ok()?;
    Some(if negative { -units } else { units })
}
#[must_use]
pub fn format_units(units: i128) -> String {
    let sign = if units < 0 { "-" } else { "" };
    let abs = units.unsigned_abs();
    let scale = SCALE.unsigned_abs();
    let whole = abs / scale;
    let fraction = format!("{:09}", abs % scale);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    }
}
/// Minor currency units (for example cents) as a major-unit decimal string.
#[must_use]
pub fn minor_units(v: &Value) -> Option<String> {
    let units = decimal_units(v)?;
    Some(format_units(units / 100))
}
/// Normalize a provider status into `ResourceHealth`.
#[must_use]
pub fn health(status: &str) -> ResourceHealth {
    let s = status.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| s.contains(w));
    if has(&["unhealthy"]) {
        ResourceHealth::Degraded
    } else if has(&["fail", "error", "cancel"]) {
        ResourceHealth::Failed
    } else if has(&["pause"]) {
        ResourceHealth::Paused
    } else if has(&["inactive", "removed"]) {
        ResourceHealth::Inactive
    } else if has(&["ready", "live", "healthy", "success"]) {
        ResourceHealth::Healthy
    } else if has(&[
        "build",
        "queue",
        "init",
        "restor",
        "coming_up",
        "upgrad",
        "restart",
        "resiz",
        "going_down",
        "active",
    ]) {
        ResourceHealth::InProgress
    } else {
        ResourceHealth::Unknown
    }
}
/// Run one adapter cycle for a connection. `secret` is the stored official token.
pub async fn poll(ctx: &Ctx, conn: &ConnectionRow, secret: Option<&str>, now: i64) -> Outcome {
    let provider = Provider::try_from(conn.provider).unwrap_or(Provider::Unspecified);
    match provider {
        Provider::Codex => codex::poll(ctx, conn, now).await,
        Provider::ClaudeCode => claude::poll(ctx, conn, now).await,
        Provider::Grok => grok::poll(ctx, now).await,
        Provider::Vercel | Provider::Supabase | Provider::Cloudflare => {
            let Some(secret) = secret else {
                return outcome(
                    provider,
                    Err(Fail::new(FailureClass::CredentialMissing)),
                    now,
                );
            };
            let result = match provider {
                Provider::Vercel => vercel::poll(ctx, secret, &conn.scope_id, now).await,
                Provider::Supabase => supabase::poll(ctx, secret, now).await,
                _ => cloudflare::poll(ctx, secret, now).await,
            };
            outcome(provider, result, now)
        }
        Provider::Unspecified => {
            outcome(provider, Err(Fail::new(FailureClass::ProviderError)), now)
        }
    }
}
/// Result of validating a credential before a connection is created or registered.
#[derive(Clone, Debug, Default)]
pub struct Validation {
    pub fingerprint: String,
    pub account_label: String,
}
/// Validate with one read-only request (official tokens) or by reading the local CLI login.
pub async fn validate(
    ctx: &Ctx,
    provider: Provider,
    secret: &str,
    scope: &str,
) -> std::result::Result<Validation, Fail> {
    match provider {
        Provider::Codex => codex::validate(ctx),
        Provider::ClaudeCode => claude::read(ctx).map(|_| Validation::default()),
        Provider::Grok => grok::validate(ctx),
        Provider::Vercel => vercel::validate(ctx, secret, scope).await,
        Provider::Supabase => supabase::validate(ctx, secret).await,
        Provider::Cloudflare => cloudflare::validate(ctx, secret).await,
        Provider::Unspecified => Err(Fail::new(FailureClass::ProviderError)),
    }
}
/// Whether this device can read a usable credential for the connection now.
///
/// An official token that authentication expiry stopped is not usable until the user reconnects
/// with a new token, which clears the stop.
#[must_use]
pub fn ready(ctx: &Ctx, conn: &ConnectionRow, has_secret: bool) -> bool {
    match Provider::try_from(conn.provider).unwrap_or(Provider::Unspecified) {
        Provider::Codex => codex::ready(ctx, conn),
        Provider::ClaudeCode => claude::read(ctx).is_ok(),
        Provider::Grok => grok::executable(ctx).is_some(),
        _ => has_secret && !conn.auth_stopped,
    }
}
