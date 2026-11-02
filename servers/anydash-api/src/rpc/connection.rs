// SPDX-License-Identifier: FSL-1.1-ALv2
//! ConnectionService: external connections and their registered collector devices.
use super::*;
use crate::{
    Server,
    audit::{Event, record},
    call::Principal,
    time, validate,
};
use chrono::Duration;
use sqlx::Row;
use std::collections::HashMap;

/// Device ID, name, registration time, readiness and latest heartbeat.
type Registered = (Uuid, String, DateTime<Utc>, bool, Option<DateTime<Utc>>);

pub fn provider_name(p: Provider) -> &'static str {
    match p {
        Provider::Codex => "Codex",
        Provider::ClaudeCode => "Claude Code",
        Provider::Grok => "Grok",
        Provider::Vercel => "Vercel",
        Provider::Supabase => "Supabase",
        Provider::Cloudflare => "Cloudflare",
        _ => "Connection",
    }
}

pub fn online(heartbeat: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    heartbeat.is_some_and(|h| now - h < Duration::seconds(time::LEASE_SECONDS))
}

/// Derives the connection state with the protocol precedence.
pub fn derive_state(
    registered: usize,
    any_online: bool,
    last_status: CollectionStatus,
    has_result: bool,
) -> ConnectionState {
    if registered == 0 {
        ConnectionState::NoRegisteredDevice
    } else if !any_online {
        ConnectionState::NoRunningCollector
    } else if last_status == CollectionStatus::AuthExpired {
        ConnectionState::AuthExpired
    } else if matches!(
        last_status,
        CollectionStatus::Failed | CollectionStatus::RateLimited
    ) {
        ConnectionState::Error
    } else if !has_result {
        ConnectionState::WaitingForFirstResult
    } else {
        ConnectionState::Ok
    }
}

/// Builds `Connection` messages of one user, optionally limited to IDs or to connections a
/// device is registered for, ordered by `created_at` and ID.
pub async fn connections(
    db: &mut PgConnection,
    user: Uuid,
    ids: Option<&[Uuid]>,
    now: DateTime<Utc>,
) -> Result<Vec<Connection>> {
    let rows = sqlx::query(
        "SELECT id,provider,display_name,account_label,account_fingerprint,created_at,lease_epoch,lease_device_id,lease_acquired_at,lease_expires_at,\
         last_manual_refresh_at,latest_observed_at,last_success_at,last_status,last_failure_class,last_error_correlation_id \
         FROM connections WHERE user_id=$1 AND deleted_at IS NULL AND ($2::uuid[] IS NULL OR id=ANY($2)) ORDER BY created_at,id",
    )
    .bind(user)
    .bind(ids.map(|i| i.to_vec()))
    .fetch_all(&mut *db)
    .await?;
    let conn_ids: Vec<Uuid> = rows.iter().map(|r| r.get("id")).collect();
    let devs = sqlx::query(
        "SELECT cd.connection_id,cd.device_id,d.name,cd.registered_at,cd.ready,d.last_heartbeat_at FROM connection_devices cd \
         JOIN devices d ON d.id=cd.device_id WHERE cd.connection_id=ANY($1) AND d.deleted_at IS NULL ORDER BY cd.registered_at,cd.device_id",
    )
    .bind(&conn_ids)
    .fetch_all(&mut *db)
    .await?;
    let mut by_conn: HashMap<Uuid, Vec<Registered>> = HashMap::new();
    for d in devs {
        by_conn.entry(d.get("connection_id")).or_default().push((
            d.get("device_id"),
            d.get("name"),
            d.get("registered_at"),
            d.get("ready"),
            d.get("last_heartbeat_at"),
        ));
    }
    Ok(rows
        .into_iter()
        .map(|r| {
            let id: Uuid = r.get("id");
            let holder: Option<Uuid> = r.get("lease_device_id");
            let expires: Option<DateTime<Utc>> = r.get("lease_expires_at");
            let valid_holder = holder.filter(|_| expires.is_some_and(|e| e > now));
            let devices = by_conn.remove(&id).unwrap_or_default();
            let any_online = devices.iter().any(|d| online(d.4, now));
            let status: CollectionStatus = enum_of(r.get("last_status"));
            let last_manual: Option<DateTime<Utc>> = r.get("last_manual_refresh_at");
            let next_manual = last_manual
                .map(|l| l + Duration::seconds(60))
                .filter(|n| *n > now)
                .unwrap_or(now);
            Connection {
                id: id.to_string(),
                provider: enum_of::<Provider>(r.get("provider")).into(),
                display_name: r.get("display_name"),
                account_label: r.get("account_label"),
                account_fingerprint: r.get("account_fingerprint"),
                created_at: Some(ts(r.get("created_at"))).into(),
                state: derive_state(
                    devices.len(),
                    any_online,
                    status,
                    r.get::<Option<DateTime<Utc>>, _>("latest_observed_at")
                        .is_some(),
                )
                .into(),
                last_success_at: r
                    .get::<Option<DateTime<Utc>>, _>("last_success_at")
                    .map(ts)
                    .into(),
                last_failure_class: enum_of::<FailureClass>(r.get("last_failure_class")).into(),
                last_error_correlation_id: r.get("last_error_correlation_id"),
                devices: devices
                    .iter()
                    .map(|d| ConnectionDevice {
                        device_id: d.0.to_string(),
                        device_name: d.1.clone(),
                        registered_at: Some(ts(d.2)).into(),
                        ready: d.3,
                        online: online(d.4, now),
                        collecting: valid_holder == Some(d.0),
                        ..Default::default()
                    })
                    .collect(),
                lease: valid_holder
                    .map(|h| Lease {
                        connection_id: id.to_string(),
                        holder_device_id: h.to_string(),
                        epoch: r.get::<i64, _>("lease_epoch") as u64,
                        acquired_at: r
                            .get::<Option<DateTime<Utc>>, _>("lease_acquired_at")
                            .map(ts)
                            .into(),
                        expires_at: expires.map(ts).into(),
                        ..Default::default()
                    })
                    .into(),
                next_manual_refresh_at: Some(ts(next_manual)).into(),
                ..Default::default()
            }
        })
        .collect())
}

