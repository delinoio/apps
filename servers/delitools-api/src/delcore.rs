// SPDX-License-Identifier: FSL-1.1-ALv2
//! Delcore consumer client.
//!
//! DeliTools talks to Delcore only through its generic public Connect API (ProtoJSON over HTTP
//! at `/delcore.v1.DelcoreService/<Method>`): `Authorize` and `AuthorizeApiKey` for paid
//! execution, `GetCatalog` to verify that model meters have registered prices, `IngestEvent`
//! to deliver measured usage and `GetTenantStates` to drop usage of finally deleted tenants.
//! Service calls use a Logto client-credentials token with the `authorize` and `usage:write`
//! scopes, which is cached until shortly before it expires.
//!
//! Every failure is classified so callers can fail closed: anything that is not a clear,
//! well-formed answer is [`DelcoreError::Unavailable`].

use crate::config::{DelcoreSettings, Secret};
use base64::Engine;
use buffa::Message;
use serde_json::{Value, json};
use server_common::authority::{TenantState, bounded_json};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

/// Scopes requested for the DeliTools service token.
pub const SERVICE_SCOPES: &str = "authorize usage:write";
const MAX_CATALOG_PAGES: usize = 200;

/// Delcore `BlockReason` values DeliTools distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    Permission,
    Other,
}

/// Result of an authorization question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub allowed: bool,
    pub reasons: Vec<Block>,
    /// Attribution of a verified API key; empty for user-token checks.
    pub tenant_id: String,
}

/// Failure of a Delcore call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelcoreError {
    /// Delcore answered `unauthenticated`.
    Unauthenticated,
    /// Delcore answered `permission_denied`.
    PermissionDenied,
    /// Unreachable, timed out, malformed or any other answer. Callers fail closed.
    Unavailable,
}

/// How a failed `IngestEvent` should be handled by the delivery worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestFailure {
    /// Delivery may succeed later; retry with backoff.
    Retryable(&'static str),
    /// Delcore will never accept this exact event; stop retrying and raise an alert.
    Poison(&'static str),
}

impl IngestFailure {
    pub fn category(self) -> &'static str {
        match self {
            Self::Retryable(c) | Self::Poison(c) => c,
        }
    }
}

/// An event to deliver. The same values are sent on every retry.
#[derive(Clone, Debug)]
pub struct UsageEvent {
    pub request_key: String,
    pub tenant_id: String,
    pub event_key: String,
    pub meter_key: String,
    pub quantity: i64,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}

pub struct DelcoreClient {
    http: reqwest::Client,
    url: String,
    token_url: String,
    resource: String,
    client_id: String,
    client_secret: Secret,
    service_id: String,
    token: Mutex<Option<(String, Instant)>>,
}

