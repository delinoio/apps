// SPDX-License-Identifier: FSL-1.1-ALv2
//! WidgetDataService: widget data for personal and team dashboards. Sharing is evaluated on
//! every call.
use super::dashboard::{
    DashboardRow, Space, WidgetRow, applicable_kind, dashboard_access, widgets,
};
use super::*;
use crate::{
    Server,
    audit::{Event, record},
    call::User,
    cost::CostSum,
    usage::{self, Point, Range},
    validate,
};
use buffa_types::google::protobuf::Timestamp;
use chrono::Duration;
use chrono_tz::Tz;
use delino_protocol::anydash::v1::__buffa::oneof::{
    metric_value::Value as MV, widget_data::Content,
};
use sqlx::Row;
use std::collections::{BTreeMap, HashMap, HashSet};

/// A breakdown group: device, tool, model and totals.
type Group = (Option<Uuid>, LocalTool, String, Acc);
/// Connection order, resource, kind, dimension and temporality of a history series.
type SeriesKey = (usize, String, i32, String, i16);

pub async fn query_widget_data(
    s: &Server,
    call: &Call,
    q: QueryWidgetDataRequest,
) -> Result<QueryWidgetDataResponse> {
    let user = s.user(call).await?;
    let id = validate::id("dashboard_id", &q.dashboard_id)?;
    let widget = validate::id("widget_id", &q.widget_id)?;
    let tz = zone(&q.time_zone, &user)?;
    let (row, space) = dashboard_access(s, call, &user, id).await?;
    let mut db = s.db.acquire().await?;
    let all = widgets(&mut db, id).await?;
    drop(db);
    let selected: Vec<WidgetRow> = all.into_iter().filter(|w| w.id == widget).collect();
    if selected.is_empty() {
        return Err(ApiError::not_found("widget_id"));
    }
    let mut data = build(s, call, &row, &space, selected, tz).await?;
    Ok(QueryWidgetDataResponse {
        data: data.pop().into(),
        ..Default::default()
    })
}

pub async fn query_dashboard_data(
    s: &Server,
    call: &Call,
    q: QueryDashboardDataRequest,
) -> Result<QueryDashboardDataResponse> {
    let user = s.user(call).await?;
    let id = validate::id("dashboard_id", &q.dashboard_id)?;
    let tz = zone(&q.time_zone, &user)?;
    let (row, space) = dashboard_access(s, call, &user, id).await?;
    let mut db = s.db.acquire().await?;
    let all = widgets(&mut db, id).await?;
    drop(db);
    let version = row.version as u64;
    Ok(QueryDashboardDataResponse {
        dashboard_version: version,
        widgets: build(s, call, &row, &space, all, tz).await?,
        ..Default::default()
    })
}

fn zone(requested: &str, user: &User) -> Result<Tz> {
    if requested.is_empty() {
        validate::time_zone("time_zone", &user.time_zone).or(Ok(chrono_tz::UTC))
    } else {
        validate::time_zone("time_zone", requested)
    }
}

/// Evaluates sharing for every widget and builds its data.
async fn build(
    s: &Server,
    call: &Call,
    row: &DashboardRow,
    space: &Space,
    mut list: Vec<WidgetRow>,
    tz: Tz,
) -> Result<Vec<WidgetData>> {
    if !space.personal {
        let owners: Vec<String> = list
            .iter()
            .filter(|w| w.sharing_state == SharingState::Shared)
            .map(|w| w.owner.to_string())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let roles = if owners.is_empty() {
            HashMap::new()
        } else {
            s.delcore.roles(&row.tenant.to_string(), &owners).await?
        };
        let accounts: HashSet<Uuid> =
            sqlx::query_scalar("SELECT user_id FROM accounts WHERE user_id=ANY($1)")
                .bind(list.iter().map(|w| w.owner).collect::<Vec<_>>())
                .fetch_all(&s.db)
                .await?
                .into_iter()
                .collect();
        for w in list
            .iter_mut()
            .filter(|w| w.sharing_state == SharingState::Shared)
        {
            let membership = roles.get(&w.owner.to_string()).copied().unwrap_or_default();
            let reason = if !accounts.contains(&w.owner) {
                Some(SharingEndReason::OwnerDataDeleted)
            } else if !membership.roles.viewer() || membership.since != w.owner_member_since {
                // A different membership start means the owner left and rejoined since the
                // widget was shared, even if no query ran during the absence.
                Some(SharingEndReason::OwnerLeftSpace)
            } else if !w.all_devices && w.connections.is_empty() && w.devices.is_empty() {
                Some(SharingEndReason::SourceDeleted)
            } else {
                None
            };
            if let Some(reason) = reason {
                end_sharing(s, call, row, w, reason).await?;
            }
        }
    }
    let mut out = Vec::with_capacity(list.len());
    for w in &list {
        out.push(widget_data(s, call, w, tz).await?);
    }
    Ok(out)
}