pub async fn connection_message(
    db: &mut PgConnection,
    user: Uuid,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<Connection> {
    connections(db, user, Some(&[id]), now)
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("connection_id"))
}

/// Requires a connection visible to the principal: the user's own connection, or for a device
/// token one the device is registered for.
async fn visible(db: &mut PgConnection, p: &Principal, id: Uuid) -> Result<()> {
    let found: bool = match p {
        Principal::User(u) => sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM connections WHERE id=$1 AND user_id=$2 AND deleted_at IS NULL)",
        )
        .bind(id)
        .bind(u.user_id)
        .fetch_one(&mut *db)
        .await?,
        Principal::Device(d) => sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM connections c JOIN connection_devices cd ON cd.connection_id=c.id \
             WHERE c.id=$1 AND c.user_id=$2 AND c.deleted_at IS NULL AND cd.device_id=$3)",
        )
        .bind(id)
        .bind(d.user_id)
        .bind(d.device_id)
        .fetch_one(&mut *db)
        .await?,
    };
    if found {
        Ok(())
    } else {
        Err(ApiError::not_found("connection_id"))
    }
}

pub async fn create_connection(
    s: &Server,
    call: &Call,
    q: CreateConnectionRequest,
) -> Result<CreateConnectionResponse> {
    let device = s.device(call).await?;
    let request_id = validate::id("request_id", &q.request_id)?;
    let provider = validate::known("provider", &q.provider)?;
    let display_name = if q.display_name.trim().is_empty() {
        provider_name(provider).to_owned()
    } else {
        validate::text("display_name", &q.display_name, 1, 100)?
    };
    let account_label = validate::text("account_label", &q.account_label, 0, 200)?;
    validate::fingerprint("account_fingerprint", &q.account_fingerprint)?;
    let caller = format!("device:{}", device.device_id);
    let print = fingerprint(&q);
    let mut tx = s.db.begin().await?;
    lock_account(&mut tx, device.user_id).await?;
    if let Some(existing) = replay(
        &mut tx,
        &caller,
        "CreateConnection",
        request_id,
        &print,
        call.now,
    )
    .await?
    {
        let connection = connection_message(&mut tx, device.user_id, existing, call.now).await?;
        tx.commit().await?;
        return Ok(CreateConnectionResponse {
            connection: Some(connection).into(),
            ..Default::default()
        });
    }
    let used: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM connections WHERE user_id=$1 AND deleted_at IS NULL",
    )
    .bind(device.user_id)
    .fetch_one(&mut *tx)
    .await?;
    if used >= i64::from(QUOTA_CONNECTIONS) {
        return Err(ApiError::quota(
            QuotaKind::ExternalConnectionsPerUser,
            QUOTA_CONNECTIONS,
            used as u32,
        ));
    }
    let id = crate::new_id();
    sqlx::query(
        "INSERT INTO connections(id,user_id,provider,display_name,account_label,account_fingerprint,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(id)
    .bind(device.user_id)
    .bind(e16(provider))
    .bind(&display_name)
    .bind(&account_label)
    .bind(&q.account_fingerprint)
    .bind(call.now)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO connection_devices(connection_id,device_id,registered_at) VALUES($1,$2,$3)",
    )
    .bind(id)
    .bind(device.device_id)
    .bind(call.now)
    .execute(&mut *tx)
    .await?;
    record(
        &mut tx,
        call,
        Event {
            kind: ActivityKind::ConnectionCreated,
            actor_device: Some(device.device_id),
            subject_user: Some(device.user_id),
            device: Some(device.device_id),
            connection: Some(id),
            ..Default::default()
        },
    )
    .await?;
    remember(
        &mut tx,
        &caller,
        "CreateConnection",
        request_id,
        &print,
        id,
        call.now,
    )
    .await?;
    let connection = connection_message(&mut tx, device.user_id, id, call.now).await?;
    tx.commit().await?;
    Ok(CreateConnectionResponse {
        connection: Some(connection).into(),
        ..Default::default()
    })
}