impl DelcoreClient {
    pub fn new(settings: &DelcoreSettings, timeout: Duration) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            url: settings.url.trim_end_matches('/').to_owned(),
            token_url: settings.token_url.clone(),
            resource: settings.resource.clone(),
            client_id: settings.client_id.clone(),
            client_secret: settings.client_secret.clone(),
            service_id: settings.service_id.clone(),
            token: Mutex::new(None),
        })
    }

    /// Returns a cached or freshly issued service token.
    pub async fn service_token(&self) -> Result<String, DelcoreError> {
        let mut cache = self.token.lock().await;
        if let Some((token, expiry)) = &*cache
            && Instant::now() < *expiry
        {
            return Ok(token.clone());
        }
        let response = self
            .http
            .post(&self.token_url)
            .basic_auth(&self.client_id, Some(self.client_secret.expose()))
            .form(&[
                ("grant_type", "client_credentials"),
                ("resource", self.resource.as_str()),
                ("scope", SERVICE_SCOPES),
            ])
            .send()
            .await
            .map_err(|_| DelcoreError::Unavailable)?;
        if !response.status().is_success() {
            tracing::error!(
                category = "delcore_token",
                status = response.status().as_u16(),
                "service token request was rejected"
            );
            return Err(DelcoreError::Unavailable);
        }
        let body = bounded_json(response, 64 << 10)
            .await
            .map_err(|_| DelcoreError::Unavailable)?;
        let token = body["access_token"].as_str().unwrap_or("");
        if token.is_empty() {
            return Err(DelcoreError::Unavailable);
        }
        let ttl = body["expires_in"].as_u64().unwrap_or(0);
        if ttl > 60
            && let Some(at) = Instant::now().checked_add(Duration::from_secs(ttl - 30))
        {
            *cache = Some((token.to_owned(), at));
        }
        Ok(token.to_owned())
    }

    async fn invalidate_token(&self) {
        *self.token.lock().await = None;
    }

    /// Sends one service-authenticated call and returns the JSON body or the raw failure.
    async fn call(&self, method: &str, body: Value) -> Result<Value, (DelcoreError, Option<i32>)> {
        let token = self.service_token().await.map_err(|e| (e, None))?;
        let response = self
            .http
            .post(format!("{}/delcore.v1.DelcoreService/{method}", self.url))
            .bearer_auth(token)
            .header("Connect-Protocol-Version", "1")
            .json(&body)
            .send()
            .await
            .map_err(|_| (DelcoreError::Unavailable, None))?;
        let status = response.status();
        let value = bounded_json(response, 8 << 20)
            .await
            .map_err(|_| (DelcoreError::Unavailable, None))?;
        if status.is_success() {
            return Ok(value);
        }
        let reason = error_reason(&value);
        let kind = match value["code"].as_str() {
            Some("unauthenticated") => DelcoreError::Unauthenticated,
            Some("permission_denied") => DelcoreError::PermissionDenied,
            _ => DelcoreError::Unavailable,
        };
        Err((kind, reason))
    }

    /// Asks whether `user` may execute paid `capability` for `tenant`.
    pub async fn authorize(
        &self,
        tenant_id: &str,
        user_id: &str,
        capability: &str,
    ) -> Result<Verdict, DelcoreError> {
        let value = self
            .call(
                "Authorize",
                json!({"tenantId":tenant_id,"userId":user_id,"capability":capability,"paid":true}),
            )
            .await
            .map_err(|(e, _)| e)?;
        parse_verdict(&value)
    }

    /// Verifies a `dlk_` API key for this service and asks whether paid `capability` is allowed.
    ///
    /// Delcore answers `unauthenticated` for malformed, unknown, revoked and expired secrets, so
    /// attribution is only present for a valid key. The cached service token is dropped on that
    /// answer because Delcore cannot tell a bad secret from a rejected service token.
    pub async fn authorize_api_key(
        &self,
        secret: &str,
        capability: &str,
    ) -> Result<Verdict, DelcoreError> {
        match self
            .call(
                "AuthorizeApiKey",
                json!({"secret":secret,"capability":capability,"paid":true}),
            )
            .await
        {
            Ok(value) => {
                let verdict = parse_verdict(&value)?;
                if verdict.tenant_id.is_empty() || !server_common::canonical_id(&verdict.tenant_id)
                {
                    return Err(DelcoreError::Unavailable);
                }
                Ok(verdict)
            }
            Err((DelcoreError::Unauthenticated, _)) => {
                self.invalidate_token().await;
                Err(DelcoreError::Unauthenticated)
            }
            Err((e, _)) => Err(e),
        }
    }

    /// Returns the meter keys of this service that have an active usage price.
    ///
    /// Reads `GetCatalog` pages with the service token; Delcore exposes prices to every
    /// authenticated caller, and DeliTools never writes prices.
    pub async fn priced_meters(&self) -> Result<HashSet<String>, DelcoreError> {
        let mut priced = HashSet::new();
        let mut after = String::new();
        for _ in 0..MAX_CATALOG_PAGES {
            let request = if after.is_empty() {
                json!({"page":{"size":100}})
            } else {
                json!({"page":{"size":100,"afterId":after}})
            };
            let value = self.call("GetCatalog", request).await.map_err(|(e, _)| e)?;
            if !value.is_object() {
                return Err(DelcoreError::Unavailable);
            }
            for price in value["prices"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                if price["serviceId"].as_str() == Some(self.service_id.as_str())
                    && is_usage_kind(&price["kind"])
                    && price["active"].as_bool() == Some(true)
                    && price["unitPrice"].as_str().is_some_and(valid_price)
                    && let Some(meter) = price["meter"].as_str()
                {
                    priced.insert(meter.to_owned());
                }
            }
            match value["nextAfterId"].as_str().filter(|s| !s.is_empty()) {
                Some(next) if next != after => after = next.to_owned(),
                Some(_) => return Err(DelcoreError::Unavailable),
                None => return Ok(priced),
            }
        }
        Err(DelcoreError::Unavailable)
    }

    /// Delivers one usage event. Success includes an identical replay of an accepted event.
    pub async fn ingest_event(&self, event: &UsageEvent) -> Result<(), IngestFailure> {
        let body = json!({
            "requestKey": event.request_key,
            "event": {
                "tenantId": event.tenant_id,
                "serviceId": self.service_id,
                "eventKey": event.event_key,
                "eventType": event.meter_key,
                "occurredAt": event.occurred_at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
                "attributes": {"quantity": {"number": event.quantity.to_string()}},
            }
        });
        match self.call("IngestEvent", body).await {
            Ok(value) if value.is_object() => Ok(()),
            Ok(_) => Err(IngestFailure::Retryable("malformed_response")),
            Err((kind, reason)) => Err(classify_ingest_failure(kind, reason)),
        }
    }

    /// Reads tenant lifecycle states, one entry per requested ID. Delcore accepts 1 to 100
    /// IDs per call, so larger inputs are read in chunks.
    pub async fn tenant_states(
        &self,
        ids: &[String],
    ) -> Result<HashMap<String, TenantState>, DelcoreError> {
        let mut result = HashMap::new();
        for chunk in ids.chunks(TENANT_STATES_BATCH) {
            result.extend(self.tenant_states_chunk(chunk).await?);
        }
        Ok(result)
    }

    async fn tenant_states_chunk(
        &self,
        ids: &[String],
    ) -> Result<HashMap<String, TenantState>, DelcoreError> {
        let value = self
            .call("GetTenantStates", json!({"tenantIds":ids}))
            .await
            .map_err(|(e, _)| e)?;
        let states = value["states"]
            .as_array()
            .ok_or(DelcoreError::Unavailable)?;
        let mut result = HashMap::new();
        for entry in states {
            let id = entry["tenantId"]
                .as_str()
                .ok_or(DelcoreError::Unavailable)?;
            let state = match entry["state"].as_str() {
                Some("TENANT_STATE_ACTIVE") => TenantState::Active,
                Some("TENANT_STATE_BLOCKED") => TenantState::Blocked,
                Some("TENANT_STATE_DELETING") => TenantState::Deleting,
                Some("TENANT_STATE_DELETED") => TenantState::Deleted,
                Some("TENANT_STATE_UNKNOWN") => TenantState::Unknown,
                _ => return Err(DelcoreError::Unavailable),
            };
            if !ids.iter().any(|i| i == id) || result.insert(id.to_owned(), state).is_some() {
                return Err(DelcoreError::Unavailable);
            }
        }
        Ok(result)
    }
}

