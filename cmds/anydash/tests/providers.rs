// SPDX-License-Identifier: Apache-2.0
//! Mocked-provider tests: success, "Not provided" fields, transient failures, HTTP 429 with
//! `Retry-After`, authentication expiry, automatic resume and no refresh-token requests.
mod support;
use anydash::{
    pb::{
        CollectionStatus, FailureClass, MetricKind, MetricState, Provider, ResourceHealth,
        Snapshot, metric_value::Value as V,
    },
    providers::{self, Ctx},
    schedule,
    store::ConnectionRow,
    time,
};
use serde_json::json;
use std::{path::Path, time::Duration};
use support::{Mock, Reply};
const NOW: i64 = 1_793_000_000;
fn row(provider: Provider) -> ConnectionRow {
    ConnectionRow {
        id: "c".into(),
        provider: provider.into(),
        scope_id: "team_1".into(),
        lease_held: true,
        ..ConnectionRow::default()
    }
}
fn metric<'a>(s: &'a Snapshot, kind: MetricKind, dimension: &str) -> &'a anydash::pb::MetricValue {
    s.metrics
        .iter()
        .find(|m| m.kind == kind as i32 && m.dimension == dimension)
        .and_then(|m| m.value.as_ref())
        .unwrap_or_else(|| panic!("missing {kind:?} {dimension}"))
}
fn not_provided(s: &Snapshot, kind: MetricKind, dimension: &str) -> bool {
    metric(s, kind, dimension).state == MetricState::NotProvided as i32
}
fn codex_auth(home: &Path, exp: i64) -> String {
    let token = support::jwt(exp);
    support::write(
        &home.join(".codex/auth.json"),
        &json!({"OPENAI_API_KEY": null, "tokens": {"id_token": "x", "access_token": token, "refresh_token": "rt-never-used", "account_id": "acct-1"}, "last_refresh": "2026-10-01T00:00:00Z"}).to_string(),
    );
    token
}
fn wham() -> serde_json::Value {
    json!({"plan_type": "prolite_future_plan", "rate_limit": {"allowed": true, "limit_reached": false,
        "primary_window": {"used_percent": 42, "limit_window_seconds": 18000, "reset_after_seconds": 100, "reset_at": NOW + 100},
        "secondary_window": null},
        "credits": {"has_credits": true, "unlimited": false, "balance": "12.50"},
        "additional_rate_limits": [{"limit_name": "Other", "metered_feature": "codex_other", "rate_limit": {"primary_window": {"used_percent": 7, "limit_window_seconds": 900, "reset_after_seconds": 1, "reset_at": NOW + 1}}}]})
}
#[tokio::test(flavor = "multi_thread")]
async fn codex_success_not_provided_and_headers() {
    let home = tempfile::tempdir().unwrap();
    let token = codex_auth(home.path(), NOW + 3600);
    let mock = Mock::start(|_| Reply::json(200, &wham())).await;
    let ctx = support::ctx(home.path(), &mock, "grok");
    let o = providers::poll(&ctx, &row(Provider::Codex), None, NOW).await;
    let s = &o.snapshot;
    assert_eq!(s.status, CollectionStatus::Succeeded as i32);
    assert_eq!(
        metric(s, MetricKind::SubscriptionUsedPercent, "five_hour").value,
        Some(V::Percent(42.0))
    );
    assert_eq!(
        metric(s, MetricKind::SubscriptionWindowDuration, "five_hour").value,
        Some(providers::seconds(18000))
    );
    assert_eq!(
        metric(s, MetricKind::SubscriptionResetAt, "five_hour").value,
        Some(V::Time(time::timestamp(NOW + 100)))
    );
    assert!(not_provided(
        s,
        MetricKind::SubscriptionUsedPercent,
        "weekly"
    ));
    assert_eq!(
        metric(
            s,
            MetricKind::SubscriptionUsedPercent,
            "codex_other.five_hour"
        )
        .value,
        Some(V::Percent(7.0))
    );
    // Unknown plan types are reported as text.
    assert_eq!(
        metric(s, MetricKind::SubscriptionPlan, "").value,
        Some(V::Text("prolite_future_plan".into()))
    );
    assert_eq!(
        metric(s, MetricKind::SubscriptionCreditsBalance, "").value,
        Some(V::Decimal("12.50".into()))
    );
    assert!(not_provided(s, MetricKind::SubscriptionRemainingAmount, ""));
    assert!(not_provided(s, MetricKind::PlanDocumentedQuota, ""));
    assert!(
        s.metrics
            .iter()
            .all(|m| m.kind != MetricKind::SubscriptionRemainingPercent as i32)
    );
    let requests = mock.recorded();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].route(), "/backend-api/wham/usage");
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {token}").as_str())
    );
    assert_eq!(requests[0].header("chatgpt-account-id"), Some("acct-1"));
    assert!(requests[0].header("cookie").is_none());
    mock.assert_no_refresh_requests();
}
#[tokio::test(flavor = "multi_thread")]
async fn codex_login_of_another_account_is_not_polled() {
    let home = tempfile::tempdir().unwrap();
    codex_auth(home.path(), NOW + 3600);
    let mock = Mock::start(|_| Reply::json(200, &wham())).await;
    let ctx = support::ctx(home.path(), &mock, "grok");
    // The connection was added for another account; the CLI is now signed in to `acct-1`.
    let mut other = row(Provider::Codex);
    other.account_fingerprint = providers::fingerprint(Provider::Codex, "acct-2");
    assert!(!providers::ready(&ctx, &other, false));
    let o = providers::poll(&ctx, &other, None, NOW).await;
    assert!(o.account_mismatch);
    assert_eq!(o.snapshot.status, CollectionStatus::AuthExpired as i32);
    assert_eq!(
        o.snapshot.failure_class,
        FailureClass::CredentialMissing as i32
    );
    assert_eq!(
        mock.count("/backend-api/wham/usage"),
        0,
        "the other account's usage is never requested"
    );
    // The connection's own account is polled; a connection without a fingerprint accepts any.
    let mut own = row(Provider::Codex);
    own.account_fingerprint = providers::fingerprint(Provider::Codex, "acct-1");
    assert!(providers::ready(&ctx, &own, false));
    for conn in [own, row(Provider::Codex)] {
        let o = providers::poll(&ctx, &conn, None, NOW).await;
        assert!(!o.account_mismatch);
        assert_eq!(o.snapshot.status, CollectionStatus::Succeeded as i32);
    }
    assert_eq!(mock.count("/backend-api/wham/usage"), 2);
}
#[tokio::test(flavor = "multi_thread")]
async fn codex_api_key_login_is_not_provided() {
    let home = tempfile::tempdir().unwrap();
    support::write(
        &home.path().join(".codex/auth.json"),
        &json!({"OPENAI_API_KEY": "sk-test"}).to_string(),
    );
    let mock = Mock::start(|_| Reply::json(200, &wham())).await;
    let o = providers::poll(
        &support::ctx(home.path(), &mock, "grok"),
        &row(Provider::Codex),
        None,
        NOW,
    )
    .await;
    assert_eq!(o.snapshot.status, CollectionStatus::NotProvided as i32);
    assert!(not_provided(
        &o.snapshot,
        MetricKind::SubscriptionUsedPercent,
        "five_hour"
    ));
    assert_eq!(mock.recorded().len(), 0);
}
#[tokio::test(flavor = "multi_thread")]
async fn local_cli_auth_expiry_stops_and_resumes_after_cli_refresh() {
    for provider in [Provider::Codex, Provider::ClaudeCode] {
        let home = tempfile::tempdir().unwrap();
        let write = |exp: i64| -> String {
            if provider == Provider::Codex {
                codex_auth(home.path(), exp)
            } else {
                let token = format!("sk-ant-oat-{exp}");
                support::write(
                    &home.path().join(".claude/.credentials.json"),
                    &json!({"claudeAiOauth": {"accessToken": token, "refreshToken": "never-used", "expiresAt": exp * 1000, "scopes": ["user:inference"]}}).to_string(),
                );
                token
            }
        };
        write(NOW + 3600);
        let mock = Mock::start(|_| Reply::text(401, "unauthorized")).await;
        let ctx = support::ctx(home.path(), &mock, "grok");
        let mut conn = row(provider);
        let o = providers::poll(&ctx, &conn, None, NOW).await;
        assert_eq!(
            (o.snapshot.status, o.snapshot.failure_class),
            (
                CollectionStatus::AuthExpired as i32,
                FailureClass::AuthExpired as i32
            )
        );
        assert_eq!(o.snapshot.provider_http_status, 401);
        assert_eq!(mock.recorded().len(), 1);
        conn.auth_stopped = true;
        conn.credential_marker = o.marker.clone();
        // While the credential is unchanged the provider is not called again.
        let again = providers::poll(&ctx, &conn, None, NOW + 300).await;
        assert!(again.skipped);
        assert_eq!(mock.recorded().len(), 1);
        // An expired credential written by the CLI is read but not sent.
        write(NOW - 10);
        let expired = providers::poll(&ctx, &conn, None, NOW + 600).await;
        assert_eq!(
            expired.snapshot.status,
            CollectionStatus::AuthExpired as i32
        );
        assert_eq!(mock.recorded().len(), 1);
        conn.credential_marker = expired.marker;
        // The CLI refreshes its own credential; polling resumes automatically.
        write(NOW + 7200);
        mock.set(move |r| {
            if provider == Provider::Codex {
                Reply::json(200, &wham())
            } else {
                assert_eq!(r.header("anthropic-beta"), Some("oauth-2025-04-20"));
                Reply::json(200, &json!({"five_hour": {"utilization": 12.5, "resets_at": "2026-11-01T05:00:00+00:00"}, "seven_day": null, "extra_usage": null}))
            }
        });
        let resumed = providers::poll(&ctx, &conn, None, NOW + 900).await;
        assert_eq!(
            resumed.snapshot.status,
            CollectionStatus::Succeeded as i32,
            "{provider:?}"
        );
        assert_eq!(mock.recorded().len(), 2);
        mock.assert_no_refresh_requests();
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn claude_usage_mapping() {
    let home = tempfile::tempdir().unwrap();
    support::write(
        &home.path().join(".claude/.credentials.json"),
        &json!({"claudeAiOauth": {"accessToken": "tok", "expiresAt": (NOW + 3600) * 1000}})
            .to_string(),
    );
    let body = json!({
        "five_hour": {"utilization": 30.0, "resets_at": "2026-11-01T05:00:00+00:00"},
        "seven_day": {"utilization": 50.0, "resets_at": "2026-11-05T00:00:00Z"},
        "seven_day_opus": null,
        "seven_day_sonnet": {"utilization": 5.0, "resets_at": null},
        "extra_usage": {"is_enabled": true, "monthly_limit": 10000, "used_credits": 8050.0, "utilization": 80.5, "currency": "USD"}
    });
    let mock = Mock::start(move |_| Reply::json(200, &body)).await;
    let o = providers::poll(
        &support::ctx(home.path(), &mock, "grok"),
        &row(Provider::ClaudeCode),
        None,
        NOW,
    )
    .await;
    let s = &o.snapshot;
    assert_eq!(s.status, CollectionStatus::Succeeded as i32);
    assert_eq!(
        metric(s, MetricKind::SubscriptionUsedPercent, "seven_day").value,
        Some(V::Percent(50.0))
    );
    assert!(not_provided(
        s,
        MetricKind::SubscriptionUsedPercent,
        "seven_day_opus"
    ));
    assert!(not_provided(
        s,
        MetricKind::SubscriptionResetAt,
        "seven_day_sonnet"
    ));
    assert_eq!(
        metric(s, MetricKind::SubscriptionExtraUsageEnabled, "").value,
        Some(V::Flag(true))
    );
    assert_eq!(
        metric(s, MetricKind::SubscriptionExtraUsageUsedAmount, "").value,
        Some(V::Money(anydash::pb::Money {
            currency_code: "USD".into(),
            amount: "80.5".into()
        }))
    );
    assert_eq!(
        metric(s, MetricKind::SubscriptionExtraUsageLimitAmount, "").value,
        Some(V::Money(anydash::pb::Money {
            currency_code: "USD".into(),
            amount: "100".into()
        }))
    );
    assert!(not_provided(s, MetricKind::SubscriptionPlan, ""));
    assert_eq!(mock.recorded()[0].route(), "/api/oauth/usage");
}
#[tokio::test(flavor = "multi_thread")]
async fn rate_limits_transient_failures_and_timeouts() {
    let home = tempfile::tempdir().unwrap();
    support::write(
        &home.path().join(".claude/.credentials.json"),
        &json!({"claudeAiOauth": {"accessToken": "tok", "expiresAt": (NOW + 3600) * 1000}})
            .to_string(),
    );
    let mock =
        Mock::start(|_| Reply::text(429, "slow down").with_header("Retry-After", "120")).await;
    let mut ctx = support::ctx(home.path(), &mock, "grok");
    let o = providers::poll(&ctx, &row(Provider::ClaudeCode), None, NOW).await;
    assert_eq!(
        (o.snapshot.status, o.retry_after),
        (
            CollectionStatus::RateLimited as i32,
            Some(Duration::from_secs(120))
        )
    );
    mock.set(|_| Reply::text(503, "down"));
    let o = providers::poll(&ctx, &row(Provider::ClaudeCode), None, NOW).await;
    assert_eq!(
        (
            o.snapshot.status,
            o.snapshot.failure_class,
            o.snapshot.provider_http_status
        ),
        (
            CollectionStatus::Failed as i32,
            FailureClass::ProviderError as i32,
            503
        )
    );
    // Last successful values stay on the server; failure snapshots mark the main metrics failed.
    assert_eq!(
        metric(
            &o.snapshot,
            MetricKind::SubscriptionUsedPercent,
            "five_hour"
        )
        .state,
        MetricState::Failed as i32
    );
    mock.set(|_| Reply::text(200, "{not json"));
    let o = providers::poll(&ctx, &row(Provider::ClaudeCode), None, NOW).await;
    assert_eq!(
        o.snapshot.failure_class,
        FailureClass::ResponseUnreadable as i32
    );
    mock.set(|_| Reply::text(403, "missing scope"));
    let o = providers::poll(&ctx, &row(Provider::ClaudeCode), None, NOW).await;
    assert_eq!(
        (o.snapshot.status, o.snapshot.failure_class),
        (
            CollectionStatus::Failed as i32,
            FailureClass::PermissionDenied as i32
        )
    );
    // The shared client times out after 30 seconds; a short client proves the classification.
    assert_eq!(schedule::REQUEST_TIMEOUT, Duration::from_secs(30));
    ctx.http = reqwest::Client::builder()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    mock.set(|_| Reply::text(200, "{}").delayed(Duration::from_secs(2)));
    let o = providers::poll(&ctx, &row(Provider::ClaudeCode), None, NOW).await;
    assert_eq!(o.snapshot.failure_class, FailureClass::Timeout as i32);
    let unreachable = Ctx {
        endpoints: anydash::providers::Endpoints::mock("http://127.0.0.1:9", "grok"),
        ..ctx.clone()
    };
    let o = providers::poll(&unreachable, &row(Provider::ClaudeCode), None, NOW).await;
    assert_eq!(o.snapshot.failure_class, FailureClass::Network as i32);
}
#[test]
fn backoff_is_exponential_with_jitter_and_capped() {
    let minute = Duration::from_secs(60);
    assert_eq!(schedule::backoff(1, 0.0), minute);
    assert_eq!(schedule::backoff(2, 0.0), 2 * minute);
    assert_eq!(schedule::backoff(5, 0.0), 16 * minute);
    assert_eq!(schedule::backoff(6, 0.0), 30 * minute);
    assert_eq!(schedule::backoff(60, 0.99), 30 * minute);
    assert_eq!(schedule::backoff(1, 0.5), Duration::from_secs(66));
    assert_eq!(schedule::backoff(4, 1.0), Duration::from_secs(576));
    for _ in 0..100 {
        let d = schedule::backoff(3, schedule::jitter());
        assert!(d >= 4 * minute && d <= Duration::from_secs(288));
    }
    assert_eq!(
        schedule::retry_after("17", 0),
        Some(Duration::from_secs(17))
    );
    let at = chrono::DateTime::parse_from_rfc2822("Sun, 01 Nov 2026 00:02:00 GMT")
        .unwrap()
        .timestamp();
    assert_eq!(
        schedule::retry_after("Sun, 01 Nov 2026 00:02:00 GMT", at - 90),
        Some(Duration::from_secs(90))
    );
    assert_eq!(
        schedule::manual_wait(100, 130),
        Some(Duration::from_secs(30))
    );
    assert_eq!(schedule::manual_wait(100, 160), None);
}
#[tokio::test(flavor = "multi_thread")]
async fn vercel_projects_plan_and_focus_charges() {
    let home = tempfile::tempdir().unwrap();
    let charges = [
        json!({"BilledCost": "1.25", "EffectiveCost": 1.0, "BillingCurrency": "USD", "ConsumedQuantity": "100", "ConsumedUnit": "GB-Hours", "ServiceName": "Function Duration", "Tags": {"ProjectId": "prj_1"}}),
        json!({"BilledCost": 0.75, "EffectiveCost": "0.5", "BillingCurrency": "USD", "ConsumedQuantity": 50, "ConsumedUnit": "GB-Hours", "ServiceName": "Function Duration", "Tags": {}}),
    ]
    .map(|c| c.to_string())
    .join("\n");
    let mock = Mock::start(move |r| match r.route() {
        "/v10/projects" => Reply::json(200, &json!({"projects": [{"id": "prj_1", "name": "site", "latestDeployments": [{"readyState": "READY"}]}, {"id": "prj_2", "name": "old", "paused": true}], "pagination": {"next": null}})),
        "/v2/teams/team_1" => Reply::json(200, &json!({"id": "team_1", "name": "Team", "billing": {"plan": "pro"}})),
        "/v1/billing/charges" => Reply::text(200, &charges),
        _ => Reply::text(404, ""),
    })
    .await;
    let o = providers::poll(
        &support::ctx(home.path(), &mock, "grok"),
        &row(Provider::Vercel),
        Some("vercel-token"),
        NOW,
    )
    .await;
    let s = &o.snapshot;
    assert_eq!(s.status, CollectionStatus::Succeeded as i32);
    assert_eq!(
        metric(s, MetricKind::DeploymentPlan, "").value,
        Some(V::Text("pro".into()))
    );
    assert!(not_provided(s, MetricKind::DeploymentOnDemandBudget, ""));
    assert!(not_provided(s, MetricKind::PlanDocumentedQuota, ""));
    assert_eq!(
        metric(s, MetricKind::DeploymentBilledCost, "total").value,
        Some(V::Money(anydash::pb::Money {
            currency_code: "USD".into(),
            amount: "2".into()
        }))
    );
    assert_eq!(
        metric(s, MetricKind::DeploymentUsageQuantity, "Function_Duration").value,
        Some(V::Decimal("150".into()))
    );
    let site = s
        .resources
        .iter()
        .find(|r| r.resource_id == "prj_1")
        .unwrap();
    assert_eq!(
        (
            site.health,
            site.provider_status.as_str(),
            site.parent_id.as_str()
        ),
        (ResourceHealth::Healthy as i32, "READY", "team_1")
    );
    assert!(
        site.metrics
            .iter()
            .any(|m| m.kind == MetricKind::DeploymentEffectiveCost as i32)
    );
    assert_eq!(
        s.resources
            .iter()
            .find(|r| r.resource_id == "prj_2")
            .unwrap()
            .health,
        ResourceHealth::Paused as i32
    );
    for r in mock.recorded() {
        assert_eq!(r.method, "GET", "only read endpoints are called");
        assert_eq!(r.header("authorization"), Some("Bearer vercel-token"));
    }
    // Expired official tokens report AUTH_EXPIRED.
    mock.set(|_| Reply::text(401, ""));
    let o = providers::poll(
        &support::ctx(home.path(), &mock, "grok"),
        &row(Provider::Vercel),
        Some("vercel-token"),
        NOW,
    )
    .await;
    assert_eq!(o.snapshot.status, CollectionStatus::AuthExpired as i32);
    // A whole-cycle failure of a deployment provider carries no metrics and no resources, so the
    // AnyDash API marks every earlier usage, cost and resource reading failed, not only the plan.
    assert!(o.snapshot.metrics.is_empty() && o.snapshot.resources.is_empty());
    mock.set(|_| Reply::text(503, ""));
    let o = providers::poll(
        &support::ctx(home.path(), &mock, "grok"),
        &row(Provider::Vercel),
        Some("vercel-token"),
        NOW,
    )
    .await;
    assert_eq!(o.snapshot.status, CollectionStatus::Failed as i32);
    assert!(o.snapshot.metrics.is_empty() && o.snapshot.resources.is_empty());
}
#[tokio::test(flavor = "multi_thread")]
async fn supabase_projects_usage_entitlements_and_no_cost() {
    let home = tempfile::tempdir().unwrap();
    let reference = "abcdefghijklmnopqrst";
    let mock = Mock::start(move |r| {
        let p = format!("/v1/projects/{reference}");
        match r.route() {
            "/v1/organizations" => Reply::json(200, &json!([{"id": "o", "slug": "org1", "name": "Org"}])),
            "/v1/organizations/org1" => Reply::json(200, &json!({"id": "o", "name": "Org", "plan": "pro", "opt_in_tags": [], "allowed_release_channels": []})),
            "/v1/organizations/org1/entitlements" => Reply::json(200, &json!({"entitlements": [
                {"feature": {"key": "log.retention_days", "type": "numeric"}, "hasAccess": true, "type": "numeric", "config": {"enabled": true, "value": 7, "unlimited": false, "unit": "days"}},
                {"feature": {"key": "custom_domain", "type": "boolean"}, "hasAccess": false, "type": "boolean", "config": {"enabled": false}}]})),
            "/v1/organizations/org1/projects" => Reply::json(200, &json!({"projects": []})),
            "/v1/projects" => Reply::json(200, &json!([{"id": "x", "ref": reference, "organization_slug": "org1", "name": "db", "status": "ACTIVE_HEALTHY"}])),
            route if route == format!("{p}/health") => Reply::json(200, &json!([{"name": "db", "healthy": false, "status": "UNHEALTHY"}])),
            route if route == format!("{p}/analytics/endpoints/usage.api-counts") => {
                assert!(r.path.contains("interval=7day"));
                Reply::json(200, &json!({"result": [{"timestamp": "2026-10-30T00:00:00Z", "total_auth_requests": 1, "total_realtime_requests": 2, "total_rest_requests": 3, "total_storage_requests": 4}, {"timestamp": "2026-10-31T00:00:00Z", "total_auth_requests": 1, "total_realtime_requests": 0, "total_rest_requests": 0, "total_storage_requests": 0}]}))
            }
            route if route == format!("{p}/analytics/endpoints/usage.api-requests-count") => Reply::json(200, &json!({"result": [{"count": 99}]})),
            route if route == format!("{p}/functions") => Reply::json(200, &json!([{"id": "f1", "slug": "hello", "name": "hello", "status": "ACTIVE", "version": 1, "created_at": 0, "updated_at": 0}])),
            route if route == format!("{p}/analytics/endpoints/functions.combined-stats") => {
                assert!(r.path.contains("interval=1day") && r.path.contains("function_id=f1"));
                Reply::json(200, &json!({"result": [{"count": 3, "avg_execution_time": 10.5}]}))
            }
            route if route == format!("{p}/config/disk/util") => Reply::json(200, &json!({"timestamp": "now", "metrics": {"fs_size_bytes": 200, "fs_avail_bytes": 150, "fs_used_bytes": 50}})),
            _ => Reply::text(404, ""),
        }
    })
    .await;
    let o = providers::poll(
        &support::ctx(home.path(), &mock, "grok"),
        &row(Provider::Supabase),
        Some("sbp_token"),
        NOW,
    )
    .await;
    let s = &o.snapshot;
    assert_eq!(
        s.status,
        CollectionStatus::Succeeded as i32,
        "{:?}",
        s.failure_class
    );
    assert_eq!(
        metric(s, MetricKind::DeploymentPlan, "org1").value,
        Some(V::Text("pro".into()))
    );
    assert_eq!(
        metric(
            s,
            MetricKind::DeploymentEntitlement,
            "org1:log.retention_days"
        )
        .value,
        Some(V::Decimal("7".into()))
    );
    assert_eq!(
        metric(s, MetricKind::DeploymentEntitlement, "org1:custom_domain").value,
        Some(V::Flag(false))
    );
    assert!(not_provided(s, MetricKind::DeploymentBilledCost, ""));
    let project = &s.resources[0];
    assert_eq!(
        (project.health, project.parent_id.as_str()),
        (ResourceHealth::Degraded as i32, "org1")
    );
    let quantity = |dim: &str| {
        project
            .metrics
            .iter()
            .find(|m| m.dimension == dim)
            .and_then(|m| m.value.clone())
            .and_then(|v| v.value)
    };
    assert_eq!(
        quantity("api_counts.total_auth_requests"),
        Some(V::Decimal("2".into()))
    );
    assert_eq!(
        quantity("functions.hello.count"),
        Some(V::Decimal("3".into()))
    );
    assert_eq!(quantity("disk"), Some(V::Percent(25.0)));
    assert!(mock.recorded().iter().all(|r| r.method == "GET"));
}
/// Replies for one Supabase organization with one project; `org_projects` answers the
/// organization project list.
fn supabase_reply(r: &support::Recorded, org_projects: fn() -> Reply) -> Reply {
    match r.route() {
        "/v1/organizations" => Reply::json(200, &json!([{"id": "o", "slug": "org1"}])),
        "/v1/organizations/org1" => Reply::json(200, &json!({"plan": "pro"})),
        "/v1/organizations/org1/entitlements" => Reply::json(200, &json!({"entitlements": []})),
        "/v1/organizations/org1/projects" => org_projects(),
        "/v1/projects" => Reply::json(
            200,
            &json!([{"ref": "p1", "organization_slug": "org1", "name": "db", "status": "ACTIVE_HEALTHY"}]),
        ),
        "/v1/projects/p1/functions" => Reply::json(200, &json!([])),
        _ => Reply::json(200, &json!({})),
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn supabase_organization_projects_failures_are_not_discarded() {
    let home = tempfile::tempdir().unwrap();
    let mock = Mock::start(|r| {
        supabase_reply(r, || {
            Reply::text(429, "slow down").with_header("Retry-After", "900")
        })
    })
    .await;
    let ctx = support::ctx(home.path(), &mock, "grok");
    let o = providers::poll(&ctx, &row(Provider::Supabase), Some("sbp_token"), NOW).await;
    assert_eq!(
        (o.snapshot.status, o.retry_after),
        (
            CollectionStatus::RateLimited as i32,
            Some(Duration::from_mins(15))
        ),
        "the rate limit window reaches the scheduler"
    );
    assert_eq!(
        mock.count("/v1/projects/p1/health"),
        0,
        "no further requests during the rate limit window"
    );
    // Other failures keep the projects but mark the snapshot partial.
    mock.set(|r| supabase_reply(r, || Reply::text(503, "down")));
    let o = providers::poll(&ctx, &row(Provider::Supabase), Some("sbp_token"), NOW).await;
    assert_eq!(
        (o.snapshot.status, o.snapshot.provider_http_status),
        (CollectionStatus::PartiallySucceeded as i32, 503)
    );
    assert_eq!(o.snapshot.resources.len(), 1);
}
#[tokio::test(flavor = "multi_thread")]
async fn cloudflare_resources_analytics_plans_and_enterprise_cost() {
    let home = tempfile::tempdir().unwrap();
    let mock = Mock::start(|r| match r.route() {
        "/client/v4/accounts" => Reply::json(200, &json!({"success": true, "result": [{"id": "acc1", "name": "A"}, {"id": "acc2", "name": "B"}]})),
        // A second account with a Worker of the same script name.
        "/client/v4/accounts/acc1/workers/scripts" | "/client/v4/accounts/acc2/workers/scripts" => Reply::json(200, &json!({"result": [{"id": "worker-a"}]})),
        "/client/v4/accounts/acc2/pages/projects" | "/client/v4/accounts/acc2/subscriptions" => Reply::json(200, &json!({"result": []})),
        "/client/v4/accounts/acc1/pages/projects" => Reply::json(200, &json!({"result": [{"id": "pg1", "name": "pages", "latest_deployment": {"latest_stage": {"name": "deploy", "status": "failure"}}}]})),
        "/client/v4/accounts/acc1/subscriptions" => Reply::json(200, &json!({"result": [{"rate_plan": {"id": "workers_paid", "public_name": "Workers Paid"}}]})),
        "/client/v4/graphql" => {
            assert_eq!(r.method, "POST");
            Reply::json(200, &json!({"data": {"viewer": {"accounts": [{
                "workers": [{"sum": {"requests": 10, "cpuTimeUs": 2500}, "dimensions": {"scriptName": "worker-a"}}],
                "r2": [{"sum": {"requests": 4}}], "d1": [], "kv": [{"sum": {"requests": 6}}]}]}}, "errors": null}))
        }
        // Enterprise accounts have no billable usage.
        "/client/v4/accounts/acc1/billable-usage" | "/client/v4/accounts/acc2/billable-usage" => {
            Reply::text(403, "")
        }
        _ => Reply::text(404, ""),
    })
    .await;
    let o = providers::poll(
        &support::ctx(home.path(), &mock, "grok"),
        &row(Provider::Cloudflare),
        Some("cf-token"),
        NOW,
    )
    .await;
    let s = &o.snapshot;
    assert_eq!(s.status, CollectionStatus::Succeeded as i32);
    assert_eq!(
        metric(s, MetricKind::DeploymentPlan, "acc1.workers_paid").value,
        Some(V::Text("Workers Paid".into()))
    );
    assert!(not_provided(s, MetricKind::DeploymentBilledCost, "acc1"));
    assert_eq!(
        metric(s, MetricKind::DeploymentUsageQuantity, "acc1.kv.requests").value,
        Some(V::Decimal("6".into()))
    );
    let mut ids: Vec<&str> = s.resources.iter().map(|r| r.resource_id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        ["acc1/worker-a", "acc2/worker-a", "pg1"],
        "resource IDs are unique"
    );
    for account in ["acc1", "acc2"] {
        let worker = s
            .resources
            .iter()
            .find(|r| r.resource_id == format!("{account}/worker-a"))
            .unwrap();
        assert_eq!(
            (worker.display_name.as_str(), worker.parent_id.as_str()),
            ("worker-a", account)
        );
        assert_eq!(worker.status_state, MetricState::NotProvided as i32);
        assert!(
            worker
                .metrics
                .iter()
                .any(|m| m.dimension == "workers.requests")
        );
    }
    assert!(
        !s.metrics.iter().any(|m| m.dimension.contains("workers.")),
        "Worker analytics attach to their resources"
    );
    assert_eq!(
        s.resources
            .iter()
            .find(|r| r.resource_id == "pg1")
            .unwrap()
            .health,
        ResourceHealth::Failed as i32
    );
    let mut b = anydash::providers::Builder::default();
    anydash::providers::cloudflare::map_billable(
        &mut b,
        "acc1",
        &json!({"result": [{"ServiceName": "Workers Standard", "BillingCurrency": "USD", "ContractedCost": 1.5}, {"ServiceName": "R2", "BillingCurrency": "USD", "ContractedCost": "0.25"}]}),
        (0, 1),
    );
    let s = b.finish(1);
    assert_eq!(
        metric(&s, MetricKind::DeploymentBilledCost, "acc1").value,
        Some(V::Money(anydash::pb::Money {
            currency_code: "USD".into(),
            amount: "1.75".into()
        }))
    );
}
#[cfg(unix)]
fn fake_grok(dir: &Path, response: &str) -> String {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join("grok");
    let script = format!(
        "#!/bin/sh\n[ \"$1 $2\" = \"agent stdio\" ] || exit 3\nread init\necho '{{\"jsonrpc\":\"2.0\",\"method\":\"session/notice\",\"params\":{{}}}}'\necho '{{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{{\"protocolVersion\":1}}}}'\nread billing\ncase \"$billing\" in *'_x.ai/billing'*) echo '{response}' ;; *) exit 4 ;; esac\nsleep 5\n"
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn grok_billing_over_acp() {
    let home = tempfile::tempdir().unwrap();
    let mock = Mock::start(|_| Reply::text(500, "")).await;
    let ok = json!({"jsonrpc": "2.0", "id": 1, "result": {"config": {"creditUsagePercent": 37.5, "currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "start": "2026-10-26T00:00:00Z", "end": "2026-11-02T00:00:00Z"}, "onDemandCap": {"val": 5000}, "onDemandUsed": {}}, "subscription_tier": "SuperGrok Heavy"}}).to_string();
    let exe = fake_grok(home.path(), &ok);
    let started = std::time::Instant::now();
    let o = providers::poll(
        &support::ctx(home.path(), &mock, &exe),
        &row(Provider::Grok),
        None,
        NOW,
    )
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the child process is terminated after the response"
    );
    let s = &o.snapshot;
    assert_eq!(s.status, CollectionStatus::Succeeded as i32);
    assert_eq!(
        metric(s, MetricKind::SubscriptionUsedPercent, "credits").value,
        Some(V::Percent(37.5))
    );
    assert_eq!(
        metric(s, MetricKind::SubscriptionPeriodKind, "credits").value,
        Some(V::Text("weekly".into()))
    );
    assert_eq!(
        metric(s, MetricKind::SubscriptionResetAt, "credits").value,
        Some(V::Time(time::timestamp(
            time::parse("2026-11-02T00:00:00Z").unwrap()
        )))
    );
    assert_eq!(
        metric(s, MetricKind::SubscriptionOnDemandCapAmount, "").value,
        Some(V::Money(anydash::pb::Money {
            currency_code: "USD".into(),
            amount: "50".into()
        }))
    );
    assert_eq!(
        metric(s, MetricKind::SubscriptionOnDemandUsedAmount, "").value,
        Some(V::Money(anydash::pb::Money {
            currency_code: "USD".into(),
            amount: "0".into()
        }))
    );
    assert_eq!(
        metric(s, MetricKind::SubscriptionPlan, "").value,
        Some(V::Text("SuperGrok Heavy".into()))
    );
    let unauthenticated = json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "Authentication required"}}).to_string();
    let exe = fake_grok(home.path(), &unauthenticated);
    let o = providers::poll(
        &support::ctx(home.path(), &mock, &exe),
        &row(Provider::Grok),
        None,
        NOW,
    )
    .await;
    assert_eq!(o.snapshot.status, CollectionStatus::AuthExpired as i32);
    // Subscription providers keep the failure state on their main keys.
    assert_eq!(
        metric(&o.snapshot, MetricKind::SubscriptionUsedPercent, "credits").state,
        MetricState::AuthExpired as i32
    );
    let o = providers::poll(
        &support::ctx(home.path(), &mock, "/nonexistent/grok"),
        &row(Provider::Grok),
        None,
        NOW,
    )
    .await;
    assert_eq!(
        (o.snapshot.status, o.snapshot.failure_class),
        (
            CollectionStatus::NotProvided as i32,
            FailureClass::CliUnavailable as i32
        )
    );
    assert!(not_provided(
        &o.snapshot,
        MetricKind::SubscriptionUsedPercent,
        "credits"
    ));
    assert!(
        mock.recorded().is_empty(),
        "AnyDash never calls a Grok endpoint itself"
    );
}
#[test]
fn fingerprints_and_health_normalization() {
    assert_eq!(providers::fingerprint(Provider::Vercel, ""), "");
    assert_eq!(
        providers::fingerprint(Provider::Vercel, "team_1"),
        providers::digest("PROVIDER_VERCEL:team_1")
    );
    for (status, health) in [
        ("READY", ResourceHealth::Healthy),
        ("ACTIVE_HEALTHY", ResourceHealth::Healthy),
        ("BUILDING", ResourceHealth::InProgress),
        ("COMING_UP", ResourceHealth::InProgress),
        ("ERROR", ResourceHealth::Failed),
        ("INIT_FAILED", ResourceHealth::Failed),
        ("CANCELED", ResourceHealth::Failed),
        ("ACTIVE_UNHEALTHY", ResourceHealth::Degraded),
        ("PAUSED", ResourceHealth::Paused),
        ("INACTIVE", ResourceHealth::Inactive),
        ("mystery", ResourceHealth::Unknown),
    ] {
        assert_eq!(providers::health(status), health, "{status}");
    }
    assert_eq!(
        providers::format_units(providers::decimal_units(&json!("-0.000000001")).unwrap()),
        "-0.000000001"
    );
    assert_eq!(providers::minor_units(&json!(1234)), Some("12.34".into()));
}
