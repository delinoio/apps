// SPDX-License-Identifier: FSL-1.1-ALv2
//! AccountService: account connection, settings, activity log and AnyDash data deletion.
use super::*;
use crate::{
    Server,
    audit::{self, Event},
    call::User,
    rate, validate,
};
use sqlx::Row;

const ACTIVITY_DAYS: i64 = 30;

fn own_kinds() -> Vec<i16> {
    [
        ActivityKind::DeviceEnrolled,
        ActivityKind::DeviceReattached,
        ActivityKind::DeviceLoggedOut,
        ActivityKind::DeviceDeleted,
        ActivityKind::DeviceCredentialsRevoked,
        ActivityKind::EnrollmentInvalidated,
        ActivityKind::ConnectionCreated,
        ActivityKind::ConnectionDeleted,
        ActivityKind::ConnectionDeviceRegistered,
        ActivityKind::ConnectionDeviceUnregistered,
        ActivityKind::CollectionDeviceChanged,
        ActivityKind::PermissionDenied,
        ActivityKind::AccountDataDeletionRequested,
    ]
    .into_iter()
    .map(e16)
    .collect()
}

fn space_kinds() -> Vec<i16> {
    [
        ActivityKind::DashboardCreated,
        ActivityKind::DashboardDeleted,
        ActivityKind::WidgetShared,
        ActivityKind::WidgetSharingEnded,
        ActivityKind::PermissionDenied,
    ]
    .into_iter()
    .map(e16)
    .collect()
}

pub async fn account_message(db: &sqlx::PgPool, user: Uuid) -> Result<Account> {
    let row = sqlx::query(
        "SELECT a.user_id,a.personal_tenant_id,a.time_zone,a.preferred_device_id,\
         (SELECT count(*) FROM devices d WHERE d.user_id=a.user_id AND d.deleted_at IS NULL) AS devices,\
         (SELECT count(*) FROM connections c WHERE c.user_id=a.user_id AND c.deleted_at IS NULL) AS connections,\
         (SELECT count(*) FROM dashboards b WHERE b.tenant_id=a.personal_tenant_id AND b.space_kind=$2 AND b.deleted_at IS NULL) AS dashboards,\
         (SELECT COALESCE(max(n),0) FROM (SELECT count(*) AS n FROM widgets w JOIN dashboards b ON b.id=w.dashboard_id \
           WHERE b.tenant_id=a.personal_tenant_id AND b.space_kind=$2 AND b.deleted_at IS NULL GROUP BY b.id) x) AS widgets \
         FROM accounts a WHERE a.user_id=$1",
    )
    .bind(user)
    .bind(e16(SpaceKind::Personal))
    .fetch_one(db)
    .await?;
    let devices: i64 = row.get("devices");
    let quota = |kind: QuotaKind, limit: u32, used: i64| QuotaInfo {
        kind: kind.into(),
        limit,
        used: used.max(0) as u32,
        ..Default::default()
    };
    Ok(Account {
        user_id: row.get::<Uuid, _>("user_id").to_string(),
        personal_space_id: row.get::<Uuid, _>("personal_tenant_id").to_string(),
        time_zone: row.get("time_zone"),
        onboarding_state: if devices > 0 {
            OnboardingState::Ready
        } else {
            OnboardingState::DeviceRequired
        }
        .into(),
        preferred_device_id: row
            .get::<Option<Uuid>, _>("preferred_device_id")
            .map(|d| d.to_string())
            .unwrap_or_default(),
        quotas: vec![
            quota(QuotaKind::CollectorDevicesPerUser, QUOTA_DEVICES, devices),
            quota(
                QuotaKind::ExternalConnectionsPerUser,
                QUOTA_CONNECTIONS,
                row.get("connections"),
            ),
            quota(
                QuotaKind::DashboardsPerSpace,
                QUOTA_DASHBOARDS,
                row.get("dashboards"),
            ),
            quota(
                QuotaKind::WidgetsPerDashboard,
                QUOTA_WIDGETS,
                row.get("widgets"),
            ),
        ],
        ..Default::default()
    })
}

