// SPDX-License-Identifier: Apache-2.0
//! Grok subscription usage through the official `grok` CLI.
//!
//! `grok agent stdio` speaks the Agent Client Protocol (newline-delimited JSON-RPC 2.0), and the
//! `x.ai/billing` extension method is sent with the ACP underscore prefix for custom methods.
//! AnyDash never handles the Grok token.
use super::{Builder, Ctx, Fail, Outcome, Validation, outcome};
use crate::{
    pb::{CollectionStatus, FailureClass, MetricKind, Provider, Snapshot, metric_value},
    schedule,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
/// The `grok` executable from the configured name or `PATH`.
#[must_use]
pub fn executable(ctx: &Ctx) -> Option<PathBuf> {
    let name = &ctx.endpoints.grok;
    if name.contains(['/', '\\']) {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    let separator = if cfg!(windows) { ';' } else { ':' };
    ctx.settings
        .var("PATH")
        .unwrap_or("")
        .split(separator)
        .filter(|d| !d.is_empty())
        .flat_map(|d| {
            let base = Path::new(d).join(name);
            [base.clone(), base.with_extension("exe")]
        })
        .find(|p| p.is_file())
}
/// A Grok connection is allowed without the `grok` CLI; its metrics then show "Not provided".
pub fn validate(ctx: &Ctx) -> Result<Validation, Fail> {
    if executable(ctx).is_none() {
        tracing::info!(
            event = "grok_cli_missing",
            "Grok metrics show Not provided until the grok CLI is installed."
        );
    }
    Ok(Validation::default())
}
async fn billing(exe: &Path) -> Result<Value, Fail> {
    let mut child = tokio::process::Command::new(exe)
        .args(["agent", "stdio"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Fail::new(FailureClass::CliUnavailable))?;
    let unreadable = || Fail::new(FailureClass::ResponseUnreadable);
    let mut stdin = child.stdin.take().ok_or_else(unreadable)?;
    let mut lines = BufReader::new(child.stdout.take().ok_or_else(unreadable)?).lines();
    let mut call = async |id: u64, method: &str, params: Value| -> Result<Value, Fail> {
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .map_err(|_| unreadable())?;
        stdin.flush().await.map_err(|_| unreadable())?;
        loop {
            let line = lines
                .next_line()
                .await
                .map_err(|_| unreadable())?
                .ok_or_else(unreadable)?;
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            // Agent requests and notifications are ignored; only our response counts.
            if message.get("method").is_some() || message["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                let text = error["message"].as_str().unwrap_or("").to_ascii_lowercase();
                let auth = error["code"].as_i64() == Some(-32000)
                    || text.contains("auth")
                    || text.contains("login");
                return Err(Fail::new(if auth {
                    FailureClass::AuthExpired
                } else {
                    FailureClass::ProviderError
                }));
            }
            return Ok(message["result"].clone());
        }
    };
    call(
        0,
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false}}),
    )
    .await?;
    let result = call(1, "_x.ai/billing", json!({})).await;
    let _ = child.kill().await;
    result
}
fn cents(v: &Value) -> Option<metric_value::Value> {
    if !v.is_object() {
        return None;
    }
    let units = v.get("val").map_or(Some(0), super::decimal_units)?;
    Some(super::money("USD", super::format_units(units / 100)))
}
fn not_provided(now: i64, status: CollectionStatus, class: FailureClass) -> Snapshot {
    let mut b = Builder::default();
    for kind in [
        MetricKind::SubscriptionUsedPercent,
        MetricKind::SubscriptionPeriodKind,
        MetricKind::SubscriptionWindowStart,
        MetricKind::SubscriptionResetAt,
    ] {
        b.not_provided(kind, "credits");
    }
    for kind in [
        MetricKind::SubscriptionOnDemandCapAmount,
        MetricKind::SubscriptionOnDemandUsedAmount,
        MetricKind::SubscriptionPlan,
        MetricKind::SubscriptionRemainingAmount,
        MetricKind::PlanDocumentedQuota,
    ] {
        b.not_provided(kind, "");
    }
    let mut s = b.finish(now);
    s.status = status.into();
    s.failure_class = class.into();
    s
}
/// Map an `x.ai/billing` result.
#[must_use]
pub fn map_billing(v: &Value, now: i64) -> Snapshot {
    let config = &v["config"];
    if !config.is_object() {
        // Team principals and accounts without a credits configuration.
        return not_provided(
            now,
            CollectionStatus::NotProvided,
            FailureClass::Unspecified,
        );
    }
    let mut b = Builder::default();
    b.maybe(
        MetricKind::SubscriptionUsedPercent,
        "credits",
        super::percent(&config["creditUsagePercent"]),
    );
    let period = &config["currentPeriod"];
    let kind = period["type"].as_str().map(str::to_ascii_lowercase);
    b.maybe(
        MetricKind::SubscriptionPeriodKind,
        "credits",
        kind.and_then(|k| {
            ["weekly", "monthly"]
                .into_iter()
                .find(|w| k.contains(w))
                .map(|w| metric_value::Value::Text(w.to_owned()))
        }),
    );
    b.maybe(
        MetricKind::SubscriptionWindowStart,
        "credits",
        super::rfc3339(&period["start"]),
    );
    b.maybe(
        MetricKind::SubscriptionResetAt,
        "credits",
        super::rfc3339(&period["end"]),
    );
    b.maybe(
        MetricKind::SubscriptionOnDemandCapAmount,
        "",
        cents(&config["onDemandCap"]),
    );
    b.maybe(
        MetricKind::SubscriptionOnDemandUsedAmount,
        "",
        cents(&config["onDemandUsed"]),
    );
    let tier = if v["subscription_tier"].is_string() {
        &v["subscription_tier"]
    } else {
        &v["subscriptionTier"]
    };
    b.maybe(MetricKind::SubscriptionPlan, "", super::text(tier));
    b.not_provided(MetricKind::SubscriptionRemainingAmount, "");
    b.not_provided(MetricKind::PlanDocumentedQuota, "");
    b.finish(now)
}
pub async fn poll(ctx: &Ctx, now: i64) -> Outcome {
    let Some(exe) = executable(ctx) else {
        return Outcome {
            snapshot: not_provided(
                now,
                CollectionStatus::NotProvided,
                FailureClass::CliUnavailable,
            ),
            ..Outcome::default()
        };
    };
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(schedule::REQUEST_TIMEOUT, billing(&exe))
        .await
        .unwrap_or_else(|_| Err(Fail::new(FailureClass::Timeout)));
    tracing::info!(
        event = "provider_request",
        endpoint = "grok.billing",
        result = result
            .as_ref()
            .map_or_else(|f| f.class.as_str_name(), |_| "ok"),
        duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
    );
    outcome(Provider::Grok, result.map(|v| map_billing(&v, now)), now)
}
