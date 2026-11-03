// SPDX-License-Identifier: Apache-2.0
//! Supabase Management API: projects, health, usage counts, function statistics, disk
//! utilization, organization plans and entitlements. Supabase has no spend API.
use super::{Builder, Ctx, Fail, Validation, cumulative, health, observation, value};
use crate::pb::{
    MetricKind, MetricState, ResourceHealth, ResourceKind, ResourceObservation, Snapshot,
    metric_value,
};
use serde_json::Value;
use std::collections::BTreeMap;
const MAX_PROJECTS: usize = 50;
const MAX_FUNCTIONS: usize = 20;
const HEALTH_SERVICES: &str = "auth,db,realtime,rest,storage";
async fn get(ctx: &Ctx, token: &str, endpoint: &str, path: &str) -> Result<Value, Fail> {
    let request = ctx
        .http
        .get(format!("{}{path}", ctx.endpoints.supabase))
        .bearer_auth(token);
    super::json(&super::send(endpoint, request).await?)
}
pub async fn validate(ctx: &Ctx, token: &str) -> Result<Validation, Fail> {
    get(ctx, token, "supabase.organizations", "/v1/organizations").await?;
    Ok(Validation::default())
}
fn number_sum(rows: &Value, field: &str) -> Option<f64> {
    let rows = rows.as_array()?;
    Some(rows.iter().filter_map(|r| r[field].as_f64()).sum())
}
fn decimal(v: f64) -> metric_value::Value {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Usage counts are rounded to 10^-9 units."
    )]
    let units = (v * 1e9).round() as i128;
    metric_value::Value::Decimal(super::format_units(units))
}
/// Health from the project status, degraded when a service health check is unhealthy.
#[must_use]
pub fn project_health(status: &str, services: &Value) -> ResourceHealth {
    let base = health(status);
    let unhealthy = services
        .as_array()
        .is_some_and(|s| s.iter().any(|h| h["status"].as_str() == Some("UNHEALTHY")));
    if unhealthy && base == ResourceHealth::Healthy {
        ResourceHealth::Degraded
    } else {
        base
    }
}
fn entitlement(e: &Value) -> Option<metric_value::Value> {
    let config = &e["config"];
    if e["hasAccess"].as_bool() == Some(false) || config["enabled"].as_bool() == Some(false) {
        return Some(metric_value::Value::Flag(false));
    }
    if config["unlimited"].as_bool() == Some(true) {
        return Some(metric_value::Value::Text("unlimited".into()));
    }
    if let Some(v) = config["value"].as_f64() {
        return Some(decimal(v));
    }
    if let Some(set) = config["set"].as_array() {
        let items: Vec<&str> = set.iter().filter_map(Value::as_str).collect();
        return Some(metric_value::Value::Text(items.join(",")));
    }
    config["enabled"].as_bool().map(metric_value::Value::Flag)
}
#[expect(
    clippy::too_many_lines,
    reason = "Keep the per-project request sequence of one cycle together."
)]
pub async fn poll(ctx: &Ctx, token: &str, now: i64) -> Result<Snapshot, Fail> {
    let mut b = Builder::default();
    let organizations = get(ctx, token, "supabase.organizations", "/v1/organizations").await?;
    let mut projects: BTreeMap<String, Value> = BTreeMap::new();
    for p in get(ctx, token, "supabase.projects", "/v1/projects")
        .await?
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(r) = p["ref"].as_str() {
            projects.insert(r.to_owned(), p.clone());
        }
    }
    for org in organizations.as_array().into_iter().flatten() {
        let Some(slug) = org["slug"].as_str() else {
            continue;
        };
        match get(
            ctx,
            token,
            "supabase.organization",
            &format!("/v1/organizations/{slug}"),
        )
        .await
        {
            Ok(v) => b.maybe(MetricKind::DeploymentPlan, slug, super::text(&v["plan"])),
            Err(fail) => b.failed_soft(MetricKind::DeploymentPlan, slug, fail)?,
        }
        match get(
            ctx,
            token,
            "supabase.entitlements",
            &format!("/v1/organizations/{slug}/entitlements"),
        )
        .await
        {
            Ok(v) => {
                for e in v["entitlements"].as_array().into_iter().flatten() {
                    if let Some(key) = e["feature"]["key"].as_str() {
                        b.maybe(
                            MetricKind::DeploymentEntitlement,
                            &format!("{slug}:{key}"),
                            entitlement(e),
                        );
                    }
                }
            }
            Err(fail) => b.failed_soft(MetricKind::DeploymentEntitlement, slug, fail)?,
        }
        // Like the other optional requests, a failed organization project list stops the cycle
        // on rate limits and expiry and otherwise marks the snapshot partial.
        match get(
            ctx,
            token,
            "supabase.organization_projects",
            &format!("/v1/organizations/{slug}/projects"),
        )
        .await
        {
            Ok(v) => {
                for p in v["projects"].as_array().into_iter().flatten() {
                    if let Some(r) = p["ref"].as_str() {
                        let mut p = p.clone();
                        p["organization_slug"] = slug.into();
                        projects.entry(r.to_owned()).or_insert(p);
                    }
                }
            }
            Err(fail) => b.tolerate(fail)?,
        }
    }
    b.not_provided(MetricKind::PlanDocumentedQuota, "");
    b.not_provided(MetricKind::DeploymentBilledCost, "");
    for (reference, p) in projects.iter().take(MAX_PROJECTS) {
        let status = p["status"].as_str().unwrap_or("").to_owned();
        let base = format!("/v1/projects/{reference}");
        let services = match get(
            ctx,
            token,
            "supabase.health",
            &format!("{base}/health?services={HEALTH_SERVICES}"),
        )
        .await
        {
            Ok(v) => v,
            Err(fail) => {
                b.tolerate(fail)?;
                Value::Null
            }
        };
        let mut r = ResourceObservation {
            resource_id: reference.clone(),
            kind: ResourceKind::SupabaseProject.into(),
            display_name: p["name"].as_str().unwrap_or(reference).to_owned(),
            parent_id: p["organization_slug"].as_str().unwrap_or("").to_owned(),
            health: project_health(&status, &services).into(),
            status_state: if status.is_empty() {
                MetricState::NotProvided
            } else {
                MetricState::Value
            }
            .into(),
            provider_status: status,
            metrics: vec![],
        };
        let week = (now - 7 * 86_400, now);
        match get(
            ctx,
            token,
            "supabase.api_counts",
            &format!("{base}/analytics/endpoints/usage.api-counts?interval=7day"),
        )
        .await
        {
            Ok(v) => {
                for field in [
                    "total_auth_requests",
                    "total_realtime_requests",
                    "total_rest_requests",
                    "total_storage_requests",
                ] {
                    if let Some(sum) = number_sum(&v["result"], field) {
                        r.metrics.push(cumulative(
                            MetricKind::DeploymentUsageQuantity,
                            &format!("api_counts.{field}"),
                            decimal(sum),
                            "requests",
                            week,
                        ));
                    }
                }
            }
            Err(fail) => {
                b.tolerate(fail)?;
                r.metrics.push(observation(
                    MetricKind::DeploymentUsageQuantity,
                    "api_counts",
                    value(MetricState::Failed, None),
                ));
            }
        }
        match get(
            ctx,
            token,
            "supabase.api_requests_count",
            &format!("{base}/analytics/endpoints/usage.api-requests-count"),
        )
        .await
        {
            Ok(v) => {
                if let Some(count) = v["result"][0]["count"].as_f64() {
                    let mut o = observation(
                        MetricKind::DeploymentUsageQuantity,
                        "api_requests_count",
                        value(MetricState::Value, Some(decimal(count))),
                    );
                    "requests".clone_into(&mut o.unit);
                    r.metrics.push(o);
                }
            }
            Err(fail) => b.tolerate(fail)?,
        }
        match get(
            ctx,
            token,
            "supabase.functions",
            &format!("{base}/functions"),
        )
        .await
        {
            Ok(functions) => {
                let day = (now - 86_400, now);
                for f in functions
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(MAX_FUNCTIONS)
                {
                    let (Some(id), Some(slug)) = (f["id"].as_str(), f["slug"].as_str()) else {
                        continue;
                    };
                    // Function statistics use windows of 7 days or less; one day is requested.
                    match get(ctx, token, "supabase.function_stats", &format!("{base}/analytics/endpoints/functions.combined-stats?interval=1day&function_id={id}")).await {
                        Ok(v) => {
                            let mut sums: BTreeMap<String, f64> = BTreeMap::new();
                            for row in v["result"].as_array().into_iter().flatten() {
                                for (k, n) in row.as_object().into_iter().flatten() {
                                    if let Some(n) = n.as_f64() {
                                        *sums.entry(k.clone()).or_default() += n;
                                    }
                                }
                            }
                            for (k, n) in sums {
                                r.metrics.push(cumulative(MetricKind::DeploymentUsageQuantity, &format!("functions.{slug}.{k}"), decimal(n), "", day));
                            }
                        }
                        Err(fail) => b.tolerate(fail)?,
                    }
                }
            }
            Err(fail) => b.tolerate(fail)?,
        }
        match get(
            ctx,
            token,
            "supabase.disk_util",
            &format!("{base}/config/disk/util"),
        )
        .await
        {
            Ok(v) => {
                let m = &v["metrics"];
                match (m["fs_used_bytes"].as_f64(), m["fs_size_bytes"].as_f64()) {
                    (Some(used), Some(size)) if size > 0.0 => r.metrics.push(observation(
                        MetricKind::DeploymentDiskUtilization,
                        "disk",
                        value(
                            MetricState::Value,
                            Some(metric_value::Value::Percent(used / size * 100.0)),
                        ),
                    )),
                    _ => r.metrics.push(observation(
                        MetricKind::DeploymentDiskUtilization,
                        "disk",
                        value(MetricState::NotProvided, None),
                    )),
                }
            }
            Err(fail) => {
                b.tolerate(fail)?;
                r.metrics.push(observation(
                    MetricKind::DeploymentDiskUtilization,
                    "disk",
                    value(MetricState::Failed, None),
                ));
            }
        }
        b.resources.push(r);
    }
    Ok(b.finish(now))
}
