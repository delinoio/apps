// SPDX-License-Identifier: Apache-2.0
//! Codex subscription usage from `GET /backend-api/wham/usage` and passive `rate_limits` in
//! local sessions. The Codex login is read in place and never refreshed.
use super::{Builder, Ctx, Fail, Outcome, Validation, digest, failure_snapshot, outcome};
use crate::{
    pb::{
        CollectionStatus, FailureClass, MetricKind, ObservationSource, Provider, Snapshot,
        metric_value,
    },
    store::ConnectionRow,
};
use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};
pub const KEYRING_SERVICE: &str = "Codex Auth";
#[derive(Clone, Debug)]
pub struct Login {
    pub access_token: String,
    pub account_id: String,
    pub expires_at: Option<i64>,
    /// An API-key login has no subscription quota.
    pub api_key_only: bool,
    pub marker: String,
}
/// The `exp` claim of a JWT, without verifying it.
#[must_use]
pub fn jwt_expiry(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()?["exp"].as_i64()
}
/// The keyring account the Codex CLI derives from its home directory.
#[must_use]
pub fn keyring_account(home: &std::path::Path) -> String {
    let canonical = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let digest = hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()));
    format!("cli|{}", &digest[..16])
}
/// Read the Codex login from `$CODEX_HOME/auth.json` or the `Codex Auth` keyring item.
pub fn read(ctx: &Ctx) -> Result<Login, Fail> {
    let (home, _) = ctx.settings.codex_home();
    let text = std::fs::read_to_string(home.join("auth.json"))
        .ok()
        .or_else(|| {
            ctx.os
                .get(KEYRING_SERVICE, &keyring_account(&home))
                .ok()
                .flatten()
        })
        .ok_or_else(|| Fail::new(FailureClass::CredentialMissing))?;
    let auth: Value =
        serde_json::from_str(&text).map_err(|_| Fail::new(FailureClass::CredentialMissing))?;
    let tokens = &auth["tokens"];
    match tokens["access_token"].as_str().filter(|t| !t.is_empty()) {
        Some(access) => Ok(Login {
            access_token: access.to_owned(),
            account_id: tokens["account_id"].as_str().unwrap_or("").to_owned(),
            expires_at: jwt_expiry(access),
            api_key_only: false,
            marker: digest(access),
        }),
        None if auth["OPENAI_API_KEY"]
            .as_str()
            .is_some_and(|k| !k.is_empty()) =>
        {
            Ok(Login {
                access_token: String::new(),
                account_id: String::new(),
                expires_at: None,
                api_key_only: true,
                marker: String::new(),
            })
        }
        None => Err(Fail::new(FailureClass::CredentialMissing)),
    }
}
/// Whether the Codex login belongs to the connection's account.
///
/// A connection without a stored fingerprint accepts any login. A different account, an API-key
/// login or a login without an account ID does not match, because its usage would be reported
/// under the wrong account.
#[must_use]
pub fn account_matches(login: &Login, expected: &str) -> bool {
    expected.is_empty() || super::fingerprint(Provider::Codex, &login.account_id) == expected
}
/// Whether this device can collect the connection: the Codex login is readable and belongs to
/// the connection's account.
#[must_use]
pub fn ready(ctx: &Ctx, conn: &ConnectionRow) -> bool {
    read(ctx).is_ok_and(|login| account_matches(&login, &conn.account_fingerprint))
}
pub fn validate(ctx: &Ctx) -> Result<Validation, Fail> {
    let login = read(ctx)?;
    Ok(Validation {
        fingerprint: super::fingerprint(Provider::Codex, &login.account_id),
        account_label: String::new(),
    })
}
fn window(b: &mut Builder, dimension: &str, w: &Value, passive: bool) {
    let kinds = [
        MetricKind::SubscriptionUsedPercent,
        MetricKind::SubscriptionWindowDuration,
        MetricKind::SubscriptionResetAt,
    ];
    if w.is_null() {
        for kind in kinds {
            b.not_provided(kind, dimension);
        }
        return;
    }
    let unreadable = Fail::new(FailureClass::ResponseUnreadable);
    if !w.is_object() {
        for kind in kinds {
            b.failed(kind, dimension, unreadable.clone());
        }
        return;
    }
    match w["used_percent"].as_f64() {
        Some(v) => b.set(kinds[0], dimension, metric_value::Value::Percent(v)),
        None => b.failed(kinds[0], dimension, unreadable),
    }
    let duration = if passive {
        w["window_minutes"].as_i64().map(|m| m * 60)
    } else {
        w["limit_window_seconds"].as_i64()
    };
    b.maybe(kinds[1], dimension, duration.map(super::seconds));
    let reset = if passive {
        &w["resets_at"]
    } else {
        &w["reset_at"]
    };
    b.maybe(kinds[2], dimension, super::unix_time(reset));
}
/// Map the rate limits of one snapshot. `limits` holds `(limit ID, primary, secondary)`.
fn map(limits: &[(String, Value, Value)], plan: &Value, credits: &Value, now: i64) -> Snapshot {
    let mut b = Builder::default();
    for (id, primary, secondary) in limits {
        let prefix = if id == "codex" {
            String::new()
        } else {
            format!("{id}.")
        };
        window(&mut b, &format!("{prefix}five_hour"), primary, false);
        window(&mut b, &format!("{prefix}weekly"), secondary, false);
    }
    // Unknown plan types are reported as text and never break parsing.
    b.maybe(MetricKind::SubscriptionPlan, "", super::text(plan));
    if credits.is_object() {
        b.maybe(
            MetricKind::SubscriptionCreditsBalance,
            "",
            credits["balance"]
                .as_str()
                .map(|s| metric_value::Value::Decimal(s.to_owned())),
        );
        b.maybe(
            MetricKind::SubscriptionCreditsUnlimited,
            "",
            credits["unlimited"]
                .as_bool()
                .map(metric_value::Value::Flag),
        );
    } else {
        b.not_provided(MetricKind::SubscriptionCreditsBalance, "");
        b.not_provided(MetricKind::SubscriptionCreditsUnlimited, "");
    }
    b.not_provided(MetricKind::SubscriptionRemainingAmount, "");
    b.not_provided(MetricKind::PlanDocumentedQuota, "");
    b.finish(now)
}
/// Map a `wham/usage` response.
#[must_use]
pub fn map_usage(v: &Value, now: i64) -> Snapshot {
    if !v.is_object() {
        return failure_snapshot(
            Provider::Codex,
            &Fail::new(FailureClass::ResponseUnreadable),
            now,
        );
    }
    let mut limits = vec![(
        "codex".to_owned(),
        v["rate_limit"]["primary_window"].clone(),
        v["rate_limit"]["secondary_window"].clone(),
    )];
    for extra in v["additional_rate_limits"].as_array().into_iter().flatten() {
        if let Some(id) = extra["metered_feature"].as_str() {
            limits.push((
                id.to_owned(),
                extra["rate_limit"]["primary_window"].clone(),
                extra["rate_limit"]["secondary_window"].clone(),
            ));
        }
    }
    map(&limits, &v["plan_type"], &v["credits"], now)
}
/// Map a passive `token_count.rate_limits` observation from a local session.
#[must_use]
pub fn map_passive(v: &Value, at: i64) -> Snapshot {
    let id = v["limit_id"].as_str().unwrap_or("codex").to_owned();
    let mut b = Builder::default();
    let prefix = if id == "codex" {
        String::new()
    } else {
        format!("{id}.")
    };
    window(&mut b, &format!("{prefix}five_hour"), &v["primary"], true);
    window(&mut b, &format!("{prefix}weekly"), &v["secondary"], true);
    b.maybe(
        MetricKind::SubscriptionPlan,
        "",
        super::text(&v["plan_type"]),
    );
    let mut snapshot = b.finish(at);
    snapshot.source = ObservationSource::PassiveSession.into();
    snapshot
}
fn not_provided(now: i64) -> Snapshot {
    let mut b = Builder::default();
    for dim in ["five_hour", "weekly"] {
        for kind in [
            MetricKind::SubscriptionUsedPercent,
            MetricKind::SubscriptionWindowDuration,
            MetricKind::SubscriptionResetAt,
        ] {
            b.not_provided(kind, dim);
        }
    }
    for kind in [
        MetricKind::SubscriptionPlan,
        MetricKind::SubscriptionCreditsBalance,
        MetricKind::SubscriptionCreditsUnlimited,
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
        Err(fail) => return outcome(Provider::Codex, Err(fail), now),
    };
    // The Codex CLI can be signed in to another account since the connection was added. Its
    // usage must not be reported under this connection: the credential of the connection's
    // account is missing on this device until the CLI is signed in to that account again.
    if !account_matches(&login, &conn.account_fingerprint) {
        tracing::warn!(
            event = "provider_account_mismatch",
            connection_id = %conn.id,
            provider = "codex",
            "The Codex CLI is signed in to a different account than the connection; the provider is not called."
        );
        let mut o = outcome(
            Provider::Codex,
            Err(Fail::new(FailureClass::CredentialMissing)),
            now,
        );
        o.account_mismatch = true;
        return o;
    }
    if login.api_key_only {
        return Outcome {
            snapshot: not_provided(now),
            ..Outcome::default()
        };
    }
    let marker = login.marker.clone();
    // An expired credential is read again every cycle; the provider is called only after the
    // Codex CLI writes a new, unexpired credential. AnyDash never refreshes it.
    if (conn.auth_stopped && marker == conn.credential_marker)
        || login.expires_at.is_some_and(|exp| exp <= now)
    {
        let mut o = outcome(
            Provider::Codex,
            Err(Fail::new(FailureClass::AuthExpired)),
            now,
        );
        o.skipped = conn.auth_stopped && marker == conn.credential_marker;
        o.marker = marker;
        return o;
    }
    let mut request = ctx
        .http
        .get(format!("{}/wham/usage", ctx.endpoints.codex))
        .bearer_auth(&login.access_token);
    if !login.account_id.is_empty() {
        request = request.header("ChatGPT-Account-Id", &login.account_id);
    }
    let result = match super::send("codex.wham_usage", request).await {
        Ok(response) => super::json(&response).map(|v| map_usage(&v, now)),
        Err(fail) => Err(fail),
    };
    let mut o = outcome(Provider::Codex, result, now);
    o.marker = marker;
    o
}