/// Persists `ENDED` the first time the condition is detected; rejoining does not restore it.
async fn end_sharing(
    s: &Server,
    call: &Call,
    row: &DashboardRow,
    w: &mut WidgetRow,
    reason: SharingEndReason,
) -> Result<()> {
    let mut tx = s.db.begin().await?;
    let changed = sqlx::query("UPDATE widgets SET sharing_state=$2,sharing_end_reason=$3 WHERE id=$1 AND sharing_state=$4")
        .bind(w.id)
        .bind(e16(SharingState::Ended))
        .bind(e16(reason))
        .bind(e16(SharingState::Shared))
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if changed == 1 {
        record(
            &mut tx,
            call,
            Event {
                kind: ActivityKind::WidgetSharingEnded,
                actor_user: Some(w.owner),
                subject_user: Some(w.owner),
                tenant: Some(row.tenant),
                dashboard: Some(row.id),
                widget: Some(w.id),
                ..Default::default()
            },
        )
        .await?;
    }
    tx.commit().await?;
    tracing::info!(widget_id = %w.id, reason = reason.proto_name(), "widget sharing ended");
    w.sharing_state = SharingState::Ended;
    w.end_reason = reason;
    Ok(())
}

fn default_title(t: WidgetType) -> &'static str {
    match t {
        WidgetType::SubscriptionLimits => "Subscription limits",
        WidgetType::SubscriptionHistory => "Subscription history",
        WidgetType::LocalUsageSummary => "Local usage",
        WidgetType::LocalUsageSeries => "Local usage over time",
        WidgetType::LocalUsageBreakdown => "Local usage breakdown",
        WidgetType::DeploymentResources => "Deployments",
        WidgetType::DeploymentUsage => "Deployment usage",
        WidgetType::DeploymentCost => "Deployment cost",
        WidgetType::MetricHistory => "Metric history",
        _ => "Widget",
    }
}

async fn widget_data(s: &Server, call: &Call, w: &WidgetRow, tz: Tz) -> Result<WidgetData> {
    let title = if w.layout.title.is_empty() {
        default_title(w.kind).to_owned()
    } else {
        w.layout.title.clone()
    };
    let mut data = WidgetData {
        widget_id: w.id.to_string(),
        r#type: w.kind.into(),
        sharing_state: w.sharing_state.into(),
        sharing_end_reason: w.end_reason.into(),
        generated_at: Some(ts(call.now)).into(),
        time_zone: tz.name().into(),
        ..Default::default()
    };
    if w.sharing_state == SharingState::Ended {
        return Ok(data);
    }
    let period = w
        .query
        .period
        .as_known()
        .filter(|p| *p != QueryPeriod::Unspecified);
    let range = period.map(|p| usage::period_range(p, tz, call.now));
    if let Some(r) = range {
        data.range_start = Some(ts(r.start)).into();
        data.range_end = Some(ts(r.end)).into();
    }
    let mut notices = BTreeMap::new();
    let (content, summary) = match w.kind {
        WidgetType::LocalUsageSummary
        | WidgetType::LocalUsageSeries
        | WidgetType::LocalUsageBreakdown => {
            local_usage(
                s,
                call,
                w,
                tz,
                range.unwrap_or(Range {
                    start: call.now,
                    end: call.now,
                }),
                &mut notices,
                &title,
            )
            .await?
        }
        WidgetType::SubscriptionLimits
        | WidgetType::DeploymentUsage
        | WidgetType::DeploymentCost => metric_panel(s, call, w, &mut notices, &title).await?,
        WidgetType::SubscriptionHistory | WidgetType::MetricHistory => {
            metric_history(
                s,
                call,
                w,
                tz,
                range.unwrap_or(Range {
                    start: call.now,
                    end: call.now,
                }),
                &mut notices,
                &title,
            )
            .await?
        }
        WidgetType::DeploymentResources => resource_list(s, call, w, &mut notices, &title).await?,
        _ => (None, WidgetSummary::default()),
    };
    data.content = content;
    data.summary = Some(summary).into();
    data.notices = notices.into_values().map(Into::into).collect();
    Ok(data)
}

/// Precedence of parser states when one breakdown row covers several device and tool statuses:
/// `PARTIAL` > `OK` > `NO_SOURCES` > `DISABLED`. A lower rank wins; unknown states rank last.
fn parser_state_rank(state: ParserState) -> u8 {
    match state {
        ParserState::Partial => 0,
        ParserState::Ok => 1,
        ParserState::NoSources => 2,
        ParserState::Disabled => 3,
        _ => 4,
    }
}

/// Combines the parser statuses behind one breakdown row into the highest-precedence state and
/// the sum of their skipped record counts. The result does not depend on the input order, so the
/// same stored data always yields the same row.
fn combine_parser_states(relevant: Vec<&(ParserState, u64)>) -> (ParserState, u64) {
    let state = relevant
        .iter()
        .map(|(state, _)| *state)
        .min_by_key(|state| parser_state_rank(*state))
        .unwrap_or(ParserState::Unspecified);
    (state, relevant.iter().map(|(_, n)| *n).sum())
}

