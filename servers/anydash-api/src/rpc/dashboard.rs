// SPDX-License-Identifier: FSL-1.1-ALv2
//! DashboardService: spaces, dashboards, widgets and data-owner sharing.
use super::*;
use crate::{
    Server,
    audit::{Event, record},
    call::User,
    delcore::Roles,
    usage, validate,
};
use server_common::authority::TenantState;
use sqlx::Row;
use std::collections::{HashMap, HashSet};

/// The caller's effective access to one space.
#[derive(Clone, Copy, Debug)]
pub struct Space {
    pub tenant: Uuid,
    pub personal: bool,
    pub roles: Roles,
    /// Start of the caller's current team membership from the role read; `None` in the personal
    /// space. Stored on widgets the caller shares so that a later departure and rejoin ends them.
    pub member_since: Option<DateTime<Utc>>,
}

impl Space {
    pub fn kind(&self) -> SpaceKind {
        if self.personal {
            SpaceKind::Personal
        } else {
            SpaceKind::Team
        }
    }
    pub fn view(&self) -> bool {
        self.personal || self.roles.viewer()
    }
    pub fn edit(&self) -> bool {
        self.view()
    }
    pub fn create(&self) -> bool {
        self.personal || self.roles.manager()
    }
    pub fn delete(&self) -> bool {
        self.create()
    }
    pub fn activity(&self) -> bool {
        self.personal || self.roles.manager()
    }
    pub fn permissions(&self) -> Vec<buffa::EnumValue<SpacePermission>> {
        let mut out = Vec::new();
        if self.view() {
            out.push(SpacePermission::ViewDashboards.into());
        }
        if self.edit() {
            out.push(SpacePermission::EditDashboards.into());
        }
        if self.create() {
            out.push(SpacePermission::CreateDashboards.into());
        }
        if self.delete() {
            out.push(SpacePermission::DeleteDashboards.into());
        }
        if !self.personal && self.activity() {
            out.push(SpacePermission::ViewSpaceActivity.into());
        }
        out
    }
}

/// Reads the caller's access to a space from Delcore within the request. The space must be
/// `ACTIVE`; a team space requires the Owner, Admin or Member role.
pub async fn space(
    s: &Server,
    call: &Call,
    user: &User,
    tenant: Uuid,
    missing_field: Option<&str>,
) -> Result<Space> {
    call.note(|a| a.tenant = Some(tenant));
    if tenant == user.personal_tenant {
        s.delcore.require_active(&tenant.to_string()).await?;
        return Ok(Space {
            tenant,
            personal: true,
            roles: Roles::OWNER,
            member_since: None,
        });
    }
    let me = user.user_id.to_string();
    let membership = s
        .delcore
        .roles(&tenant.to_string(), std::slice::from_ref(&me))
        .await?
        .get(&me)
        .copied()
        .unwrap_or_default();
    if !membership.roles.viewer() {
        return Err(match missing_field {
            Some(field) => ApiError::not_found(field),
            None => ApiError::denied(),
        });
    }
    s.delcore.require_active(&tenant.to_string()).await?;
    Ok(Space {
        tenant,
        personal: false,
        roles: membership.roles,
        member_since: membership.since,
    })
}

/// Whether the user has at least one registered collector device.
pub async fn onboarded(db: &sqlx::PgPool, user: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM devices WHERE user_id=$1 AND deleted_at IS NULL)",
    )
    .bind(user)
    .fetch_one(db)
    .await?)
}