/// Delcore accepts at most this many tenant IDs in one `GetTenantStates` call.
const TENANT_STATES_BATCH: usize = 100;

fn is_usage_kind(kind: &Value) -> bool {
    kind.as_str() == Some("PRICE_KIND_USAGE") || kind.as_i64() == Some(1)
}

/// A selling price is a non-negative plain decimal such as `0.000002`.
pub fn valid_price(value: &str) -> bool {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    !whole.is_empty()
        && whole.len() <= 30
        && fraction.len() <= 30
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && (!value.contains('.') || !fraction.is_empty())
}

fn parse_block(value: &Value) -> Block {
    match value {
        Value::String(s) if s == "BLOCK_REASON_PERMISSION" => Block::Permission,
        Value::Number(n) if n.as_i64() == Some(1) => Block::Permission,
        _ => Block::Other,
    }
}

/// Interprets an `AuthorizeResponse` or `AuthorizeApiKeyResponse`.
///
/// ProtoJSON omits default values, so a missing `allowed` means `false`. An `allowed=false`
/// answer without any reason never occurs in a healthy Delcore and is treated as malformed.
pub fn parse_verdict(value: &Value) -> Result<Verdict, DelcoreError> {
    if !value.is_object() {
        return Err(DelcoreError::Unavailable);
    }
    let allowed = match value.get("allowed") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(DelcoreError::Unavailable),
    };
    let reasons: Vec<Block> = match value.get("reasons") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.iter().map(parse_block).collect(),
        Some(_) => return Err(DelcoreError::Unavailable),
    };
    if allowed == !reasons.is_empty() {
        // allowed with reasons, or denied without any reason.
        return Err(DelcoreError::Unavailable);
    }
    Ok(Verdict {
        allowed,
        reasons,
        tenant_id: value["tenantId"].as_str().unwrap_or("").to_owned(),
    })
}

/// Decodes the Delcore `ErrorDetail.reason` number from a Connect error body.
fn error_reason(body: &Value) -> Option<i32> {
    let engine = base64::engine::general_purpose::STANDARD_NO_PAD;
    for detail in body["details"].as_array()? {
        let Some(raw) = detail["value"].as_str() else {
            continue;
        };
        let Ok(bytes) = engine.decode(raw.trim_end_matches('=')) else {
            continue;
        };
        if let Ok(decoded) = delino_protocol::delcore::v1::ErrorDetail::decode_from_slice(&bytes) {
            return Some(decoded.reason.to_i32());
        }
    }
    None
}