fn notice(notices: &mut BTreeMap<i32, DataNotice>, n: DataNotice) {
    notices.insert(n.to_i32(), n);
}

/// Running totals of usage buckets.
#[derive(Clone, Debug, Default)]
struct Acc {
    sessions: u64,
    tokens: [Option<u64>; 4],
    cost: CostSum,
}

impl Acc {
    fn add(&mut self, b: &BucketRow) {
        self.sessions = self.sessions.saturating_add(b.sessions);
        for (i, t) in b.tokens.iter().enumerate() {
            if let Some(t) = t {
                self.tokens[i] = Some(self.tokens[i].unwrap_or(0).saturating_add(*t));
            }
        }
        self.cost.add(
            b.computable,
            b.cost.as_deref(),
            b.version.map(|v| v.to_string()),
        );
    }
    fn totals(&self) -> UsageTotals {
        UsageTotals {
            session_count: self.sessions,
            tokens: Some(TokenCounts {
                input_tokens: self.tokens[0],
                output_tokens: self.tokens[1],
                cache_read_tokens: self.tokens[2],
                cache_write_tokens: self.tokens[3],
                ..Default::default()
            })
            .into(),
            estimated_cost: Some(self.cost.estimate()).into(),
            ..Default::default()
        }
    }
    fn total_tokens(&self) -> u64 {
        self.tokens
            .iter()
            .flatten()
            .fold(0u64, |a, b| a.saturating_add(*b))
    }
}

struct BucketRow {
    device: Uuid,
    tool: LocalTool,
    model: String,
    start: DateTime<Utc>,
    sessions: u64,
    tokens: [Option<u64>; 4],
    computable: bool,
    cost: Option<String>,
    version: Option<Uuid>,
    updated: DateTime<Utc>,
}

struct DeviceInfo {
    name: String,
    enrolled: DateTime<Utc>,
    heartbeat: Option<DateTime<Utc>>,
}

fn money(amount: &str) -> MetricValue {
    MetricValue {
        state: MetricState::Value.into(),
        value: Some(MV::Money(Box::new(Money {
            currency_code: "USD".into(),
            amount: amount.into(),
            ..Default::default()
        }))),
        ..Default::default()
    }
}

fn integer(n: u64) -> MetricValue {
    MetricValue {
        state: MetricState::Value.into(),
        value: Some(MV::Integer(i64::try_from(n).unwrap_or(i64::MAX))),
        ..Default::default()
    }
}

fn cost_value(c: &CostEstimate) -> MetricValue {
    match c.state.as_known() {
        Some(MetricState::CannotBeCalculated) => MetricValue {
            state: MetricState::CannotBeCalculated.into(),
            ..Default::default()
        },
        Some(MetricState::Partial) => {
            let mut v = money(&c.amount_usd);
            v.state = MetricState::Partial.into();
            v
        }
        _ => money(&c.amount_usd),
    }
}

