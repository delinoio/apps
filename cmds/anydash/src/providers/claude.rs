// SPDX-License-Identifier: Apache-2.0
//! Claude Code subscription usage from `GET /api/oauth/usage`. The stored claude.ai OAuth access
//! token is read in place on every cycle and never refreshed.
use super::{Builder, Ctx, Fail, Outcome, digest, failure_snapshot, outcome};
use crate::{
    pb::{CollectionStatus, FailureClass, MetricKind, Provider, Snapshot, metric_value},
    store::ConnectionRow,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
pub const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
#[derive(Clone, Debug)]
pub struct Login {
    pub access_token: String,
    /// `expiresAt` in Unix milliseconds.
    pub expires_at_ms: Option<i64>,
    /// A Console API key, Bedrock, Vertex or Foundry login without a claude.ai token.
    pub api_login: bool,
    pub marker: String,
}
/// The macOS Keychain service name; Claude Code appends the first 8 hex digits of the
/// SHA-256 of the configuration directory when `CLAUDE_CONFIG_DIR` is set.
#[must_use]
pub fn keychain_service(config_dir: Option<&str>) -> String {
    config_dir.map_or_else(
        || KEYCHAIN_SERVICE.to_owned(),
        |dir| {
            let digest = hex::encode(Sha256::digest(dir.as_bytes()));
            format!("{KEYCHAIN_SERVICE}-{}", &digest[..8])
        },
    )
}
fn parse(text: &str) -> Option<Login> {
    let v: Value = serde_json::from_str(text).ok()?;
    let oauth = &v["claudeAiOauth"];
    Some(
        oauth["accessToken"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map_or_else(
                || Login {
                    access_token: String::new(),
                    expires_at_ms: None,
                    api_login: true,
                    marker: String::new(),
                },
                |token| Login {
                    access_token: token.to_owned(),
                    expires_at_ms: oauth["expiresAt"].as_i64(),
                    api_login: false,
                    marker: digest(token),
                },
            ),
    )
}
/// Read the Claude Code login from the macOS Keychain or `.credentials.json`.
pub fn read(ctx: &Ctx) -> Result<Login, Fail> {
    if cfg!(target_os = "macos") {
        let first = ctx
            .settings
            .var("CLAUDE_CONFIG_DIR")
            .and_then(|v| v.split(',').map(str::trim).find(|s| !s.is_empty()));
        let account = ctx.settings.var("USER").unwrap_or("");
        if let Some(login) = ctx
            .os
            .get(&keychain_service(first), account)
            .ok()
            .flatten()
            .and_then(|t| parse(&t))
        {
            return Ok(login);
        }
    }
    ctx.settings
        .claude_dirs()
        .into_iter()
        .find_map(|(dir, _)| {
            std::fs::read_to_string(dir.join(".credentials.json"))
                .ok()
                .and_then(|t| parse(&t))
        })
        .ok_or_else(|| Fail::new(FailureClass::CredentialMissing))
}
/// Map an `/api/oauth/usage` response.
#[must_use]
pub fn map_usage(v: &Value, now: i64) -> Snapshot {
    let Some(object) = v.as_object() else {
        return failure_snapshot(
            Provider::ClaudeCode,
            &Fail::new(FailureClass::ResponseUnreadable),
            now,
        );
    };
    let mut b = Builder::default();
    let mut keys: Vec<&String> = object.keys().collect();
    keys.sort();
    for key in keys {
        let w = &object[key];
        if key == "extra_usage" {
            continue;
        }
        if w.is_null() && (key.starts_with("five_hour") || key.starts_with("seven_day")) {
            b.not_provided(MetricKind::SubscriptionUsedPercent, key);
            b.not_provided(MetricKind::SubscriptionResetAt, key);
        } else if w.get("utilization").is_some() {
            b.maybe(
                MetricKind::SubscriptionUsedPercent,
                key,
                super::percent(&w["utilization"]),
            );
            b.maybe(
                MetricKind::SubscriptionResetAt,
                key,
                super::rfc3339(&w["resets_at"]),
            );
        }
    }
    let extra = &v["extra_usage"];
    let kinds = [
        MetricKind::SubscriptionExtraUsageEnabled,
        MetricKind::SubscriptionExtraUsageUsedPercent,
        MetricKind::SubscriptionExtraUsageUsedAmount,
        MetricKind::SubscriptionExtraUsageLimitAmount,
    ];
    if extra.is_object() {
        let currency = extra["currency"].as_str().unwrap_or("");
        b.maybe(
            kinds[0],
            "",
            extra["is_enabled"].as_bool().map(metric_value::Value::Flag),
        );
        b.maybe(kinds[1], "", super::percent(&extra["utilization"]));
        // Credits are reported in minor currency units.
        b.maybe(
            kinds[2],
            "",
            super::minor_units(&extra["used_credits"]).map(|a| super::money(currency, a)),
        );
        b.maybe(
            kinds[3],
            "",
            super::minor_units(&extra["monthly_limit"]).map(|a| super::money(currency, a)),
        );
    } else {
        for kind in kinds {
            b.not_provided(kind, "");
        }
    }
    let plan = if v["plan_type"].is_string() {
        &v["plan_type"]
    } else {
        &v["subscription_type"]
    };
    b.maybe(MetricKind::SubscriptionPlan, "", super::text(plan));
    b.not_provided(MetricKind::SubscriptionRemainingAmount, "");
    b.not_provided(MetricKind::PlanDocumentedQuota, "");
    b.finish(now)
}
fn not_provided(now: i64) -> Snapshot {
    let mut b = Builder::default();
    for dim in ["five_hour", "seven_day"] {
        b.not_provided(MetricKind::SubscriptionUsedPercent, dim);
        b.not_provided(MetricKind::SubscriptionResetAt, dim);
    }
    for kind in [
        MetricKind::SubscriptionExtraUsageEnabled,
        MetricKind::SubscriptionPlan,
        MetricKind::SubscriptionRemainingAmount,
        MetricKind::PlanDocumentedQuota,
    ] {
        b.not_provided(kind, "");
    }
    let mut s = b.finish(now);
    s.status = CollectionStatus::NotProvided.into();
    s
}
pub async fn poll(ctx: &Ctx, conn: &ConnectionRow, now: i64) -> Outcome {
    let login = match read(ctx) {
        Ok(login) => login,
        Err(fail) => return outcome(Provider::ClaudeCode, Err(fail), now),
    };
    if login.api_login {
        return Outcome {
            snapshot: not_provided(now),
            ..Outcome::default()
        };
    }
    let marker = login.marker.clone();
    let unchanged = conn.auth_stopped && marker == conn.credential_marker;
    if unchanged || login.expires_at_ms.is_some_and(|ms| ms <= now * 1000) {
        let mut o = outcome(
            Provider::ClaudeCode,
            Err(Fail::new(FailureClass::AuthExpired)),
            now,
        );
        o.skipped = unchanged;
        o.marker = marker;
        return o;
    }
    let request = ctx
        .http
        .get(format!("{}/api/oauth/usage", ctx.endpoints.anthropic))
        .bearer_auth(&login.access_token)
        .header("anthropic-beta", "oauth-2025-04-20");
    let result = match super::send("claude.oauth_usage", request).await {
        Ok(response) => super::json(&response).map(|v| map_usage(&v, now)),
        Err(fail) => Err(fail),
    };
    let mut o = outcome(Provider::ClaudeCode, result, now);
    o.marker = marker;
    o
}