pub async fn list_connections(
    s: &Server,
    call: &Call,
    q: ListConnectionsRequest,
) -> Result<ListConnectionsResponse> {
    let p = s.principal(call).await?;
    let page = q.page.as_option().cloned().unwrap_or_default();
    let size = validate::page_size(page.size)?;
    let (context, device) = match &p {
        Principal::User(u) => (format!("user:{}|ListConnections", u.user_id), None),
        Principal::Device(d) => (
            format!("device:{}|ListConnections", d.device_id),
            Some(d.device_id),
        ),
    };
    let after = decode_cursor(&context, &page.cursor)?;
    let mut db = s.db.acquire().await?;
    let ids: Vec<(Uuid, DateTime<Utc>)> = sqlx::query_as(
        "SELECT c.id,c.created_at FROM connections c WHERE c.user_id=$1 AND c.deleted_at IS NULL \
         AND ($2::uuid IS NULL OR EXISTS(SELECT 1 FROM connection_devices cd WHERE cd.connection_id=c.id AND cd.device_id=$2)) \
         AND ($3::timestamptz IS NULL OR (c.created_at,c.id)>($3,$4)) ORDER BY c.created_at,c.id LIMIT $5",
    )
    .bind(p.user_id())
    .bind(device)
    .bind(after.map(|a| a.0))
    .bind(after.map(|a| a.1).unwrap_or_default())
    .bind(size + 1)
    .fetch_all(&mut *db)
    .await?;
    let more = ids.len() as i64 > size;
    let page_ids: Vec<Uuid> = ids.iter().take(size as usize).map(|r| r.0).collect();
    let next_cursor = if more {
        let (id, at) = ids[size as usize - 1];
        encode_cursor(&context, at, &id.to_string())
    } else {
        String::new()
    };
    Ok(ListConnectionsResponse {
        connections: connections(&mut db, p.user_id(), Some(&page_ids), call.now).await?,
        next_cursor,
        ..Default::default()
    })
}

