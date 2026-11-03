// SPDX-License-Identifier: Apache-2.0
//! Cloudflare deployment adapter.
//!
//! Reads accounts, Workers scripts, Pages projects, GraphQL Analytics usage, subscriptions and
//! billable usage. Account-level dimensions are prefixed with the account ID because one token
//! can span several accounts. Requests stay well within the Cloudflare API limits.
use super::{Builder, Ctx, Fail, Validation, cumulative, decimal_units, format_units, health};
use crate::{
    pb::{
        FailureClass, MetricKind, MetricState, ResourceKind, ResourceObservation, Snapshot,
        metric_value,
    },
    time,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
const MAX_ACCOUNTS: usize = 20;
/// The resource ID of a Workers script. Script names are unique only within an account, and
/// one token can span several accounts, so the ID carries the account ID.
#[must_use]
pub fn worker_resource_id(account: &str, script: &str) -> String {
    format!("{account}/{script}")
}
async fn get(ctx: &Ctx, token: &str, endpoint: &str, path: &str) -> Result<Value, Fail> {
    let request = ctx
        .http
        .get(format!("{}{path}", ctx.endpoints.cloudflare))
        .bearer_auth(token);
    super::json(&super::send(endpoint, request).await?)
}
pub async fn validate(ctx: &Ctx, token: &str) -> Result<Validation, Fail> {
    get(ctx, token, "cloudflare.accounts", "/accounts?per_page=50").await?;
    Ok(Validation::default())
}
const ANALYTICS: &str = "query($account: String!, $from: Time!, $to: Time!, $fromDate: Date!, $toDate: Date!) {
  viewer { accounts(filter: {accountTag: $account}) {
    workers: workersInvocationsAdaptive(limit: 1000, filter: {datetime_geq: $from, datetime_leq: $to}) { sum { requests cpuTimeUs } dimensions { scriptName } }
    r2: r2OperationsAdaptiveGroups(limit: 1000, filter: {datetime_geq: $from, datetime_leq: $to}) { sum { requests } }
    d1: d1AnalyticsAdaptiveGroups(limit: 1000, filter: {date_geq: $fromDate, date_leq: $toDate}) { sum { readQueries writeQueries rowsRead rowsWritten } }
    kv: kvOperationsAdaptiveGroups(limit: 1000, filter: {date_geq: $fromDate, date_leq: $toDate}) { sum { requests } }
  } }
}";
/// Map a GraphQL Analytics response for one account over the last 24 hours.
pub fn map_analytics(
    b: &mut Builder,
    account: &str,
    v: &Value,
    resources: &mut [ResourceObservation],
    period: (i64, i64),
) {
    if v["errors"].as_array().is_some_and(|e| !e.is_empty()) {
        b.mark_partial(Fail::new(FailureClass::ResponseUnreadable));
    }
    let a = &v["data"]["viewer"]["accounts"][0];
    let mut scripts: BTreeMap<String, (i128, i128)> = BTreeMap::new();
    for row in a["workers"].as_array().into_iter().flatten() {
        let name = row["dimensions"]["scriptName"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        let entry = scripts.entry(name).or_default();
        entry.0 += decimal_units(&row["sum"]["requests"]).unwrap_or(0);
        entry.1 += decimal_units(&row["sum"]["cpuTimeUs"]).unwrap_or(0);
    }
    for (name, (requests, cpu)) in scripts {
        let metrics = [
            cumulative(
                MetricKind::DeploymentUsageQuantity,
                "workers.requests",
                metric_value::Value::Decimal(format_units(requests)),
                "requests",
                period,
            ),
            cumulative(
                MetricKind::DeploymentUsageQuantity,
                "workers.cpu_time",
                metric_value::Value::Decimal(format_units(cpu)),
                "microseconds",
                period,
            ),
        ];
        let id = worker_resource_id(account, &name);
        match resources
            .iter_mut()
            .find(|r| r.kind == ResourceKind::CloudflareWorkerScript as i32 && r.resource_id == id)
        {
            Some(r) => r.metrics.extend(metrics),
            None => b.metrics.extend(metrics.map(|mut m| {
                m.dimension = format!("{account}.{}.{name}", m.dimension);
                m
            })),
        }
    }
    let datasets: [(&str, &[(&str, &str)]); 3] = [
        ("r2", &[("requests", "operations")]),
        (
            "d1",
            &[
                ("readQueries", "queries"),
                ("writeQueries", "queries"),
                ("rowsRead", "rows"),
                ("rowsWritten", "rows"),
            ],
        ),
        ("kv", &[("requests", "operations")]),
    ];
    for (dataset, fields) in datasets {
        for (field, unit) in fields {
            let mut total = 0;
            let mut seen = false;
            for row in a[dataset].as_array().into_iter().flatten() {
                if let Some(n) = decimal_units(&row["sum"][field]) {
                    total += n;
                    seen = true;
                }
            }
            if seen || a[dataset].is_array() {
                b.metrics.push(cumulative(
                    MetricKind::DeploymentUsageQuantity,
                    &format!("{account}.{dataset}.{field}"),
                    metric_value::Value::Decimal(format_units(total)),
                    unit,
                    period,
                ));
            }
        }
    }
}
/// Map billable usage rows: `ContractedCost` summed by service and in total, month to date.
pub fn map_billable(b: &mut Builder, account: &str, v: &Value, period: (i64, i64)) {
    let mut by_service: BTreeMap<String, (i128, String)> = BTreeMap::new();
    let mut total = (0, String::new());
    for row in v["result"].as_array().into_iter().flatten() {
        let Some(cost) = decimal_units(&row["ContractedCost"]) else {
            b.mark_partial(Fail::new(FailureClass::ResponseUnreadable));
            continue;
        };
        let currency = row["BillingCurrency"].as_str().unwrap_or("").to_owned();
        let service = row["ServiceName"]
            .as_str()
            .unwrap_or("unknown")
            .replace(char::is_whitespace, "_");
        let entry = by_service
            .entry(service)
            .or_insert_with(|| (0, currency.clone()));
        entry.0 += cost;
        total.0 += cost;
        if total.1.is_empty() {
            total.1 = currency;
        }
    }
    for (service, (cost, currency)) in by_service {
        b.metrics.push(cumulative(
            MetricKind::DeploymentBilledCost,
            &format!("{account}.{service}"),
            super::money(&currency, format_units(cost)),
            "",
            period,
        ));
    }
    b.metrics.push(cumulative(
        MetricKind::DeploymentBilledCost,
        account,
        super::money(&total.1, format_units(total.0)),
        "",
        period,
    ));
}
#[expect(
    clippy::too_many_lines,
    reason = "Keep the per-account request sequence of one cycle together."
)]
pub async fn poll(ctx: &Ctx, token: &str, now: i64) -> Result<Snapshot, Fail> {
    let accounts = get(ctx, token, "cloudflare.accounts", "/accounts?per_page=50").await?;
    let mut b = Builder::default();
    b.not_provided(MetricKind::PlanDocumentedQuota, "");
    for account in accounts["result"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_ACCOUNTS)
    {
        let Some(id) = account["id"].as_str() else {
            continue;
        };
        let mut resources = vec![];
        match get(
            ctx,
            token,
            "cloudflare.workers_scripts",
            &format!("/accounts/{id}/workers/scripts"),
        )
        .await
        {
            Ok(v) => {
                for s in v["result"].as_array().into_iter().flatten() {
                    if let Some(name) = s["id"].as_str() {
                        resources.push(ResourceObservation {
                            resource_id: worker_resource_id(id, name),
                            kind: ResourceKind::CloudflareWorkerScript.into(),
                            display_name: name.to_owned(),
                            parent_id: id.to_owned(),
                            health: crate::pb::ResourceHealth::Unknown.into(),
                            provider_status: String::new(),
                            // Workers have no deployment status in the API.
                            status_state: MetricState::NotProvided.into(),
                            metrics: vec![],
                        });
                    }
                }
            }
            Err(fail) => b.tolerate(fail)?,
        }
        match get(
            ctx,
            token,
            "cloudflare.pages_projects",
            &format!("/accounts/{id}/pages/projects"),
        )
        .await
        {
            Ok(v) => {
                for p in v["result"].as_array().into_iter().flatten() {
                    let Some(name) = p["name"].as_str() else {
                        continue;
                    };
                    let status = p["latest_deployment"]["latest_stage"]["status"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned();
                    resources.push(ResourceObservation {
                        resource_id: p["id"].as_str().unwrap_or(name).to_owned(),
                        kind: ResourceKind::CloudflarePagesProject.into(),
                        display_name: name.to_owned(),
                        parent_id: id.to_owned(),
                        health: health(&status).into(),
                        status_state: if status.is_empty() {
                            MetricState::NotProvided
                        } else {
                            MetricState::Value
                        }
                        .into(),
                        provider_status: status,
                        metrics: vec![],
                    });
                }
            }
            Err(fail) => b.tolerate(fail)?,
        }
        match get(
            ctx,
            token,
            "cloudflare.subscriptions",
            &format!("/accounts/{id}/subscriptions"),
        )
        .await
        {
            Ok(v) => {
                for s in v["result"].as_array().into_iter().flatten() {
                    let plan = &s["rate_plan"];
                    let key = plan["id"].as_str().unwrap_or("plan");
                    b.maybe(
                        MetricKind::DeploymentPlan,
                        &format!("{id}.{key}"),
                        super::text(&plan["public_name"]),
                    );
                }
            }
            Err(fail) => b.failed_soft(MetricKind::DeploymentPlan, id, fail)?,
        }
        let day = (now - 86_400, now);
        let date = |t: i64| time::rfc3339(t)[..10].to_owned();
        let query = json!({"query": ANALYTICS, "variables": {
            "account": id, "from": time::rfc3339(day.0), "to": time::rfc3339(day.1),
            "fromDate": date(day.0), "toDate": date(day.1),
        }});
        let request = ctx
            .http
            .post(format!("{}/graphql", ctx.endpoints.cloudflare))
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .body(query.to_string());
        match super::send("cloudflare.graphql", request)
            .await
            .and_then(|r| super::json(&r))
        {
            Ok(v) => map_analytics(&mut b, id, &v, &mut resources, day),
            Err(fail) => b.tolerate(fail)?,
        }
        let month = (super::vercel::month_start(now), now);
        let path = format!(
            "/accounts/{id}/billable-usage?from={}&to={}",
            date(month.0),
            date(month.1)
        );
        match get(ctx, token, "cloudflare.billable_usage", &path).await {
            Ok(v) => map_billable(&mut b, id, &v, month),
            // Enterprise accounts and tokens without billing access have no billable usage.
            Err(fail) if matches!(fail.http_status, 400 | 403 | 404) => {
                b.not_provided(MetricKind::DeploymentBilledCost, id);
            }
            Err(fail) => b.failed_soft(MetricKind::DeploymentBilledCost, id, fail)?,
        }
        b.resources.extend(resources);
    }
    Ok(b.finish(now))
}