#[allow(clippy::too_many_arguments)]
async fn local_usage(
    s: &Server,
    call: &Call,
    w: &WidgetRow,
    tz: Tz,
    range: Range,
    notices: &mut BTreeMap<i32, DataNotice>,
    title: &str,
) -> Result<(Option<Content>, WidgetSummary)> {
    let devices: HashMap<Uuid, DeviceInfo> = sqlx::query(
        "SELECT id,name,enrolled_at,last_heartbeat_at FROM devices WHERE user_id=$1 AND deleted_at IS NULL AND ($2 OR id=ANY($3))",
    )
    .bind(w.owner)
    .bind(w.all_devices)
    .bind(&w.devices)
    .fetch_all(&s.db)
    .await?
    .into_iter()
    .map(|r| {
        (
            r.get("id"),
            DeviceInfo {
                name: r.get("name"),
                enrolled: r.get("enrolled_at"),
                heartbeat: r.get("last_heartbeat_at"),
            },
        )
    })
    .collect();
    let ids: Vec<Uuid> = devices.keys().copied().collect();
    let tools: Vec<i16> = w
        .query
        .tools
        .iter()
        .filter_map(|t| t.as_known())
        .map(e16)
        .collect();
    let rows = sqlx::query(
        "SELECT device_id,tool,model,bucket_start,session_count,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,cost_computable,\
         estimated_cost_usd::text AS cost,price_version_id,updated_at FROM usage_buckets \
         WHERE user_id=$1 AND device_id=ANY($2) AND bucket_start>=$3 AND bucket_start<$4 \
         AND (cardinality($5::smallint[])=0 OR tool=ANY($5)) AND (cardinality($6::text[])=0 OR model=ANY($6)) ORDER BY bucket_start",
    )
    .bind(w.owner)
    .bind(&ids)
    .bind(crate::time::floor_bucket(range.start))
    .bind(range.end)
    .bind(&tools)
    .bind(&w.query.models)
    .fetch_all(&s.db)
    .await?;
    let opt =
        |r: &sqlx::postgres::PgRow, c: &str| r.get::<Option<i64>, _>(c).map(|v| v.max(0) as u64);
    let buckets: Vec<BucketRow> = rows
        .iter()
        .map(|r| BucketRow {
            device: r.get("device_id"),
            tool: enum_of(r.get("tool")),
            model: r.get("model"),
            start: r.get("bucket_start"),
            sessions: r.get::<i64, _>("session_count").max(0) as u64,
            tokens: [
                opt(r, "input_tokens"),
                opt(r, "output_tokens"),
                opt(r, "cache_read_tokens"),
                opt(r, "cache_write_tokens"),
            ],
            computable: r.get("cost_computable"),
            cost: r.get("cost"),
            version: r.get("price_version_id"),
            updated: r.get("updated_at"),
        })
        .collect();
    // Parser states of included devices and tools.
    let parsers: HashMap<(Uuid, i16), (ParserState, u64)> = sqlx::query(
        "SELECT device_id,tool,state,skipped_record_count FROM device_parser_status WHERE device_id=ANY($1) AND (cardinality($2::smallint[])=0 OR tool=ANY($2))",
    )
    .bind(&ids)
    .bind(&tools)
    .fetch_all(&s.db)
    .await?
    .into_iter()
    .map(|r| {
        (
            (r.get("device_id"), r.get("tool")),
            (enum_of(r.get("state")), r.get::<i64, _>("skipped_record_count").max(0) as u64),
        )
    })
    .collect();
    let with_data: HashSet<Uuid> = buckets.iter().map(|b| b.device).collect();
    if with_data.len() >= 2 {
        notice(notices, DataNotice::PossibleCrossDeviceDuplication);
    }
    if parsers
        .values()
        .any(|(state, _)| *state == ParserState::Partial)
    {
        notice(notices, DataNotice::PartialAggregation);
    }
    if devices
        .values()
        .any(|d| !super::connection::online(d.heartbeat, call.now))
    {
        notice(notices, DataNotice::NoRunningCollector);
    }
    if !buckets.is_empty() {
        notice(notices, DataNotice::EstimatedCost);
    }
    let recent: Vec<Uuid> = devices
        .iter()
        .filter(|(_, d)| call.now - d.enrolled < Duration::days(crate::time::RETENTION_DAYS))
        .map(|(id, _)| *id)
        .collect();
    if !recent.is_empty() {
        let earliest: Vec<Option<DateTime<Utc>>> = sqlx::query_scalar(
            "SELECT min(bucket_start) FROM usage_buckets WHERE device_id=ANY($1) GROUP BY device_id",
        )
        .bind(&recent)
        .fetch_all(&s.db)
        .await?;
        if earliest.iter().flatten().any(|e| range.start < *e) {
            notice(notices, DataNotice::BackfillLimited);
        }
    }
    let mut total = Acc::default();
    for b in &buckets {
        total.add(b);
    }
    let updated = buckets.iter().map(|b| b.updated).max();
    let summary = WidgetSummary {
        title: title.into(),
        primary_label: "Tokens".into(),
        primary_value: Some(integer(total.total_tokens())).into(),
        secondary_label: "Estimated cost".into(),
        secondary_value: Some(cost_value(&total.cost.estimate())).into(),
        data_updated_at: updated.map(ts).into(),
        ..Default::default()
    };
    let content = match w.kind {
        WidgetType::LocalUsageSummary => Content::UsageSummary(Box::new(UsageSummary {
            totals: Some(total.totals()).into(),
            ..Default::default()
        })),
        WidgetType::LocalUsageSeries => {
            let granularity = w
                .query
                .granularity
                .as_known()
                .unwrap_or(TimeGranularity::Day);
            let points = usage::points(granularity, tz, range);
            let mut accs: HashMap<Point, Acc> = HashMap::new();
            for b in &buckets {
                accs.entry(usage::point_of(granularity, tz, b.start))
                    .or_default()
                    .add(b);
            }
            Content::UsageSeries(Box::new(UsageSeries {
                granularity: granularity.into(),
                points: points
                    .iter()
                    .map(|p| UsagePoint {
                        start: Some(ts(p.start)).into(),
                        end: Some(ts(p.end)).into(),
                        utc_offset_minutes: p.utc_offset_minutes,
                        totals: Some(accs.get(p).cloned().unwrap_or_default().totals()).into(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }))
        }
        _ => {
            let dimension = w
                .query
                .breakdown
                .as_known()
                .unwrap_or(UsageBreakdownDimension::Device);
            let mut groups: BTreeMap<(String, i32, String), Group> = BTreeMap::new();
            for b in &buckets {
                let (key, entry) = match dimension {
                    UsageBreakdownDimension::Tool => (
                        (String::new(), b.tool.to_i32(), String::new()),
                        (None, b.tool, String::new()),
                    ),
                    UsageBreakdownDimension::Model => (
                        (String::new(), 0, b.model.clone()),
                        (None, LocalTool::Unspecified, b.model.clone()),
                    ),
                    _ => {
                        let name = devices
                            .get(&b.device)
                            .map(|d| d.name.clone())
                            .unwrap_or_default();
                        (
                            (format!("{name}\u{0}{}", b.device), 0, String::new()),
                            (Some(b.device), LocalTool::Unspecified, String::new()),
                        )
                    }
                };
                groups
                    .entry(key)
                    .or_insert_with(|| (entry.0, entry.1, entry.2, Acc::default()))
                    .3
                    .add(b);
            }
            let rows = groups
                .into_values()
                .map(|(device, tool, model, acc)| {
                    let relevant: Vec<&(ParserState, u64)> = parsers
                        .iter()
                        .filter(|((d, t), _)| {
                            device.is_none_or(|x| x == *d)
                                && (tool == LocalTool::Unspecified
                                    || i32::from(*t) == tool.to_i32())
                        })
                        .map(|(_, v)| v)
                        .collect();
                    let (parser_state, skipped) =
                        if dimension == UsageBreakdownDimension::Model || relevant.is_empty() {
                            (ParserState::Unspecified, 0)
                        } else {
                            combine_parser_states(relevant)
                        };
                    UsageBreakdownRow {
                        device_id: device.map(|d| d.to_string()).unwrap_or_default(),
                        device_name: device
                            .and_then(|d| devices.get(&d))
                            .map(|d| d.name.clone())
                            .unwrap_or_default(),
                        tool: tool.into(),
                        model,
                        totals: Some(acc.totals()).into(),
                        parser_state: parser_state.into(),
                        skipped_record_count: skipped,
                        ..Default::default()
                    }
                })
                .collect();
            Content::UsageBreakdown(Box::new(UsageBreakdown {
                dimension: dimension.into(),
                rows,
                total: Some(total.totals()).into(),
                ..Default::default()
            }))
        }
    };
    Ok((Some(content), summary))
}

/// Connections of the data owner included in a widget, in source order, with providers.
async fn included_connections(s: &Server, w: &WidgetRow) -> Result<Vec<(Uuid, Provider)>> {
    let rows: Vec<(Uuid, i16)> = sqlx::query_as(
        "SELECT id,provider FROM connections WHERE id=ANY($1) AND user_id=$2 AND deleted_at IS NULL ORDER BY created_at,id",
    )
    .bind(&w.connections)
    .bind(w.owner)
    .fetch_all(&s.db)
    .await?;
    Ok(rows.into_iter().map(|(id, p)| (id, enum_of(p))).collect())
}

/// Adds `NO_RUNNING_COLLECTOR` when an included connection has no registered device with a
/// heartbeat in the last 5 minutes.
async fn connection_notices(
    s: &Server,
    call: &Call,
    ids: &[Uuid],
    notices: &mut BTreeMap<i32, DataNotice>,
) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let running: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT cd.connection_id) FROM connection_devices cd JOIN devices d ON d.id=cd.device_id \
         WHERE cd.connection_id=ANY($1) AND d.deleted_at IS NULL AND d.last_heartbeat_at>$2",
    )
    .bind(ids)
    .bind(call.now - Duration::seconds(crate::time::LEASE_SECONDS))
    .fetch_one(&s.db)
    .await?;
    if running < ids.len() as i64 {
        notice(notices, DataNotice::NoRunningCollector);
    }
    Ok(())
}