pub async fn connect_account(
    s: &Server,
    call: &Call,
    q: ConnectAccountRequest,
) -> Result<ConnectAccountResponse> {
    let subject = s.subject(call).await?;
    rate::check(
        &s.db,
        RateLimitScope::UserToken,
        &format!("subject:{}", crate::sha256(subject.as_bytes())),
        rate::USER_TOKEN_LIMIT,
    )
    .await?;
    let claims = s
        .verifier
        .verify(&q.delcore_access_token, &s.config.delcore_resource)
        .await?;
    if claims.subject != subject {
        return Err(ApiError::unauthenticated());
    }
    let current = s.delcore.current_user(&q.delcore_access_token).await?;
    let personal: Vec<_> = current
        .tenants
        .iter()
        .filter(|t| t.personal && t.owner_id == current.user_id)
        .collect();
    let [tenant] = personal.as_slice() else {
        tracing::warn!(
            count = personal.len(),
            "Delcore personal tenant is missing or ambiguous"
        );
        return Err(ApiError::dependency());
    };
    s.delcore.require_active(&tenant.id).await?;
    let user_id = validate::id("user_id", &current.user_id).map_err(|_| ApiError::dependency())?;
    let tenant_id =
        validate::id("personal_space_id", &tenant.id).map_err(|_| ApiError::dependency())?;
    call.note(|a| a.user = Some(user_id));
    let mut tx = s.db.begin().await?;
    let other: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM accounts WHERE logto_subject=$1 AND user_id<>$2")
            .bind(&subject)
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if other.is_some() {
        tracing::error!("Logto subject is already mapped to another Delcore user");
        return Err(ApiError::internal());
    }
    sqlx::query(
        "INSERT INTO accounts(user_id,logto_subject,personal_tenant_id,created_at,updated_at) VALUES($1,$2,$3,$4,$4) \
         ON CONFLICT (user_id) DO UPDATE SET logto_subject=EXCLUDED.logto_subject,personal_tenant_id=EXCLUDED.personal_tenant_id,updated_at=EXCLUDED.updated_at",
    )
    .bind(user_id)
    .bind(&subject)
    .bind(tenant_id)
    .bind(call.now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ConnectAccountResponse {
        account: Some(account_message(&s.db, user_id).await?).into(),
        ..Default::default()
    })
}

pub async fn get_account(
    s: &Server,
    call: &Call,
    _q: GetAccountRequest,
) -> Result<GetAccountResponse> {
    let user = s.active_user(call).await?;
    Ok(GetAccountResponse {
        account: Some(account_message(&s.db, user.user_id).await?).into(),
        ..Default::default()
    })
}

pub async fn update_account_settings(
    s: &Server,
    call: &Call,
    q: UpdateAccountSettingsRequest,
) -> Result<UpdateAccountSettingsResponse> {
    let user = s.active_user(call).await?;
    let tz = validate::time_zone("time_zone", &q.time_zone)?;
    sqlx::query("UPDATE accounts SET time_zone=$2,updated_at=$3 WHERE user_id=$1")
        .bind(user.user_id)
        .bind(tz.name())
        .bind(call.now)
        .execute(&s.db)
        .await?;
    Ok(UpdateAccountSettingsResponse {
        account: Some(account_message(&s.db, user.user_id).await?).into(),
        ..Default::default()
    })
}

pub async fn list_activity(
    s: &Server,
    call: &Call,
    q: ListActivityRequest,
) -> Result<ListActivityResponse> {
    let user: User = s.active_user(call).await?;
    let tenant = validate::optional_id("space_id", &q.space_id)?;
    let mut filter = Vec::new();
    for k in &q.kinds {
        filter.push(e16(validate::known("kinds", k)?));
    }
    let allowed = match tenant {
        None => own_kinds(),
        Some(t) => {
            let space = super::dashboard::space(s, call, &user, t, None).await?;
            if !space.activity() {
                return Err(ApiError::denied());
            }
            space_kinds()
        }
    };
    let kinds: Vec<i16> = if filter.is_empty() {
        allowed
    } else {
        filter.into_iter().filter(|k| allowed.contains(k)).collect()
    };
    let page = q.page.as_option().cloned().unwrap_or_default();
    let size = validate::page_size(page.size)?;
    let mut sorted = kinds.clone();
    sorted.sort_unstable();
    let context = format!(
        "{}|ListActivity|{}|{sorted:?}",
        user.user_id,
        tenant.map(|t| t.to_string()).unwrap_or_default()
    );
    let after = decode_cursor(&context, &page.cursor)?;
    let rows = sqlx::query(
        "SELECT id,occurred_at,kind,actor_user_id,tenant_id,device_id,connection_id,dashboard_id,widget_id,correlation_id,client_kind FROM audit_events \
         WHERE occurred_at>$1 AND kind=ANY($2) \
         AND (($3::uuid IS NULL AND subject_user_id=$4 AND tenant_id IS NULL) OR ($3::uuid IS NOT NULL AND tenant_id=$3)) \
         AND ($5::timestamptz IS NULL OR (occurred_at,id)<($5,$6)) ORDER BY occurred_at DESC,id DESC LIMIT $7",
    )
    .bind(call.now - chrono::Duration::days(ACTIVITY_DAYS))
    .bind(&kinds)
    .bind(tenant)
    .bind(user.user_id)
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
            last.get("occurred_at"),
            &last.get::<Uuid, _>("id").to_string(),
        )
    } else {
        String::new()
    };
    let id_text = |r: &sqlx::postgres::PgRow, c: &str| {
        r.get::<Option<Uuid>, _>(c)
            .map(|u| u.to_string())
            .unwrap_or_default()
    };
    Ok(ListActivityResponse {
        events: rows
            .iter()
            .take(size as usize)
            .map(|r| ActivityEvent {
                id: r.get::<Uuid, _>("id").to_string(),
                occurred_at: Some(ts(r.get("occurred_at"))).into(),
                kind: enum_of::<ActivityKind>(r.get("kind")).into(),
                actor_user_id: id_text(r, "actor_user_id"),
                space_id: id_text(r, "tenant_id"),
                device_id: id_text(r, "device_id"),
                connection_id: id_text(r, "connection_id"),
                dashboard_id: id_text(r, "dashboard_id"),
                widget_id: id_text(r, "widget_id"),
                correlation_id: r.get("correlation_id"),
                client_kind: enum_of::<ClientKind>(r.get("client_kind")).into(),
                ..Default::default()
            })
            .collect(),
        next_cursor,
        ..Default::default()
    })
}