/// Personal space dashboard methods need a connected collector device; team spaces do not.
pub async fn require_onboarded(s: &Server, user: &User, space: &Space) -> Result<()> {
    if space.personal && !onboarded(&s.db, user.user_id).await? {
        return Err(ApiError::precondition(
            ErrorReason::OnboardingRequired,
            "connect a collector device first",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct DashboardRow {
    pub id: Uuid,
    pub tenant: Uuid,
    pub space_kind: i16,
    pub created_by: Uuid,
    pub name: String,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn load_dashboard(
    db: &mut PgConnection,
    id: Uuid,
    lock: bool,
) -> Result<Option<DashboardRow>> {
    let sql = format!(
        "SELECT id,tenant_id,space_kind,created_by_user_id,name,version,created_at,updated_at FROM dashboards WHERE id=$1 AND deleted_at IS NULL{}",
        if lock { " FOR UPDATE" } else { "" }
    );
    Ok(sqlx::query(&sql)
        .bind(id)
        .fetch_optional(db)
        .await?
        .map(|r| DashboardRow {
            id: r.get("id"),
            tenant: r.get("tenant_id"),
            space_kind: r.get("space_kind"),
            created_by: r.get("created_by_user_id"),
            name: r.get("name"),
            version: r.get("version"),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }))
}

/// Resolves a dashboard and the caller's access. Dashboards the caller cannot see are
/// `NOT_FOUND`, never distinguished from missing ones.
pub async fn dashboard_access(
    s: &Server,
    call: &Call,
    user: &User,
    id: Uuid,
) -> Result<(DashboardRow, Space)> {
    call.note(|a| a.dashboard = Some(id));
    let mut db = s.db.acquire().await?;
    let row = load_dashboard(&mut db, id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("dashboard_id"))?;
    drop(db);
    if row.space_kind == e16(SpaceKind::Personal) && row.tenant != user.personal_tenant {
        return Err(ApiError::not_found("dashboard_id"));
    }
    let space = space(s, call, user, row.tenant, Some("dashboard_id")).await?;
    require_onboarded(s, user, &space).await?;
    Ok((row, space))
}

/// Locks the dashboard for a versioned mutation and checks `expected_version`.
async fn lock_version(
    db: &mut PgConnection,
    id: Uuid,
    expected: Option<u64>,
) -> Result<DashboardRow> {
    let row = load_dashboard(db, id, true)
        .await?
        .ok_or_else(|| ApiError::not_found("dashboard_id"))?;
    if let Some(expected) = expected
        && row.version as u64 != expected
    {
        return Err(ApiError::version_conflict(row.version as u64));
    }
    Ok(row)
}

async fn bump(db: &mut PgConnection, id: Uuid, now: DateTime<Utc>) -> Result<()> {
    sqlx::query("UPDATE dashboards SET version=version+1,updated_at=$2 WHERE id=$1")
        .bind(id)
        .bind(now)
        .execute(db)
        .await?;
    Ok(())
}

pub struct WidgetRow {
    pub id: Uuid,
    pub owner: Uuid,
    pub kind: WidgetType,
    pub layout: WidgetLayout,
    pub query: WidgetQuery,
    pub all_devices: bool,
    pub sharing_state: SharingState,
    pub end_reason: SharingEndReason,
    /// The data owner's membership start when the widget was last shared in a team space.
    pub owner_member_since: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub connections: Vec<Uuid>,
    pub devices: Vec<Uuid>,
}

pub async fn widgets(db: &mut PgConnection, dashboard: Uuid) -> Result<Vec<WidgetRow>> {
    let rows = sqlx::query(
        "SELECT id,data_owner_user_id,type,x,y,width,height,title,presentation,query,all_devices,sharing_state,sharing_end_reason,data_owner_member_since,created_at \
         FROM widgets WHERE dashboard_id=$1 ORDER BY sort_order,id",
    )
    .bind(dashboard)
    .fetch_all(&mut *db)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.get("id")).collect();
    let sources = sqlx::query("SELECT widget_id,source_kind,source_id FROM widget_sources WHERE widget_id=ANY($1) ORDER BY source_id")
        .bind(&ids)
        .fetch_all(&mut *db)
        .await?;
    let mut by_widget: HashMap<Uuid, (Vec<Uuid>, Vec<Uuid>)> = HashMap::new();
    for s in sources {
        let entry = by_widget.entry(s.get("widget_id")).or_default();
        if s.get::<i16, _>("source_kind") == SOURCE_CONNECTION {
            entry.0.push(s.get("source_id"));
        } else {
            entry.1.push(s.get("source_id"));
        }
    }
    rows.into_iter()
        .map(|r| {
            let id: Uuid = r.get("id");
            let (connections, devices) = by_widget.remove(&id).unwrap_or_default();
            Ok(WidgetRow {
                id,
                owner: r.get("data_owner_user_id"),
                kind: enum_of(r.get("type")),
                layout: WidgetLayout {
                    x: r.get::<i32, _>("x") as u32,
                    y: r.get::<i32, _>("y") as u32,
                    width: r.get::<i32, _>("width") as u32,
                    height: r.get::<i32, _>("height") as u32,
                    title: r.get("title"),
                    presentation: enum_of::<WidgetPresentation>(r.get("presentation")).into(),
                    ..Default::default()
                },
                query: serde_json::from_value(r.get("query"))?,
                all_devices: r.get("all_devices"),
                sharing_state: enum_of(r.get("sharing_state")),
                end_reason: enum_of(r.get("sharing_end_reason")),
                owner_member_since: r.get("data_owner_member_since"),
                created_at: r.get("created_at"),
                connections,
                devices,
            })
        })
        .collect()
}

pub fn widget_message(w: &WidgetRow, caller: Uuid) -> Widget {
    let owner = w.owner == caller;
    Widget {
        id: w.id.to_string(),
        r#type: w.kind.into(),
        layout: Some(w.layout.clone()).into(),
        query: Some(w.query.clone()).into(),
        source: owner
            .then(|| WidgetSource {
                connection_ids: w.connections.iter().map(Uuid::to_string).collect(),
                device_ids: w.devices.iter().map(Uuid::to_string).collect(),
                all_devices: w.all_devices,
                ..Default::default()
            })
            .into(),
        data_owner_user_id: w.owner.to_string(),
        sharing_state: w.sharing_state.into(),
        sharing_end_reason: w.end_reason.into(),
        caller_is_data_owner: owner,
        created_at: Some(ts(w.created_at)).into(),
        ..Default::default()
    }
}

pub async fn dashboard_message(db: &mut PgConnection, id: Uuid, caller: Uuid) -> Result<Dashboard> {
    let row = load_dashboard(db, id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("dashboard_id"))?;
    let widgets = widgets(db, id).await?;
    Ok(Dashboard {
        id: row.id.to_string(),
        space_id: row.tenant.to_string(),
        name: row.name,
        version: row.version as u64,
        widgets: widgets.iter().map(|w| widget_message(w, caller)).collect(),
        created_by_user_id: row.created_by.to_string(),
        created_at: Some(ts(row.created_at)).into(),
        updated_at: Some(ts(row.updated_at)).into(),
        ..Default::default()
    })
}

pub async fn list_spaces(
    s: &Server,
    call: &Call,
    q: ListSpacesRequest,
) -> Result<ListSpacesResponse> {
    let user = s.user(call).await?;
    let claims = s
        .verifier
        .verify(&q.delcore_access_token, &s.config.delcore_resource)
        .await?;
    if claims.subject != user.subject {
        return Err(ApiError::unauthenticated());
    }
    let current = s.delcore.current_user(&q.delcore_access_token).await?;
    if current.user_id != user.user_id.to_string() {
        return Err(ApiError::precondition(
            ErrorReason::AccountNotConnected,
            "reconnect the AnyDash account",
        ));
    }
    let personal = user.personal_tenant.to_string();
    let teams: Vec<_> = current
        .tenants
        .iter()
        .filter(|t| t.id != personal && !t.personal)
        .collect();
    let mut ids = vec![personal.clone()];
    ids.extend(teams.iter().map(|t| t.id.clone()));
    let states = s.delcore.states(&ids).await?;
    if states.first() != Some(&TenantState::Active) {
        return Err(ApiError::space_inactive());
    }
    let personal_space = Space {
        tenant: user.personal_tenant,
        personal: true,
        roles: Roles::OWNER,
        member_since: None,
    };
    let onboarding = if onboarded(&s.db, user.user_id).await? {
        OnboardingState::Ready
    } else {
        OnboardingState::DeviceRequired
    };
    let mut spaces = vec![super::Space {
        id: personal.clone(),
        kind: SpaceKind::Personal.into(),
        name: current
            .tenants
            .iter()
            .find(|t| t.id == personal)
            .map(|t| t.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Personal".into()),
        permissions: personal_space.permissions(),
        onboarding_state: onboarding.into(),
        ..Default::default()
    }];
    let me = user.user_id.to_string();
    let mut team_spaces = Vec::new();
    for (tenant, state) in teams.iter().zip(&states[1..]) {
        if *state != TenantState::Active {
            continue;
        }
        let membership = s
            .delcore
            .roles(&tenant.id, std::slice::from_ref(&me))
            .await?
            .get(&me)
            .copied()
            .unwrap_or_default();
        if !membership.roles.viewer() {
            continue;
        }
        let space = Space {
            tenant: validate::id("space_id", &tenant.id)?,
            personal: false,
            roles: membership.roles,
            member_since: membership.since,
        };
        team_spaces.push(super::Space {
            id: tenant.id.clone(),
            kind: SpaceKind::Team.into(),
            name: tenant.name.clone(),
            permissions: space.permissions(),
            onboarding_state: OnboardingState::Ready.into(),
            ..Default::default()
        });
    }
    team_spaces.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    spaces.extend(team_spaces);
    Ok(ListSpacesResponse {
        spaces,
        ..Default::default()
    })
}

pub async fn list_dashboards(
    s: &Server,
    call: &Call,
    q: ListDashboardsRequest,
) -> Result<ListDashboardsResponse> {
    let user = s.user(call).await?;
    let tenant = validate::id("space_id", &q.space_id)?;
    let space = space(s, call, &user, tenant, None).await?;
    require_onboarded(s, &user, &space).await?;
    let page = q.page.as_option().cloned().unwrap_or_default();
    let size = validate::page_size(page.size)?;
    let context = format!("{}|ListDashboards|{tenant}", user.user_id);
    let after = decode_cursor(&context, &page.cursor)?;
    let rows = sqlx::query(
        "SELECT d.id,d.name,d.version,d.created_at,d.updated_at,(SELECT count(*) FROM widgets w WHERE w.dashboard_id=d.id) AS widgets \
         FROM dashboards d WHERE d.tenant_id=$1 AND d.space_kind=$2 AND d.deleted_at IS NULL \
         AND ($3::timestamptz IS NULL OR (d.created_at,d.id)>($3,$4)) ORDER BY d.created_at,d.id LIMIT $5",
    )
    .bind(tenant)
    .bind(e16(space.kind()))
    .bind(after.map(|a| a.0))
    .bind(after.map(|a| a.1).unwrap_or_default())
    .bind(size + 1)
    .fetch_all(&s.db)
    .await?;
    let more = rows.len() as i64 > size;
    let next_cursor = if more {
        let last = &rows[size as usize - 1];
        encode_cursor(
            &context,
            last.get("created_at"),
            &last.get::<Uuid, _>("id").to_string(),
        )
    } else {
        String::new()
    };
    Ok(ListDashboardsResponse {
        dashboards: rows
            .iter()
            .take(size as usize)
            .map(|r| DashboardSummary {
                id: r.get::<Uuid, _>("id").to_string(),
                space_id: tenant.to_string(),
                name: r.get("name"),
                version: r.get::<i64, _>("version") as u64,
                widget_count: r.get::<i64, _>("widgets") as u32,
                updated_at: Some(ts(r.get("updated_at"))).into(),
                ..Default::default()
            })
            .collect(),
        next_cursor,
        ..Default::default()
    })
}

pub async fn get_dashboard(
    s: &Server,
    call: &Call,
    q: GetDashboardRequest,
) -> Result<GetDashboardResponse> {
    let user = s.user(call).await?;
    let id = validate::id("dashboard_id", &q.dashboard_id)?;
    let _ = dashboard_access(s, call, &user, id).await?;
    let mut db = s.db.acquire().await?;
    Ok(GetDashboardResponse {
        dashboard: Some(dashboard_message(&mut db, id, user.user_id).await?).into(),
        ..Default::default()
    })
}

pub async fn create_dashboard(
    s: &Server,
    call: &Call,
    q: CreateDashboardRequest,
) -> Result<CreateDashboardResponse> {
    let user = s.user(call).await?;
    let request_id = validate::id("request_id", &q.request_id)?;
    let tenant = validate::id("space_id", &q.space_id)?;
    let name = validate::text("name", &q.name, 1, 100)?;
    let space = space(s, call, &user, tenant, None).await?;
    if !space.create() {
        return Err(ApiError::denied());
    }
    require_onboarded(s, &user, &space).await?;
    let caller = format!("user:{}", user.user_id);
    let print = fingerprint(&q);
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text,0))")
        .bind(format!("anydash-space:{tenant}"))
        .execute(&mut *tx)
        .await?;
    if let Some(existing) = replay(
        &mut tx,
        &caller,
        "CreateDashboard",
        request_id,
        &print,
        call.now,
    )
    .await?
    {
        let dashboard = dashboard_message(&mut tx, existing, user.user_id).await?;
        tx.commit().await?;
        return Ok(CreateDashboardResponse {
            dashboard: Some(dashboard).into(),
            ..Default::default()
        });
    }
    let used: i64 = sqlx::query_scalar("SELECT count(*) FROM dashboards WHERE tenant_id=$1 AND space_kind=$2 AND deleted_at IS NULL")
        .bind(tenant)
        .bind(e16(space.kind()))
        .fetch_one(&mut *tx)
        .await?;
    if used >= i64::from(QUOTA_DASHBOARDS) {
        return Err(ApiError::quota(
            QuotaKind::DashboardsPerSpace,
            QUOTA_DASHBOARDS,
            used as u32,
        ));
    }
    let id = crate::new_id();
    sqlx::query(
        "INSERT INTO dashboards(id,tenant_id,space_kind,created_by_user_id,name,version,created_at,updated_at) VALUES($1,$2,$3,$4,$5,1,$6,$6)",
    )
    .bind(id)
    .bind(tenant)
    .bind(e16(space.kind()))
    .bind(user.user_id)
    .bind(&name)
    .bind(call.now)
    .execute(&mut *tx)
    .await?;
    record(
        &mut tx,
        call,
        Event {
            kind: ActivityKind::DashboardCreated,
            actor_user: Some(user.user_id),
            subject_user: Some(user.user_id),
            tenant: Some(tenant),
            dashboard: Some(id),
            ..Default::default()
        },
    )
    .await?;
    remember(
        &mut tx,
        &caller,
        "CreateDashboard",
        request_id,
        &print,
        id,
        call.now,
    )
    .await?;
    let dashboard = dashboard_message(&mut tx, id, user.user_id).await?;
    tx.commit().await?;
    Ok(CreateDashboardResponse {
        dashboard: Some(dashboard).into(),
        ..Default::default()
    })
}

pub async fn rename_dashboard(
    s: &Server,
    call: &Call,
    q: RenameDashboardRequest,
) -> Result<RenameDashboardResponse> {
    let user = s.user(call).await?;
    let id = validate::id("dashboard_id", &q.dashboard_id)?;
    let name = validate::text("name", &q.name, 1, 100)?;
    let (_, space) = dashboard_access(s, call, &user, id).await?;
    if !space.edit() {
        return Err(ApiError::denied());
    }
    let mut tx = s.db.begin().await?;
    lock_version(&mut tx, id, Some(q.expected_version)).await?;
    sqlx::query("UPDATE dashboards SET name=$2 WHERE id=$1")
        .bind(id)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    bump(&mut tx, id, call.now).await?;
    let dashboard = dashboard_message(&mut tx, id, user.user_id).await?;
    tx.commit().await?;
    Ok(RenameDashboardResponse {
        dashboard: Some(dashboard).into(),
        ..Default::default()
    })
}

/// Blocks a dashboard immediately and enqueues its physical deletion.
pub async fn delete_dashboard_in(
    db: &mut PgConnection,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    sqlx::query("UPDATE dashboards SET deleted_at=$2 WHERE id=$1 AND deleted_at IS NULL")
        .bind(id)
        .bind(now)
        .execute(&mut *db)
        .await?;
    enqueue_deletion(db, DeletionKind::Dashboard, Some(id), None, None, now).await
}

pub async fn delete_dashboard(
    s: &Server,
    call: &Call,
    q: DeleteDashboardRequest,
) -> Result<DeleteDashboardResponse> {
    let user = s.user(call).await?;
    let id = validate::id("dashboard_id", &q.dashboard_id)?;
    let (row, space) = dashboard_access(s, call, &user, id).await?;
    if !space.delete() {
        return Err(ApiError::denied());
    }
    let mut tx = s.db.begin().await?;
    lock_version(&mut tx, id, None).await?;
    let deadline = delete_dashboard_in(&mut tx, id, call.now).await?;
    record(
        &mut tx,
        call,
        Event {
            kind: ActivityKind::DashboardDeleted,
            actor_user: Some(user.user_id),
            subject_user: Some(user.user_id),
            tenant: Some(row.tenant),
            dashboard: Some(id),
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await?;
    Ok(DeleteDashboardResponse {
        purge_deadline: Some(ts(deadline)).into(),
        ..Default::default()
    })
}

/// Provider categories for widget sources.
pub fn subscription_provider(p: Provider) -> bool {
    matches!(p, Provider::Codex | Provider::ClaudeCode | Provider::Grok)
}
pub fn deployment_provider(p: Provider) -> bool {
    matches!(
        p,
        Provider::Vercel | Provider::Supabase | Provider::Cloudflare
    )
}

fn allowed_presentations(t: WidgetType) -> &'static [WidgetPresentation] {
    use WidgetPresentation as P;
    match t {
        WidgetType::SubscriptionLimits => &[P::Number, P::Gauge, P::List],
        WidgetType::SubscriptionHistory
        | WidgetType::LocalUsageSeries
        | WidgetType::MetricHistory => &[P::LineChart, P::BarChart],
        WidgetType::LocalUsageSummary => &[P::Number, P::Table],
        WidgetType::LocalUsageBreakdown => &[P::Table, P::BarChart, P::List],
        WidgetType::DeploymentResources => &[P::List, P::Table],
        WidgetType::DeploymentUsage => &[P::Number, P::Table, P::BarChart],
        WidgetType::DeploymentCost => &[P::Number, P::Table],
        _ => &[],
    }
}

pub fn local_type(t: WidgetType) -> bool {
    matches!(
        t,
        WidgetType::LocalUsageSummary
            | WidgetType::LocalUsageSeries
            | WidgetType::LocalUsageBreakdown
    )
}

/// Metric kinds a widget type can show.
pub fn applicable_kind(t: WidgetType, k: MetricKind) -> bool {
    let name = k.proto_name();
    let subscription =
        name.starts_with("METRIC_KIND_SUBSCRIPTION_") || k == MetricKind::PlanDocumentedQuota;
    let deployment =
        name.starts_with("METRIC_KIND_DEPLOYMENT_") || k == MetricKind::PlanDocumentedQuota;
    match t {
        WidgetType::SubscriptionLimits | WidgetType::SubscriptionHistory => subscription,
        WidgetType::DeploymentUsage => {
            deployment
                && !matches!(
                    k,
                    MetricKind::DeploymentBilledCost | MetricKind::DeploymentEffectiveCost
                )
        }
        WidgetType::DeploymentCost => matches!(
            k,
            MetricKind::DeploymentBilledCost | MetricKind::DeploymentEffectiveCost
        ),
        WidgetType::DeploymentResources => deployment,
        WidgetType::MetricHistory => true,
        _ => false,
    }
}

/// Validates the layout fields.
pub fn check_layout(field: &str, t: WidgetType, l: &WidgetLayout) -> Result<WidgetLayout> {
    // Report the violating field. `x` is checked before `x + width`, so the sum cannot overflow.
    let fits = "the widget does not fit the 12-column grid";
    if l.x > 11 {
        return Err(ApiError::invalid(&format!("{field}.x"), fits));
    }
    if !(1..=12).contains(&l.width) || l.x + l.width > 12 {
        return Err(ApiError::invalid(&format!("{field}.width"), fits));
    }
    if l.y > 999 {
        return Err(ApiError::invalid(&format!("{field}.y"), fits));
    }
    if !(1..=12).contains(&l.height) {
        return Err(ApiError::invalid(&format!("{field}.height"), fits));
    }
    let title = validate::text(&format!("{field}.title"), &l.title, 0, 100)?;
    let presentation = validate::known(&format!("{field}.presentation"), &l.presentation)?;
    if !allowed_presentations(t).contains(&presentation) {
        return Err(ApiError::invalid(
            &format!("{field}.presentation"),
            "presentation is not allowed for this widget type",
        ));
    }
    Ok(WidgetLayout {
        x: l.x,
        y: l.y,
        width: l.width,
        height: l.height,
        title,
        presentation: presentation.into(),
        ..Default::default()
    })
}

/// Validates a widget query for its type and returns the normalized query.
pub fn check_query(t: WidgetType, q: &WidgetQuery) -> Result<WidgetQuery> {
    let period = validate::optional_enum("query.period", &q.period)?;
    let granularity = validate::optional_enum("query.granularity", &q.granularity)?;
    let breakdown = validate::optional_enum("query.breakdown", &q.breakdown)?;
    let (needs_period, needs_granularity) = match t {
        WidgetType::SubscriptionHistory
        | WidgetType::LocalUsageSeries
        | WidgetType::MetricHistory => (true, true),
        WidgetType::LocalUsageSummary | WidgetType::LocalUsageBreakdown => (true, false),
        _ => (false, false),
    };
    if needs_period != period.is_some() {
        return Err(ApiError::invalid(
            "query.period",
            if needs_period {
                "period is required"
            } else {
                "period must be unspecified"
            },
        ));
    }
    if needs_granularity != granularity.is_some() {
        return Err(ApiError::invalid(
            "query.granularity",
            if needs_granularity {
                "granularity is required"
            } else {
                "granularity must be unspecified"
            },
        ));
    }
    if let (Some(p), Some(g)) = (period, granularity)
        && !usage::granularity_allowed(p, g)
    {
        return Err(ApiError::invalid(
            "query.granularity",
            "granularity is not allowed for the period",
        ));
    }
    if (t == WidgetType::LocalUsageBreakdown) != breakdown.is_some() {
        return Err(ApiError::invalid(
            "query.breakdown",
            "breakdown is required only for usage breakdowns",
        ));
    }
    let local = local_type(t);
    for (field, len) in [
        ("query.tools", q.tools.len()),
        ("query.models", q.models.len()),
    ] {
        if !local && len > 0 {
            return Err(ApiError::invalid(
                field,
                "filter does not apply to this widget type",
            ));
        }
    }
    for (field, len) in [
        ("query.metric_kinds", q.metric_kinds.len()),
        ("query.resource_ids", q.resource_ids.len()),
        ("query.dimensions", q.dimensions.len()),
    ] {
        if local && len > 0 {
            return Err(ApiError::invalid(
                field,
                "filter does not apply to this widget type",
            ));
        }
    }
    let resources_apply = matches!(
        t,
        WidgetType::DeploymentResources
            | WidgetType::DeploymentUsage
            | WidgetType::DeploymentCost
            | WidgetType::MetricHistory
    );
    if !resources_apply && !q.resource_ids.is_empty() {
        return Err(ApiError::invalid(
            "query.resource_ids",
            "filter does not apply to this widget type",
        ));
    }
    for (field, len) in [
        ("query.tools", q.tools.len()),
        ("query.models", q.models.len()),
        ("query.metric_kinds", q.metric_kinds.len()),
        ("query.resource_ids", q.resource_ids.len()),
        ("query.dimensions", q.dimensions.len()),
    ] {
        if len > 50 {
            return Err(ApiError::invalid(field, "too many filter values"));
        }
    }
    let mut tools = HashSet::new();
    for tool in &q.tools {
        let tool = validate::known("query.tools", tool)?;
        if !tools.insert(tool.to_i32()) {
            return Err(ApiError::invalid("query.tools", "duplicated value"));
        }
    }
    for m in &q.models {
        validate::token("query.models", m, 1, 200)?;
    }
    for k in &q.metric_kinds {
        let k = validate::known("query.metric_kinds", k)?;
        if !applicable_kind(t, k) {
            return Err(ApiError::invalid(
                "query.metric_kinds",
                "metric kind does not apply to this widget type",
            ));
        }
    }
    for r in &q.resource_ids {
        validate::token("query.resource_ids", r, 1, 200)?;
    }
    for d in &q.dimensions {
        validate::token("query.dimensions", d, 1, 200)?;
    }
    Ok(WidgetQuery {
        period: q.period,
        granularity: q.granularity,
        tools: q.tools.clone(),
        models: q.models.clone(),
        metric_kinds: q.metric_kinds.clone(),
        resource_ids: q.resource_ids.clone(),
        dimensions: q.dimensions.clone(),
        breakdown: q.breakdown,
        ..Default::default()
    })
}

/// A validated widget source owned by the data owner.
pub struct Source {
    pub connections: Vec<Uuid>,
    pub devices: Vec<Uuid>,
    pub all_devices: bool,
}

/// Validates a source scope: only the owner's own, non-deleted connections or devices, in the
/// category the widget type needs.
pub async fn check_source(
    db: &mut PgConnection,
    owner: Uuid,
    t: WidgetType,
    src: &WidgetSource,
) -> Result<Source> {
    let connections = src
        .connection_ids
        .iter()
        .map(|id| validate::id("source.connection_ids", id))
        .collect::<Result<Vec<_>>>()?;
    let devices = src
        .device_ids
        .iter()
        .map(|id| validate::id("source.device_ids", id))
        .collect::<Result<Vec<_>>>()?;
    let unique = |v: &[Uuid]| v.iter().collect::<HashSet<_>>().len() == v.len();
    if !unique(&connections) || !unique(&devices) {
        return Err(ApiError::invalid("source", "duplicated source"));
    }
    if local_type(t) {
        if !connections.is_empty() {
            return Err(ApiError::invalid(
                "source.connection_ids",
                "local usage widgets use devices",
            ));
        }
        if src.all_devices != devices.is_empty() || devices.len() > QUOTA_DEVICES as usize {
            return Err(ApiError::invalid(
                "source.device_ids",
                "select devices or all devices",
            ));
        }
        let owned: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM devices WHERE id=ANY($1) AND user_id=$2 AND deleted_at IS NULL",
        )
        .bind(&devices)
        .bind(owner)
        .fetch_one(db)
        .await?;
        if owned != devices.len() as i64 {
            return Err(ApiError::not_found("source.device_ids"));
        }
    } else {
        if !devices.is_empty() || src.all_devices {
            return Err(ApiError::invalid(
                "source.device_ids",
                "this widget type uses connections",
            ));
        }
        if connections.is_empty() || connections.len() > 50 {
            return Err(ApiError::invalid(
                "source.connection_ids",
                "select 1 to 50 connections",
            ));
        }
        let providers: Vec<i16> = sqlx::query_scalar("SELECT provider FROM connections WHERE id=ANY($1) AND user_id=$2 AND deleted_at IS NULL")
            .bind(&connections)
            .bind(owner)
            .fetch_all(db)
            .await?;
        if providers.len() != connections.len() {
            return Err(ApiError::not_found("source.connection_ids"));
        }
        let ok = providers.iter().all(|p| {
            let p: Provider = enum_of(*p);
            match t {
                WidgetType::SubscriptionLimits | WidgetType::SubscriptionHistory => {
                    subscription_provider(p)
                }
                WidgetType::DeploymentResources
                | WidgetType::DeploymentUsage
                | WidgetType::DeploymentCost => deployment_provider(p),
                _ => true,
            }
        });
        if !ok {
            return Err(ApiError::invalid(
                "source.connection_ids",
                "connection provider does not match the widget type",
            ));
        }
    }
    Ok(Source {
        connections,
        devices,
        all_devices: src.all_devices,
    })
}

async fn write_sources(db: &mut PgConnection, widget: Uuid, source: &Source) -> Result<()> {
    sqlx::query("DELETE FROM widget_sources WHERE widget_id=$1")
        .bind(widget)
        .execute(&mut *db)
        .await?;
    let kinds: Vec<i16> = source
        .connections
        .iter()
        .map(|_| SOURCE_CONNECTION)
        .chain(source.devices.iter().map(|_| SOURCE_DEVICE))
        .collect();
    let ids: Vec<Uuid> = source
        .connections
        .iter()
        .chain(&source.devices)
        .copied()
        .collect();
    sqlx::query("INSERT INTO widget_sources(widget_id,source_kind,source_id) SELECT $1,k,i FROM UNNEST($2::smallint[],$3::uuid[]) AS t(k,i)")
        .bind(widget)
        .bind(kinds)
        .bind(ids)
        .execute(&mut *db)
        .await?;
    sqlx::query("UPDATE widgets SET all_devices=$2 WHERE id=$1")
        .bind(widget)
        .bind(source.all_devices)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn add_widget(s: &Server, call: &Call, q: AddWidgetRequest) -> Result<AddWidgetResponse> {
    let user = s.user(call).await?;
    let id = validate::id("dashboard_id", &q.dashboard_id)?;
    let request_id = validate::id("request_id", &q.request_id)?;
    let (row, space) = dashboard_access(s, call, &user, id).await?;
    if !space.edit() {
        return Err(ApiError::denied());
    }
    let kind = validate::known("type", &q.r#type)?;
    let layout = check_layout(
        "layout",
        kind,
        &q.layout.as_option().cloned().unwrap_or_default(),
    )?;
    let query = check_query(kind, &q.query.as_option().cloned().unwrap_or_default())?;
    let caller = format!("user:{}", user.user_id);
    let print = fingerprint(&q);
    let mut tx = s.db.begin().await?;
    // Lock the dashboard before the replay lookup: a retry that overlaps the original request
    // waits for its commit and then returns the original result instead of VERSION_CONFLICT.
    let locked = lock_version(&mut tx, id, None).await?;
    if let Some(existing) =
        replay(&mut tx, &caller, "AddWidget", request_id, &print, call.now).await?
    {
        let dashboard = dashboard_message(&mut tx, id, user.user_id).await?;
        tx.commit().await?;
        return Ok(AddWidgetResponse {
            dashboard: Some(dashboard).into(),
            widget_id: existing.to_string(),
            ..Default::default()
        });
    }
    if locked.version as u64 != q.expected_version {
        return Err(ApiError::version_conflict(locked.version as u64));
    }
    let source = check_source(
        &mut tx,
        user.user_id,
        kind,
        &q.source.as_option().cloned().unwrap_or_default(),
    )
    .await?;
    let used: i64 = sqlx::query_scalar("SELECT count(*) FROM widgets WHERE dashboard_id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if used >= i64::from(QUOTA_WIDGETS) {
        return Err(ApiError::quota(
            QuotaKind::WidgetsPerDashboard,
            QUOTA_WIDGETS,
            used as u32,
        ));
    }
    let widget = crate::new_id();
    let state = if space.personal {
        SharingState::Private
    } else {
        SharingState::Shared
    };
    sqlx::query(
        "INSERT INTO widgets(id,dashboard_id,data_owner_user_id,type,x,y,width,height,title,presentation,query,all_devices,sharing_state,data_owner_member_since,sort_order,created_at) \
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$15,(SELECT COALESCE(max(sort_order),0)+1 FROM widgets WHERE dashboard_id=$2),$14)",
    )
    .bind(widget)
    .bind(id)
    .bind(user.user_id)
    .bind(e16(kind))
    .bind(layout.x as i32)
    .bind(layout.y as i32)
    .bind(layout.width as i32)
    .bind(layout.height as i32)
    .bind(&layout.title)
    .bind(layout.presentation.to_i32() as i16)
    .bind(serde_json::to_value(&query)?)
    .bind(source.all_devices)
    .bind(e16(state))
    .bind(call.now)
    // The data owner's membership start from this request's role read; a different start in a
    // later read ends sharing with OWNER_LEFT_SPACE.
    .bind(space.member_since)
    .execute(&mut *tx)
    .await?;
    write_sources(&mut tx, widget, &source).await?;
    if !space.personal {
        record(
            &mut tx,
            call,
            Event {
                kind: ActivityKind::WidgetShared,
                actor_user: Some(user.user_id),
                subject_user: Some(user.user_id),
                tenant: Some(row.tenant),
                dashboard: Some(id),
                widget: Some(widget),
                ..Default::default()
            },
        )
        .await?;
    }
    bump(&mut tx, id, call.now).await?;
    remember(
        &mut tx,
        &caller,
        "AddWidget",
        request_id,
        &print,
        widget,
        call.now,
    )
    .await?;
    let dashboard = dashboard_message(&mut tx, id, user.user_id).await?;
    tx.commit().await?;
    Ok(AddWidgetResponse {
        dashboard: Some(dashboard).into(),
        widget_id: widget.to_string(),
        ..Default::default()
    })
}

/// Loads one widget of a dashboard: `(data owner, sharing state)`.
async fn widget_owner(
    db: &mut PgConnection,
    dashboard: Uuid,
    widget: Uuid,
) -> Result<(Uuid, SharingState)> {
    let row: Option<(Uuid, i16)> = sqlx::query_as(
        "SELECT data_owner_user_id,sharing_state FROM widgets WHERE id=$1 AND dashboard_id=$2",
    )
    .bind(widget)
    .bind(dashboard)
    .fetch_optional(db)
    .await?;
    row.map(|(o, s)| (o, enum_of(s)))
        .ok_or_else(|| ApiError::not_found("widget_id"))
}

pub async fn remove_widget(
    s: &Server,
    call: &Call,
    q: RemoveWidgetRequest,
) -> Result<RemoveWidgetResponse> {
    let user = s.user(call).await?;
    let id = validate::id("dashboard_id", &q.dashboard_id)?;
    let widget = validate::id("widget_id", &q.widget_id)?;
    call.note(|a| a.widget = Some(widget));
    let (row, space) = dashboard_access(s, call, &user, id).await?;
    let mut tx = s.db.begin().await?;
    let (owner, state) = widget_owner(&mut tx, id, widget).await?;
    let own = owner == user.user_id;
    if !space.edit() || (!own && !(space.personal || space.roles.manager())) {
        return Err(ApiError::denied());
    }
    lock_version(&mut tx, id, Some(q.expected_version)).await?;
    sqlx::query("DELETE FROM widgets WHERE id=$1")
        .bind(widget)
        .execute(&mut *tx)
        .await?;
    if state == SharingState::Shared {
        record(
            &mut tx,
            call,
            Event {
                kind: ActivityKind::WidgetSharingEnded,
                actor_user: Some(user.user_id),
                subject_user: Some(owner),
                tenant: Some(row.tenant),
                dashboard: Some(id),
                widget: Some(widget),
                ..Default::default()
            },
        )
        .await?;
    }
    bump(&mut tx, id, call.now).await?;
    let dashboard = dashboard_message(&mut tx, id, user.user_id).await?;
    tx.commit().await?;
    Ok(RemoveWidgetResponse {
        dashboard: Some(dashboard).into(),
        ..Default::default()
    })
}

pub async fn update_widget_layout(
    s: &Server,
    call: &Call,
    q: UpdateWidgetLayoutRequest,
) -> Result<UpdateWidgetLayoutResponse> {
    let user = s.user(call).await?;
    let id = validate::id("dashboard_id", &q.dashboard_id)?;
    let (_, space) = dashboard_access(s, call, &user, id).await?;
    if !space.edit() {
        return Err(ApiError::denied());
    }
    if q.updates.is_empty() || q.updates.len() > QUOTA_WIDGETS as usize {
        return Err(ApiError::invalid(
            "updates",
            "supply 1 to 100 layout updates",
        ));
    }
    let mut tx = s.db.begin().await?;
    lock_version(&mut tx, id, Some(q.expected_version)).await?;
    let types: HashMap<Uuid, WidgetType> =
        sqlx::query_as::<_, (Uuid, i16)>("SELECT id,type FROM widgets WHERE dashboard_id=$1")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|(i, t)| (i, enum_of(t)))
            .collect();
    let mut seen = HashSet::new();
    for (i, u) in q.updates.iter().enumerate() {
        let field = format!("updates[{i}]");
        let widget = validate::id(&format!("{field}.widget_id"), &u.widget_id)?;
        let kind = *types
            .get(&widget)
            .ok_or_else(|| ApiError::not_found(&format!("{field}.widget_id")))?;
        if !seen.insert(widget) {
            return Err(ApiError::invalid(
                &format!("{field}.widget_id"),
                "duplicated widget",
            ));
        }
        let layout = check_layout(
            &format!("{field}.layout"),
            kind,
            &u.layout.as_option().cloned().unwrap_or_default(),
        )?;
        sqlx::query(
            "UPDATE widgets SET x=$2,y=$3,width=$4,height=$5,title=$6,presentation=$7 WHERE id=$1",
        )
        .bind(widget)
        .bind(layout.x as i32)
        .bind(layout.y as i32)
        .bind(layout.width as i32)
        .bind(layout.height as i32)
        .bind(&layout.title)
        .bind(layout.presentation.to_i32() as i16)
        .execute(&mut *tx)
        .await?;
    }
    bump(&mut tx, id, call.now).await?;
    let dashboard = dashboard_message(&mut tx, id, user.user_id).await?;
    tx.commit().await?;
    Ok(UpdateWidgetLayoutResponse {
        dashboard: Some(dashboard).into(),
        ..Default::default()
    })
}

fn data_owner_required() -> ApiError {
    ApiError::new(
        ErrorReason::DataOwnerRequired,
        "only the widget's data owner can change its query or sharing",
    )
}

pub async fn update_widget_query(
    s: &Server,
    call: &Call,
    q: UpdateWidgetQueryRequest,
) -> Result<UpdateWidgetQueryResponse> {
    let user = s.user(call).await?;
    let id = validate::id("dashboard_id", &q.dashboard_id)?;
    let widget = validate::id("widget_id", &q.widget_id)?;
    call.note(|a| a.widget = Some(widget));
    let (_, space) = dashboard_access(s, call, &user, id).await?;
    let mut tx = s.db.begin().await?;
    let (owner, _) = widget_owner(&mut tx, id, widget).await?;
    if owner != user.user_id {
        return Err(data_owner_required());
    }
    if !space.edit() {
        return Err(ApiError::denied());
    }
    lock_version(&mut tx, id, Some(q.expected_version)).await?;
    let kind: WidgetType = enum_of(
        sqlx::query_scalar::<_, i16>("SELECT type FROM widgets WHERE id=$1")
            .bind(widget)
            .fetch_one(&mut *tx)
            .await?,
    );
    let query = check_query(kind, &q.query.as_option().cloned().unwrap_or_default())?;
    sqlx::query("UPDATE widgets SET query=$2 WHERE id=$1")
        .bind(widget)
        .bind(serde_json::to_value(&query)?)
        .execute(&mut *tx)
        .await?;
    bump(&mut tx, id, call.now).await?;
    let dashboard = dashboard_message(&mut tx, id, user.user_id).await?;
    tx.commit().await?;
    Ok(UpdateWidgetQueryResponse {
        dashboard: Some(dashboard).into(),
        ..Default::default()
    })
}

pub async fn set_widget_sharing_scope(
    s: &Server,
    call: &Call,
    q: SetWidgetSharingScopeRequest,
) -> Result<SetWidgetSharingScopeResponse> {
    let user = s.user(call).await?;
    let id = validate::id("dashboard_id", &q.dashboard_id)?;
    let widget = validate::id("widget_id", &q.widget_id)?;
    call.note(|a| a.widget = Some(widget));
    let (row, space) = dashboard_access(s, call, &user, id).await?;
    let mut tx = s.db.begin().await?;
    let (owner, _) = widget_owner(&mut tx, id, widget).await?;
    if owner != user.user_id {
        return Err(data_owner_required());
    }
    if !space.edit() {
        return Err(ApiError::denied());
    }
    lock_version(&mut tx, id, Some(q.expected_version)).await?;
    let kind: WidgetType = enum_of(
        sqlx::query_scalar::<_, i16>("SELECT type FROM widgets WHERE id=$1")
            .bind(widget)
            .fetch_one(&mut *tx)
            .await?,
    );
    let source = check_source(
        &mut tx,
        user.user_id,
        kind,
        &q.source.as_option().cloned().unwrap_or_default(),
    )
    .await?;
    write_sources(&mut tx, widget, &source).await?;
    let state = if space.personal {
        SharingState::Private
    } else {
        SharingState::Shared
    };
    // Sharing again records the owner's current membership start, so a rejoined owner can
    // explicitly share a widget that an earlier departure ended.
    sqlx::query("UPDATE widgets SET sharing_state=$2,sharing_end_reason=0,data_owner_member_since=$3 WHERE id=$1")
        .bind(widget)
        .bind(e16(state))
        .bind(space.member_since)
        .execute(&mut *tx)
        .await?;
    if !space.personal {
        record(
            &mut tx,
            call,
            Event {
                kind: ActivityKind::WidgetShared,
                actor_user: Some(user.user_id),
                subject_user: Some(user.user_id),
                tenant: Some(row.tenant),
                dashboard: Some(id),
                widget: Some(widget),
                ..Default::default()
            },
        )
        .await?;
    }
    bump(&mut tx, id, call.now).await?;
    let dashboard = dashboard_message(&mut tx, id, user.user_id).await?;
    tx.commit().await?;
    Ok(SetWidgetSharingScopeResponse {
        dashboard: Some(dashboard).into(),
        ..Default::default()
    })
}

pub async fn revoke_widget_sharing(
    s: &Server,
    call: &Call,
    q: RevokeWidgetSharingRequest,
) -> Result<RevokeWidgetSharingResponse> {
    let user = s.user(call).await?;
    let id = validate::id("dashboard_id", &q.dashboard_id)?;
    let widget = validate::id("widget_id", &q.widget_id)?;
    call.note(|a| a.widget = Some(widget));
    let (row, space) = dashboard_access(s, call, &user, id).await?;
    let mut tx = s.db.begin().await?;
    let (owner, _) = widget_owner(&mut tx, id, widget).await?;
    if owner != user.user_id {
        return Err(data_owner_required());
    }
    lock_version(&mut tx, id, None).await?;
    sqlx::query("UPDATE widgets SET sharing_state=$2,sharing_end_reason=$3 WHERE id=$1")
        .bind(widget)
        .bind(e16(SharingState::Ended))
        .bind(e16(SharingEndReason::OwnerRevoked))
        .execute(&mut *tx)
        .await?;
    if !space.personal {
        record(
            &mut tx,
            call,
            Event {
                kind: ActivityKind::WidgetSharingEnded,
                actor_user: Some(user.user_id),
                subject_user: Some(user.user_id),
                tenant: Some(row.tenant),
                dashboard: Some(id),
                widget: Some(widget),
                ..Default::default()
            },
        )
        .await?;
    }
    bump(&mut tx, id, call.now).await?;
    let dashboard = dashboard_message(&mut tx, id, user.user_id).await?;
    tx.commit().await?;
    Ok(RevokeWidgetSharingResponse {
        dashboard: Some(dashboard).into(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn roles(r: &[Roles]) -> Roles {
        r.iter().fold(Roles::default(), |a, b| a.union(*b))
    }
    #[test]
    fn permission_matrix_by_role() {
        let tenant = crate::new_id();
        let team = |r: Roles| Space {
            tenant,
            personal: false,
            roles: r,
            member_since: Some(Utc::now()),
        };
        let personal = Space {
            tenant,
            personal: true,
            roles: Roles::OWNER,
            member_since: None,
        };
        let check = |s: Space| (s.view(), s.edit(), s.create(), s.delete(), s.activity());
        assert_eq!(check(personal), (true, true, true, true, true));
        assert_eq!(check(team(Roles::OWNER)), (true, true, true, true, true));
        assert_eq!(check(team(Roles::ADMIN)), (true, true, true, true, true));
        assert_eq!(
            check(team(Roles::MEMBER)),
            (true, true, false, false, false)
        );
        assert_eq!(
            check(team(Roles::BILLING)),
            (false, false, false, false, false)
        );
        assert_eq!(
            check(team(Roles::default())),
            (false, false, false, false, false)
        );
        assert_eq!(
            check(team(roles(&[Roles::BILLING, Roles::MEMBER]))),
            (true, true, false, false, false)
        );
        assert_eq!(team(Roles::ADMIN).permissions().len(), 5);
        assert_eq!(team(Roles::MEMBER).permissions().len(), 2);
        assert_eq!(personal.permissions().len(), 4);
    }
    #[test]
    fn layout_errors_name_the_violating_field() {
        let layout = |x, y, width, height| WidgetLayout {
            x,
            y,
            width,
            height,
            presentation: WidgetPresentation::LineChart.into(),
            ..Default::default()
        };
        let field_of = |l: WidgetLayout| {
            check_layout("l", WidgetType::LocalUsageSeries, &l)
                .expect_err("layout must be rejected")
                .detail
                .field
        };
        assert_eq!(field_of(layout(12, 0, 1, 1)), "l.x");
        assert_eq!(field_of(layout(0, 0, 0, 1)), "l.width");
        assert_eq!(field_of(layout(0, 0, 13, 1)), "l.width");
        assert_eq!(field_of(layout(8, 0, 6, 1)), "l.width");
        assert_eq!(field_of(layout(0, 1000, 1, 1)), "l.y");
        assert_eq!(field_of(layout(0, 0, 1, 0)), "l.height");
        assert_eq!(field_of(layout(0, 0, 1, 13)), "l.height");
        // The order of the checks is unchanged: x, then width, then y, then height.
        assert_eq!(field_of(layout(12, 1000, 13, 13)), "l.x");
        assert_eq!(field_of(layout(0, 1000, 13, 13)), "l.width");
        assert_eq!(field_of(layout(0, 1000, 1, 13)), "l.y");
        // Layouts on every edge of the grid are accepted.
        assert!(check_layout("l", WidgetType::LocalUsageSeries, &layout(11, 999, 1, 12)).is_ok());
        assert!(check_layout("l", WidgetType::LocalUsageSeries, &layout(0, 0, 12, 1)).is_ok());
    }
    #[test]
    fn layout_and_query_rules_per_widget_type() {
        let layout = |x, w, p: WidgetPresentation| WidgetLayout {
            x,
            y: 0,
            width: w,
            height: 2,
            title: String::new(),
            presentation: p.into(),
            ..Default::default()
        };
        assert!(
            check_layout(
                "l",
                WidgetType::LocalUsageSeries,
                &layout(0, 12, WidgetPresentation::LineChart)
            )
            .is_ok()
        );
        assert!(
            check_layout(
                "l",
                WidgetType::LocalUsageSeries,
                &layout(6, 7, WidgetPresentation::LineChart)
            )
            .is_err()
        );
        assert!(
            check_layout(
                "l",
                WidgetType::LocalUsageSeries,
                &layout(0, 4, WidgetPresentation::Gauge)
            )
            .is_err()
        );
        let query = |p: QueryPeriod, g: TimeGranularity| WidgetQuery {
            period: p.into(),
            granularity: g.into(),
            ..Default::default()
        };
        assert!(
            check_query(
                WidgetType::LocalUsageSeries,
                &query(QueryPeriod::Today, TimeGranularity::Hour)
            )
            .is_ok()
        );
        assert!(
            check_query(
                WidgetType::LocalUsageSeries,
                &query(QueryPeriod::Last30Days, TimeGranularity::Hour)
            )
            .is_err()
        );
        assert!(
            check_query(
                WidgetType::LocalUsageSummary,
                &query(QueryPeriod::Today, TimeGranularity::Hour)
            )
            .is_err()
        );
        assert!(
            check_query(
                WidgetType::SubscriptionLimits,
                &query(QueryPeriod::Unspecified, TimeGranularity::Unspecified)
            )
            .is_ok()
        );
        assert!(
            check_query(
                WidgetType::SubscriptionLimits,
                &query(QueryPeriod::Today, TimeGranularity::Unspecified)
            )
            .is_err()
        );
        let mut breakdown = query(QueryPeriod::Today, TimeGranularity::Unspecified);
        assert!(check_query(WidgetType::LocalUsageBreakdown, &breakdown).is_err());
        breakdown.breakdown = UsageBreakdownDimension::Model.into();
        assert!(check_query(WidgetType::LocalUsageBreakdown, &breakdown).is_ok());
        let mut kinds = query(QueryPeriod::Unspecified, TimeGranularity::Unspecified);
        kinds.metric_kinds = vec![MetricKind::DeploymentBilledCost.into()];
        assert!(check_query(WidgetType::DeploymentCost, &kinds).is_ok());
        assert!(check_query(WidgetType::SubscriptionLimits, &kinds).is_err());
        kinds.tools = vec![LocalTool::Codex.into()];
        assert!(check_query(WidgetType::DeploymentCost, &kinds).is_err());
    }
}