pub async fn get_connection(
    s: &Server,
    call: &Call,
    q: GetConnectionRequest,
) -> Result<GetConnectionResponse> {
    let p = s.principal(call).await?;
    let id = validate::id("connection_id", &q.connection_id)?;
    let mut db = s.db.acquire().await?;
    visible(&mut db, &p, id).await?;
    Ok(GetConnectionResponse {
        connection: Some(connection_message(&mut db, p.user_id(), id, call.now).await?).into(),
        ..Default::default()
    })
}

/// Blocks a connection immediately and enqueues its physical deletion.
pub async fn delete_connection_in(
    db: &mut PgConnection,
    call: &Call,
    user: Uuid,
    id: Uuid,
) -> Result<DateTime<Utc>> {
    sqlx::query(
        "UPDATE connections SET deleted_at=$2,last_holder_device_id=lease_device_id,lease_device_id=NULL,lease_expires_at=NULL WHERE id=$1",
    )
    .bind(id)
    .bind(call.now)
    .execute(&mut *db)
    .await?;
    detach_source(db, call, SOURCE_CONNECTION, id).await?;
    enqueue_deletion(
        db,
        DeletionKind::Connection,
        Some(id),
        Some(user),
        None,
        call.now,
    )
    .await
}

pub async fn delete_connection(
    s: &Server,
    call: &Call,
    q: DeleteConnectionRequest,
) -> Result<DeleteConnectionResponse> {
    let user = s.active_user(call).await?;
    let id = validate::id("connection_id", &q.connection_id)?;
    let mut tx = s.db.begin().await?;
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM connections WHERE id=$1 AND user_id=$2 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if found.is_none() {
        return Err(ApiError::not_found("connection_id"));
    }
    let deadline = delete_connection_in(&mut tx, call, user.user_id, id).await?;
    record(
        &mut tx,
        call,
        Event {
            kind: ActivityKind::ConnectionDeleted,
            actor_user: Some(user.user_id),
            subject_user: Some(user.user_id),
            connection: Some(id),
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await?;
    Ok(DeleteConnectionResponse {
        purge_deadline: Some(ts(deadline)).into(),
        ..Default::default()
    })
}

pub async fn register_connection_device(
    s: &Server,
    call: &Call,
    q: RegisterConnectionDeviceRequest,
) -> Result<RegisterConnectionDeviceResponse> {
    let device = s.device(call).await?;
    let id = validate::id("connection_id", &q.connection_id)?;
    validate::fingerprint("account_fingerprint", &q.account_fingerprint)?;
    let mut tx = s.db.begin().await?;
    // The registration insert takes a key-share lock on the device row; lock the device before the
    // connection, as `Heartbeat` does, so the two calls cannot deadlock.
    lock_device(&mut tx, device.device_id).await?;
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT account_fingerprint FROM connections WHERE id=$1 AND user_id=$2 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .bind(device.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(stored) = stored else {
        return Err(ApiError::not_found("connection_id"));
    };
    if !stored.is_empty() && !q.account_fingerprint.is_empty() && stored != q.account_fingerprint {
        return Err(ApiError::precondition(
            ErrorReason::ConnectionAccountMismatch,
            "the provider account differs from the connection's account",
        ));
    }
    if stored.is_empty() && !q.account_fingerprint.is_empty() {
        sqlx::query("UPDATE connections SET account_fingerprint=$2 WHERE id=$1")
            .bind(id)
            .bind(&q.account_fingerprint)
            .execute(&mut *tx)
            .await?;
    }
    let inserted = sqlx::query(
        "INSERT INTO connection_devices(connection_id,device_id,registered_at) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(device.device_id)
    .bind(call.now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 1 {
        record(
            &mut tx,
            call,
            Event {
                kind: ActivityKind::ConnectionDeviceRegistered,
                actor_device: Some(device.device_id),
                subject_user: Some(device.user_id),
                device: Some(device.device_id),
                connection: Some(id),
                ..Default::default()
            },
        )
        .await?;
    }
    let connection = connection_message(&mut tx, device.user_id, id, call.now).await?;
    tx.commit().await?;
    Ok(RegisterConnectionDeviceResponse {
        connection: Some(connection).into(),
        ..Default::default()
    })
}

pub async fn unregister_connection_device(
    s: &Server,
    call: &Call,
    q: UnregisterConnectionDeviceRequest,
) -> Result<UnregisterConnectionDeviceResponse> {
    let p = s.principal(call).await?;
    let id = validate::id("connection_id", &q.connection_id)?;
    let target = match &p {
        Principal::Device(d) => match validate::optional_id("device_id", &q.device_id)? {
            None => d.device_id,
            Some(other) if other == d.device_id => other,
            Some(_) => return Err(ApiError::denied()),
        },
        Principal::User(_) => validate::id("device_id", &q.device_id)?,
    };
    let mut tx = s.db.begin().await?;
    // Lock the target device before the connection and its registration row, as `Heartbeat` does,
    // so the two calls cannot deadlock. Another user's device is neither locked here nor matched by
    // the deletion below.
    sqlx::query("SELECT 1 FROM devices WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(target)
        .bind(p.user_id())
        .execute(&mut *tx)
        .await?;
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM connections WHERE id=$1 AND user_id=$2 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .bind(p.user_id())
    .fetch_optional(&mut *tx)
    .await?;
    if found.is_none() {
        return Err(ApiError::not_found("connection_id"));
    }
    let removed = sqlx::query(
        "DELETE FROM connection_devices cd USING devices d WHERE cd.connection_id=$1 AND cd.device_id=$2 AND d.id=cd.device_id AND d.user_id=$3",
    )
    .bind(id)
    .bind(target)
    .bind(p.user_id())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if removed == 0 {
        return Err(ApiError::not_found("device_id"));
    }
    sqlx::query(
        "UPDATE connections SET last_holder_device_id=lease_device_id,lease_device_id=NULL,lease_expires_at=NULL WHERE id=$1 AND lease_device_id=$2",
    )
    .bind(id)
    .bind(target)
    .execute(&mut *tx)
    .await?;
    let (actor_user, actor_device) = match &p {
        Principal::User(u) => (Some(u.user_id), None),
        Principal::Device(d) => (None, Some(d.device_id)),
    };
    record(
        &mut tx,
        call,
        Event {
            kind: ActivityKind::ConnectionDeviceUnregistered,
            actor_user,
            actor_device,
            subject_user: Some(p.user_id()),
            device: Some(target),
            connection: Some(id),
            ..Default::default()
        },
    )
    .await?;
    let connection = connection_message(&mut tx, p.user_id(), id, call.now).await?;
    tx.commit().await?;
    Ok(UnregisterConnectionDeviceResponse {
        connection: Some(connection).into(),
        ..Default::default()
    })
}

pub async fn request_connection_refresh(
    s: &Server,
    call: &Call,
    q: RequestConnectionRefreshRequest,
) -> Result<RequestConnectionRefreshResponse> {
    let p = s.principal(call).await?;
    let id = validate::id("connection_id", &q.connection_id)?;
    let mut tx = s.db.begin().await?;
    visible(&mut tx, &p, id).await?;
    let last: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT last_manual_refresh_at FROM connections WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    super::device::manual_refresh_allowed(last, call.now)?;
    sqlx::query(
        "UPDATE connections SET refresh_requested=true,last_manual_refresh_at=$2 WHERE id=$1",
    )
    .bind(id)
    .bind(call.now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(RequestConnectionRefreshResponse {
        next_allowed_at: Some(ts(call.now + Duration::seconds(60))).into(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_state_precedence() {
        use CollectionStatus as S;
        assert_eq!(
            derive_state(0, true, S::Failed, true),
            ConnectionState::NoRegisteredDevice
        );
        assert_eq!(
            derive_state(1, false, S::AuthExpired, true),
            ConnectionState::NoRunningCollector
        );
        assert_eq!(
            derive_state(1, true, S::AuthExpired, true),
            ConnectionState::AuthExpired
        );
        assert_eq!(
            derive_state(1, true, S::RateLimited, true),
            ConnectionState::Error
        );
        assert_eq!(
            derive_state(1, true, S::Failed, true),
            ConnectionState::Error
        );
        assert_eq!(
            derive_state(1, true, S::Unspecified, false),
            ConnectionState::WaitingForFirstResult
        );
        assert_eq!(
            derive_state(1, true, S::Succeeded, true),
            ConnectionState::Ok
        );
    }
}