fn stored_value(v: Option<serde_json::Value>, state: MetricState) -> MetricValue {
    v.and_then(|v| serde_json::from_value::<MetricValue>(v).ok())
        .unwrap_or_else(|| MetricValue {
            state: state.into(),
            ..Default::default()
        })
}

fn kind_filter(w: &WidgetRow, k: MetricKind) -> bool {
    applicable_kind(w.kind, k)
        && (w.query.metric_kinds.is_empty()
            || w.query.metric_kinds.iter().any(|x| x.as_known() == Some(k)))
}

/// Derives `100 − used` for the remaining percentage; non-value states carry over.
pub fn remaining(v: &MetricValue) -> MetricValue {
    match &v.value {
        Some(MV::Percent(p)) => MetricValue {
            state: v.state,
            value: Some(MV::Percent((100.0 - p).max(0.0))),
            ..Default::default()
        },
        _ => MetricValue {
            state: v.state,
            ..Default::default()
        },
    }
}

async fn readings(
    s: &Server,
    w: &WidgetRow,
    conns: &[(Uuid, Provider)],
    resource_level: Option<bool>,
) -> Result<Vec<MetricReading>> {
    let ids: Vec<Uuid> = conns.iter().map(|c| c.0).collect();
    let rows = sqlx::query(
        "SELECT connection_id,resource_id,kind,dimension,state,value,unit,temporality,period_start,period_end,observed_at,failure_class,last_success_value,last_success_at \
         FROM metric_latest WHERE connection_id=ANY($1) ORDER BY resource_id,kind,dimension",
    )
    .bind(&ids)
    .fetch_all(&s.db)
    .await?;
    let provider: HashMap<Uuid, Provider> = conns.iter().copied().collect();
    let mut out = Vec::new();
    for c in conns {
        for r in rows
            .iter()
            .filter(|r| r.get::<Uuid, _>("connection_id") == c.0)
        {
            let kind: MetricKind = enum_of(r.get("kind"));
            let resource: String = r.get("resource_id");
            let dimension: String = r.get("dimension");
            if let Some(level) = resource_level
                && level == resource.is_empty()
            {
                continue;
            }
            // A nonempty resource allowlist also excludes account-level readings, which have no
            // resource ID: the data owner chose to show only the listed resources.
            if !w.query.resource_ids.is_empty() && !w.query.resource_ids.contains(&resource) {
                continue;
            }
            if !w.query.dimensions.is_empty() && !w.query.dimensions.contains(&dimension) {
                continue;
            }
            let state: MetricState = enum_of(r.get("state"));
            let current = stored_value(r.get("value"), state);
            let failed = matches!(state, MetricState::Failed | MetricState::AuthExpired);
            let last_success = if failed {
                r.get::<Option<serde_json::Value>, _>("last_success_value")
                    .and_then(|v| serde_json::from_value::<MetricValue>(v).ok())
            } else {
                None
            };
            let reading = MetricReading {
                connection_id: c.0.to_string(),
                provider: provider[&c.0].into(),
                resource_id: resource.clone(),
                kind: kind.into(),
                dimension: dimension.clone(),
                unit: r.get("unit"),
                temporality: enum_of::<MetricTemporality>(r.get("temporality")).into(),
                current: Some(current.clone()).into(),
                observed_at: Some(ts(r.get("observed_at"))).into(),
                last_success: last_success.clone().into(),
                last_success_at: if failed {
                    r.get::<Option<DateTime<Utc>>, _>("last_success_at").map(ts)
                } else {
                    None
                }
                .into(),
                failure_class: enum_of::<FailureClass>(r.get("failure_class")).into(),
                period_start: r
                    .get::<Option<DateTime<Utc>>, _>("period_start")
                    .map(ts)
                    .into(),
                period_end: r
                    .get::<Option<DateTime<Utc>>, _>("period_end")
                    .map(ts)
                    .into(),
                ..Default::default()
            };
            if kind_filter(w, kind) {
                out.push(reading.clone());
            }
            if kind == MetricKind::SubscriptionUsedPercent
                && kind_filter(w, MetricKind::SubscriptionRemainingPercent)
            {
                let mut derived = reading;
                derived.kind = MetricKind::SubscriptionRemainingPercent.into();
                derived.current = Some(remaining(&current)).into();
                derived.last_success = last_success.map(|v| remaining(&v)).into();
                out.push(derived);
            }
        }
    }
    Ok(out)
}

