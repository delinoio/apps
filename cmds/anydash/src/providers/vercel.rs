// SPDX-License-Identifier: Apache-2.0
//! Vercel projects, plan and FOCUS billing charges. Only read endpoints are called, although
//! Vercel tokens can write.
use super::{Builder, Ctx, Fail, Validation, cumulative, decimal_units, format_units, health};
use crate::{
    pb::{
        FailureClass, MetricKind, MetricState, Provider, ResourceKind, ResourceObservation,
        Snapshot, metric_value,
    },
    time,
};
use chrono::{Datelike as _, TimeZone as _};
use serde_json::Value;
use std::collections::BTreeMap;
const MAX_PAGES: usize = 10;
fn get(ctx: &Ctx, token: &str, path: &str) -> reqwest::RequestBuilder {
    ctx.http
        .get(format!("{}{path}", ctx.endpoints.vercel))
        .bearer_auth(token)
}
pub async fn validate(ctx: &Ctx, token: &str, team: &str) -> Result<Validation, Fail> {
    let v = super::json(
        &super::send("vercel.team", get(ctx, token, &format!("/v2/teams/{team}"))).await?,
    )?;
    Ok(Validation {
        fingerprint: super::fingerprint(Provider::Vercel, team),
        account_label: v["name"].as_str().unwrap_or("").to_owned(),
    })
}
/// Unix seconds of the start of the current UTC month.
#[must_use]
pub fn month_start(now: i64) -> i64 {
    let t = chrono::DateTime::from_timestamp(now, 0).unwrap_or_default();
    chrono::Utc
        .with_ymd_and_hms(t.year(), t.month(), 1, 0, 0, 0)
        .single()
        .map_or(now, |m| m.timestamp())
}
/// Map project list pages into resources.
#[must_use]
pub fn map_projects(projects: &[Value], team: &str) -> Vec<ResourceObservation> {
    projects
        .iter()
        .filter_map(|p| {
            let id = p["id"].as_str()?;
            let status = if p["paused"].as_bool() == Some(true) {
                "PAUSED".to_owned()
            } else {
                p["latestDeployments"][0]["readyState"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| (p["live"].as_bool() == Some(true)).then(|| "LIVE".to_owned()))
                    .unwrap_or_default()
            };
            Some(ResourceObservation {
                resource_id: id.to_owned(),
                kind: ResourceKind::VercelProject.into(),
                display_name: p["name"].as_str().unwrap_or(id).to_owned(),
                parent_id: team.to_owned(),
                health: health(&status).into(),
                status_state: if status.is_empty() {
                    MetricState::NotProvided
                } else {
                    MetricState::Value
                }
                .into(),
                provider_status: status,
                metrics: vec![],
            })
        })
        .collect()
}
#[derive(Default)]
struct Sum {
    billed: i128,
    effective: i128,
    quantity: i128,
    unit: String,
    currency: String,
}
/// Map FOCUS JSONL charges: costs and consumed quantities by service, by service and project,
/// and as totals. Lines that cannot be read make the cost metrics partial.
#[expect(
    clippy::too_many_lines,
    reason = "One pass sums every FOCUS dimension."
)]
pub fn map_charges(
    b: &mut Builder,
    body: &[u8],
    resources: &mut [ResourceObservation],
    period: (i64, i64),
) {
    let mut by_service: BTreeMap<String, Sum> = BTreeMap::new();
    let mut by_project: BTreeMap<(String, String), Sum> = BTreeMap::new();
    let mut total = Sum::default();
    let mut unreadable = false;
    for line in body.split(|c| *c == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(row) = serde_json::from_slice::<Value>(line) else {
            unreadable = true;
            continue;
        };
        let (Some(billed), Some(effective)) = (
            decimal_units(&row["BilledCost"]),
            decimal_units(&row["EffectiveCost"]),
        ) else {
            unreadable = true;
            continue;
        };
        let currency = row["BillingCurrency"].as_str().unwrap_or("").to_owned();
        let service = row["ServiceName"]
            .as_str()
            .unwrap_or("unknown")
            .replace(char::is_whitespace, "_");
        let quantity = decimal_units(&row["ConsumedQuantity"]);
        let unit = row["ConsumedUnit"].as_str().unwrap_or("").to_owned();
        let project = row["Tags"]["ProjectId"].as_str().map(str::to_owned);
        let add = |s: &mut Sum| {
            s.billed += billed;
            s.effective += effective;
            s.quantity += quantity.unwrap_or(0);
            if s.unit.is_empty() {
                s.unit.clone_from(&unit);
            }
            if s.currency.is_empty() {
                s.currency.clone_from(&currency);
            }
        };
        add(by_service.entry(service.clone()).or_default());
        add(&mut total);
        if let Some(project) = project {
            add(by_project.entry((project, service)).or_default());
        }
    }
    let cost = |s: &Sum, units: i128| super::money(&s.currency, format_units(units));
    for (service, s) in &by_service {
        b.metrics.push(cumulative(
            MetricKind::DeploymentBilledCost,
            service,
            cost(s, s.billed),
            "",
            period,
        ));
        b.metrics.push(cumulative(
            MetricKind::DeploymentEffectiveCost,
            service,
            cost(s, s.effective),
            "",
            period,
        ));
        b.metrics.push(cumulative(
            MetricKind::DeploymentUsageQuantity,
            service,
            metric_value::Value::Decimal(format_units(s.quantity)),
            &s.unit,
            period,
        ));
    }
    b.metrics.push(cumulative(
        MetricKind::DeploymentBilledCost,
        "total",
        cost(&total, total.billed),
        "",
        period,
    ));
    b.metrics.push(cumulative(
        MetricKind::DeploymentEffectiveCost,
        "total",
        cost(&total, total.effective),
        "",
        period,
    ));
    for ((project, service), s) in &by_project {
        if let Some(r) = resources.iter_mut().find(|r| &r.resource_id == project) {
            r.metrics.push(cumulative(
                MetricKind::DeploymentBilledCost,
                service,
                cost(s, s.billed),
                "",
                period,
            ));
            r.metrics.push(cumulative(
                MetricKind::DeploymentEffectiveCost,
                service,
                cost(s, s.effective),
                "",
                period,
            ));
            r.metrics.push(cumulative(
                MetricKind::DeploymentUsageQuantity,
                service,
                metric_value::Value::Decimal(format_units(s.quantity)),
                &s.unit,
                period,
            ));
        }
    }
    if unreadable {
        b.mark_partial(Fail::new(FailureClass::ResponseUnreadable));
    }
}
pub async fn poll(ctx: &Ctx, token: &str, team: &str, now: i64) -> Result<Snapshot, Fail> {
    let mut projects = vec![];
    let mut next: Option<i64> = None;
    for _ in 0..MAX_PAGES {
        let until = next.map_or_else(String::new, |u| format!("&until={u}"));
        let path = format!("/v10/projects?teamId={team}&limit=100{until}");
        let v = super::json(&super::send("vercel.projects", get(ctx, token, &path)).await?)?;
        let page = v["projects"]
            .as_array()
            .ok_or_else(|| Fail::new(FailureClass::ResponseUnreadable))?;
        projects.extend(page.iter().cloned());
        next = v["pagination"]["next"].as_i64();
        if next.is_none() {
            break;
        }
    }
    let mut b = Builder::default();
    let mut resources = map_projects(&projects, team);
    match super::send("vercel.team", get(ctx, token, &format!("/v2/teams/{team}")))
        .await
        .and_then(|r| super::json(&r))
    {
        Ok(v) => b.maybe(
            MetricKind::DeploymentPlan,
            "",
            super::text(&v["billing"]["plan"]),
        ),
        Err(fail) => b.failed_soft(MetricKind::DeploymentPlan, "", fail)?,
    }
    b.not_provided(MetricKind::PlanDocumentedQuota, "");
    b.not_provided(MetricKind::DeploymentOnDemandBudget, "");
    let period = (month_start(now), now);
    let path = format!(
        "/v1/billing/charges?teamId={team}&from={}&to={}",
        time::rfc3339(period.0),
        time::rfc3339(period.1)
    );
    match super::send("vercel.billing_charges", get(ctx, token, &path)).await {
        Ok(response) => map_charges(&mut b, &response.body, &mut resources, period),
        Err(fail)
            if fail.class == FailureClass::RateLimited
                || fail.class == FailureClass::AuthExpired =>
        {
            return Err(fail);
        }
        Err(fail) => {
            b.failed(MetricKind::DeploymentBilledCost, "total", fail.clone());
            b.failed(MetricKind::DeploymentEffectiveCost, "total", fail);
        }
    }
    b.resources = resources;
    Ok(b.finish(now))
}