/// Blocks all of a user's AnyDash data immediately: devices, connections, personal dashboards
/// and the user's widgets in team dashboards, which end with `reason`.
pub async fn block_user_data(
    db: &mut PgConnection,
    user: Uuid,
    now: DateTime<Utc>,
    correlation: &str,
    client_kind: i32,
    reason: SharingEndReason,
) -> Result<()> {
    sqlx::query("UPDATE devices SET deleted_at=$2 WHERE user_id=$1 AND deleted_at IS NULL")
        .bind(user)
        .bind(now)
        .execute(&mut *db)
        .await?;
    sqlx::query(
        "UPDATE connections SET deleted_at=COALESCE(deleted_at,$2),last_holder_device_id=lease_device_id,lease_device_id=NULL,lease_expires_at=NULL WHERE user_id=$1",
    )
    .bind(user)
    .bind(now)
    .execute(&mut *db)
    .await?;
    sqlx::query(
        "UPDATE dashboards SET deleted_at=$2 WHERE deleted_at IS NULL AND space_kind=$3 AND tenant_id=(SELECT personal_tenant_id FROM accounts WHERE user_id=$1)",
    )
    .bind(user)
    .bind(now)
    .bind(e16(SpaceKind::Personal))
    .execute(&mut *db)
    .await?;
    let ended: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "UPDATE widgets w SET sharing_state=$2,sharing_end_reason=$3 FROM dashboards d \
         WHERE w.data_owner_user_id=$1 AND d.id=w.dashboard_id AND d.space_kind=$4 AND w.sharing_state<>$2 \
         RETURNING w.id,w.dashboard_id,d.tenant_id",
    )
    .bind(user)
    .bind(e16(SharingState::Ended))
    .bind(e16(reason))
    .bind(e16(SpaceKind::Team))
    .fetch_all(&mut *db)
    .await?;
    sqlx::query(
        "DELETE FROM widget_sources s USING widgets w WHERE s.widget_id=w.id AND w.data_owner_user_id=$1",
    )
    .bind(user)
    .execute(&mut *db)
    .await?;
    for (widget, dashboard, tenant) in ended {
        audit::insert(
            db,
            correlation,
            client_kind,
            now,
            Event {
                kind: ActivityKind::WidgetSharingEnded,
                actor_user: Some(user),
                subject_user: Some(user),
                tenant: Some(tenant),
                dashboard: Some(dashboard),
                widget: Some(widget),
                ..Default::default()
            },
        )
        .await?;
    }
    sqlx::query("UPDATE accounts SET preferred_device_id=NULL,data_deleted_at=$2,updated_at=$2 WHERE user_id=$1")
        .bind(user)
        .bind(now)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete_account_data(
    s: &Server,
    call: &Call,
    q: DeleteAccountDataRequest,
) -> Result<DeleteAccountDataResponse> {
    let user = s.active_user(call).await?;
    if !q.confirm {
        return Err(ApiError::invalid("confirm", "confirm the deletion"));
    }
    let mut tx = s.db.begin().await?;
    lock_account(&mut tx, user.user_id).await?;
    block_user_data(
        &mut tx,
        user.user_id,
        call.now,
        &call.correlation,
        call.client_kind.to_i32(),
        SharingEndReason::OwnerDataDeleted,
    )
    .await?;
    let deadline = enqueue_deletion(
        &mut tx,
        DeletionKind::AccountData,
        None,
        Some(user.user_id),
        None,
        call.now,
    )
    .await?;
    audit::record(
        &mut tx,
        call,
        Event {
            kind: ActivityKind::AccountDataDeletionRequested,
            actor_user: Some(user.user_id),
            subject_user: Some(user.user_id),
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await?;
    Ok(DeleteAccountDataResponse {
        purge_deadline: Some(ts(deadline)).into(),
        ..Default::default()
    })
}