fn label(r: &MetricReading) -> String {
    let provider = super::connection::provider_name(r.provider.as_known().unwrap_or_default());
    let kind = r
        .kind
        .as_known()
        .map(|k| {
            k.proto_name()
                .trim_start_matches("METRIC_KIND_")
                .replace('_', " ")
                .to_lowercase()
        })
        .unwrap_or_default();
    if r.dimension.is_empty() {
        format!("{provider} {kind}")
    } else {
        format!("{provider} {} {kind}", r.dimension)
    }
}

fn newest(times: impl Iterator<Item = Option<Timestamp>>) -> Option<Timestamp> {
    times.flatten().max_by_key(|t| (t.seconds, t.nanos))
}

async fn metric_panel(
    s: &Server,
    call: &Call,
    w: &WidgetRow,
    notices: &mut BTreeMap<i32, DataNotice>,
    title: &str,
) -> Result<(Option<Content>, WidgetSummary)> {
    let conns = included_connections(s, w).await?;
    connection_notices(
        s,
        call,
        &conns.iter().map(|c| c.0).collect::<Vec<_>>(),
        notices,
    )
    .await?;
    let level = if w.kind == WidgetType::SubscriptionLimits {
        Some(false)
    } else {
        None
    };
    let readings = readings(s, w, &conns, level).await?;
    let preferred = |r: &&MetricReading| match w.kind {
        WidgetType::SubscriptionLimits => {
            r.kind.as_known() == Some(MetricKind::SubscriptionRemainingPercent)
        }
        WidgetType::DeploymentCost => {
            r.kind.as_known() == Some(MetricKind::DeploymentBilledCost) && r.resource_id.is_empty()
        }
        _ => r.resource_id.is_empty(),
    };
    let mut ordered: Vec<&MetricReading> = readings.iter().filter(preferred).collect();
    ordered.extend(readings.iter().filter(|r| !preferred(r)));
    let summary = WidgetSummary {
        title: title.into(),
        primary_label: ordered.first().map(|r| label(r)).unwrap_or_default(),
        primary_value: ordered
            .first()
            .and_then(|r| r.current.as_option().cloned())
            .into(),
        secondary_label: ordered.get(1).map(|r| label(r)).unwrap_or_default(),
        secondary_value: ordered
            .get(1)
            .and_then(|r| r.current.as_option().cloned())
            .into(),
        data_updated_at: newest(readings.iter().map(|r| r.observed_at.as_option().cloned())).into(),
        ..Default::default()
    };
    Ok((
        Some(Content::MetricPanel(Box::new(MetricPanel {
            readings,
            ..Default::default()
        }))),
        summary,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn metric_history(
    s: &Server,
    call: &Call,
    w: &WidgetRow,
    tz: Tz,
    range: Range,
    notices: &mut BTreeMap<i32, DataNotice>,
    title: &str,
) -> Result<(Option<Content>, WidgetSummary)> {
    let conns = included_connections(s, w).await?;
    let ids: Vec<Uuid> = conns.iter().map(|c| c.0).collect();
    connection_notices(s, call, &ids, notices).await?;
    let rows = sqlx::query(
        "SELECT connection_id,resource_id,kind,dimension,hour_start,value,unit,temporality,observed_at FROM metric_hourly \
         WHERE connection_id=ANY($1) AND hour_start>=$2 AND hour_start<$3 ORDER BY connection_id,resource_id,kind,dimension,hour_start",
    )
    .bind(&ids)
    .bind(crate::time::floor_hour(range.start))
    .bind(range.end)
    .fetch_all(&s.db)
    .await?;
    let granularity = w
        .query
        .granularity
        .as_known()
        .unwrap_or(TimeGranularity::Hour);
    let account_only = w.kind == WidgetType::SubscriptionHistory;
    let order: HashMap<Uuid, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut series: BTreeMap<SeriesKey, MetricHistorySeries> = BTreeMap::new();
    let mut day_index: HashMap<(SeriesKey, DateTime<Utc>), usize> = HashMap::new();
    for r in &rows {
        let conn: Uuid = r.get("connection_id");
        let kind: MetricKind = enum_of(r.get("kind"));
        let resource: String = r.get("resource_id");
        let dimension: String = r.get("dimension");
        if (account_only && !resource.is_empty())
            || (!w.query.resource_ids.is_empty() && !w.query.resource_ids.contains(&resource))
            || (!w.query.dimensions.is_empty() && !w.query.dimensions.contains(&dimension))
        {
            continue;
        }
        let value = stored_value(r.get("value"), MetricState::Value);
        // Collectors never report the remaining percentage, so its history is derived from the
        // used-percentage history in the same way as the latest readings.
        let mut values = Vec::with_capacity(2);
        if kind_filter(w, kind) {
            values.push((kind, value.clone()));
        }
        if kind == MetricKind::SubscriptionUsedPercent
            && kind_filter(w, MetricKind::SubscriptionRemainingPercent)
        {
            values.push((MetricKind::SubscriptionRemainingPercent, remaining(&value)));
        }
        let temporality: i16 = r.get("temporality");
        let hour: DateTime<Utc> = r.get("hour_start");
        let observed_at: DateTime<Utc> = r.get("observed_at");
        // A daily point belongs to the local day of its observation. The UTC hour that holds the
        // observation can start on the previous local day for 30- and 45-minute offsets, so the
        // hour-aligned query can also return an observation from before the range start.
        if granularity == TimeGranularity::Day && observed_at < range.start {
            continue;
        }
        for (kind, value) in values {
            let key = (
                order[&conn],
                resource.clone(),
                kind.to_i32(),
                dimension.clone(),
                temporality,
            );
            let entry = series
                .entry(key.clone())
                .or_insert_with(|| MetricHistorySeries {
                    connection_id: conn.to_string(),
                    resource_id: resource.clone(),
                    kind: kind.into(),
                    dimension: dimension.clone(),
                    unit: r.get("unit"),
                    temporality: enum_of::<MetricTemporality>(temporality).into(),
                    ..Default::default()
                });
            let point = MetricHistoryPoint {
                hour_start: Some(ts(hour)).into(),
                observed_at: Some(ts(observed_at)).into(),
                value: Some(value).into(),
                ..Default::default()
            };
            if granularity == TimeGranularity::Day {
                // The last successful hourly snapshot observed in each local day. Rows are ordered
                // by hour, and each row's observation lies in its hour, so a later row is also a
                // later observation.
                let day = usage::point_of(TimeGranularity::Day, tz, observed_at).start;
                match day_index.get(&(key.clone(), day)) {
                    Some(i) => entry.points[*i] = point,
                    None => {
                        day_index.insert((key, day), entry.points.len());
                        entry.points.push(point);
                    }
                }
            } else {
                entry.points.push(point);
            }
        }
    }
    let series: Vec<MetricHistorySeries> = series.into_values().collect();
    let first = series.first().and_then(|s| s.points.last());
    let summary = WidgetSummary {
        title: title.into(),
        primary_label: series
            .first()
            .map(|s| {
                let kind = s
                    .kind
                    .as_known()
                    .map(|k| {
                        k.proto_name()
                            .trim_start_matches("METRIC_KIND_")
                            .replace('_', " ")
                            .to_lowercase()
                    })
                    .unwrap_or_default();
                if s.dimension.is_empty() {
                    kind
                } else {
                    format!("{} {kind}", s.dimension)
                }
            })
            .unwrap_or_default(),
        primary_value: first.and_then(|p| p.value.as_option().cloned()).into(),
        data_updated_at: newest(
            series
                .iter()
                .flat_map(|s| s.points.iter().map(|p| p.observed_at.as_option().cloned())),
        )
        .into(),
        ..Default::default()
    };
    Ok((
        Some(Content::MetricHistory(Box::new(MetricHistory {
            series,
            ..Default::default()
        }))),
        summary,
    ))
}

async fn resource_list(
    s: &Server,
    call: &Call,
    w: &WidgetRow,
    notices: &mut BTreeMap<i32, DataNotice>,
    title: &str,
) -> Result<(Option<Content>, WidgetSummary)> {
    let conns = included_connections(s, w).await?;
    let ids: Vec<Uuid> = conns.iter().map(|c| c.0).collect();
    connection_notices(s, call, &ids, notices).await?;
    let rows = sqlx::query(
        "SELECT connection_id,resource_id,kind,display_name,parent_id,health,provider_status,status_state,observed_at FROM resources_latest \
         WHERE connection_id=ANY($1) ORDER BY display_name,resource_id",
    )
    .bind(&ids)
    .fetch_all(&s.db)
    .await?;
    let metrics = readings(s, w, &conns, Some(true)).await?;
    let provider: HashMap<Uuid, Provider> = conns.iter().copied().collect();
    let resources: Vec<ResourceReading> = rows
        .iter()
        .filter(|r| {
            w.query.resource_ids.is_empty()
                || w.query
                    .resource_ids
                    .contains(&r.get::<String, _>("resource_id"))
        })
        .map(|r| {
            let conn: Uuid = r.get("connection_id");
            let resource: String = r.get("resource_id");
            ResourceReading {
                connection_id: conn.to_string(),
                provider: provider[&conn].into(),
                resource_id: resource.clone(),
                kind: enum_of::<ResourceKind>(r.get("kind")).into(),
                display_name: r.get("display_name"),
                parent_id: r.get("parent_id"),
                health: enum_of::<ResourceHealth>(r.get("health")).into(),
                provider_status: r.get("provider_status"),
                status_state: enum_of::<MetricState>(r.get("status_state")).into(),
                observed_at: Some(ts(r.get("observed_at"))).into(),
                metrics: metrics
                    .iter()
                    .filter(|m| m.connection_id == conn.to_string() && m.resource_id == resource)
                    .cloned()
                    .collect(),
                ..Default::default()
            }
        })
        .collect();
    let healthy = resources
        .iter()
        .filter(|r| r.health.as_known() == Some(ResourceHealth::Healthy))
        .count();
    let summary = WidgetSummary {
        title: title.into(),
        primary_label: "Resources".into(),
        primary_value: Some(integer(resources.len() as u64)).into(),
        secondary_label: "Healthy".into(),
        secondary_value: Some(integer(healthy as u64)).into(),
        data_updated_at: newest(resources.iter().map(|r| r.observed_at.as_option().cloned()))
            .into(),
        ..Default::default()
    };
    Ok((
        Some(Content::ResourceList(Box::new(ResourceList {
            resources,
            ..Default::default()
        }))),
        summary,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_state_precedence_ignores_input_order() {
        use ParserState as P;
        let states = [P::Partial, P::Ok, P::NoSources, P::Disabled];
        for (i, a) in states.iter().enumerate() {
            for (j, b) in states.iter().enumerate() {
                let (first, second) = ((*a, 2), (*b, 3));
                let expected = states[i.min(j)];
                for pair in [vec![&first, &second], vec![&second, &first]] {
                    assert_eq!(
                        combine_parser_states(pair),
                        (expected, 5),
                        "{a:?} with {b:?}"
                    );
                }
            }
        }
        let all = [
            (P::Disabled, 1),
            (P::NoSources, 2),
            (P::Ok, 4),
            (P::Partial, 8),
        ];
        assert_eq!(
            combine_parser_states(all.iter().rev().collect()),
            (P::Partial, 15)
        );
        assert_eq!(
            combine_parser_states(all.iter().take(3).collect()),
            (P::Ok, 7)
        );
        assert_eq!(
            combine_parser_states(all.iter().take(2).collect()),
            (P::NoSources, 3)
        );
    }
    #[test]
    fn remaining_percent_is_derived_only_from_used_percent() {
        let used = MetricValue {
            state: MetricState::Value.into(),
            value: Some(MV::Percent(37.5)),
            ..Default::default()
        };
        assert_eq!(remaining(&used).value, Some(MV::Percent(62.5)));
        let failed = MetricValue {
            state: MetricState::Failed.into(),
            ..Default::default()
        };
        assert_eq!(remaining(&failed).state, MetricState::Failed);
        assert_eq!(remaining(&failed).value, None);
        let over = MetricValue {
            state: MetricState::Value.into(),
            value: Some(MV::Percent(120.0)),
            ..Default::default()
        };
        assert_eq!(remaining(&over).value, Some(MV::Percent(0.0)));
    }
}