/// Maps a Delcore failure to delivery handling.
///
/// Reasons that identify an event Delcore can never accept (`INVALID_INPUT`,
/// `INVALID_ATTRIBUTE`, `EVENT_IDENTITY_CONFLICT`, `REPORTING_WINDOW_CLOSED`) are poison.
/// Everything else, including a missing meter or a rejected service token, may be repaired by an
/// operator and is retried with backoff until the 30-day expiry.
pub fn classify_ingest_failure(kind: DelcoreError, reason: Option<i32>) -> IngestFailure {
    match reason {
        Some(1) => IngestFailure::Poison("invalid_input"),
        Some(9) => IngestFailure::Poison("invalid_attribute"),
        Some(11) => IngestFailure::Poison("identity_conflict"),
        Some(14) => IngestFailure::Poison("reporting_window_closed"),
        Some(8) => IngestFailure::Retryable("no_matching_meter"),
        Some(10) => IngestFailure::Retryable("cardinality_exceeded"),
        _ => IngestFailure::Retryable(match kind {
            DelcoreError::Unauthenticated => "unauthenticated",
            DelcoreError::PermissionDenied => "forbidden",
            DelcoreError::Unavailable => "unavailable",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_fail_closed_on_malformed_answers() {
        assert_eq!(
            parse_verdict(&json!({"allowed":true,"tenantId":"t"})).unwrap(),
            Verdict {
                allowed: true,
                reasons: vec![],
                tenant_id: "t".into()
            }
        );
        let denied = parse_verdict(&json!({"reasons":["BLOCK_REASON_PERMISSION"]})).unwrap();
        assert!(!denied.allowed);
        assert_eq!(denied.reasons, vec![Block::Permission]);
        let billing =
            parse_verdict(&json!({"allowed":false,"reasons":["BLOCK_REASON_BALANCE",1]})).unwrap();
        assert_eq!(billing.reasons, vec![Block::Other, Block::Permission]);
        for malformed in [
            json!([]),
            json!("x"),
            json!({}),
            json!({"allowed":"yes"}),
            json!({"allowed":true,"reasons":["BLOCK_REASON_BALANCE"]}),
            json!({"allowed":false}),
            json!({"reasons":"x"}),
        ] {
            assert_eq!(
                parse_verdict(&malformed),
                Err(DelcoreError::Unavailable),
                "{malformed}"
            );
        }
    }

    #[test]
    fn price_syntax_is_a_plain_non_negative_decimal() {
        for good in ["0", "1", "0.000002", "12.5", "100"] {
            assert!(valid_price(good), "{good}");
        }
        for bad in ["", "-1", "1.", ".5", "1e3", "NaN", "1,5", " 1", "1.2.3"] {
            assert!(!valid_price(bad), "{bad}");
        }
    }

    #[test]
    fn ingest_failures_are_classified() {
        use DelcoreError::*;
        assert_eq!(
            classify_ingest_failure(Unavailable, Some(11)),
            IngestFailure::Poison("identity_conflict")
        );
        assert_eq!(
            classify_ingest_failure(Unavailable, Some(14)),
            IngestFailure::Poison("reporting_window_closed")
        );
        assert_eq!(
            classify_ingest_failure(Unavailable, Some(9)),
            IngestFailure::Poison("invalid_attribute")
        );
        assert_eq!(
            classify_ingest_failure(Unavailable, Some(1)),
            IngestFailure::Poison("invalid_input")
        );
        assert_eq!(
            classify_ingest_failure(Unavailable, Some(8)),
            IngestFailure::Retryable("no_matching_meter")
        );
        assert_eq!(
            classify_ingest_failure(Unauthenticated, None),
            IngestFailure::Retryable("unauthenticated")
        );
        assert_eq!(
            classify_ingest_failure(PermissionDenied, None),
            IngestFailure::Retryable("forbidden")
        );
        assert_eq!(
            classify_ingest_failure(Unavailable, None),
            IngestFailure::Retryable("unavailable")
        );
    }

    #[test]
    fn error_reason_is_decoded_from_connect_details() {
        let detail = delino_protocol::delcore::v1::ErrorDetail {
            reason: delino_protocol::delcore::v1::ErrorReason::ERROR_REASON_EVENT_IDENTITY_CONFLICT
                .into(),
            correlation_id: "c".into(),
            ..Default::default()
        };
        let encoded =
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(detail.encode_to_vec());
        let body = json!({"code":"already_exists","details":[{"type":"delcore.v1.ErrorDetail","value":encoded}]});
        assert_eq!(error_reason(&body), Some(11));
        assert_eq!(error_reason(&json!({"code":"x"})), None);
        assert_eq!(error_reason(&json!({"details":[{"value":"!!"}]})), None);
    }
}
