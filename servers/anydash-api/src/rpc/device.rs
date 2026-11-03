// SPDX-License-Identifier: FSL-1.1-ALv2
//! DeviceService: enrollment, device credentials and device management.
use super::*;
use crate::{
    Server,
    audit::{Event, record},
    auth::{Bearer, secret, valid_token},
    rate, time, validate,
};
use chrono::Duration;
use rand::Rng;
use sqlx::{Postgres, Row, Transaction};
use std::collections::HashMap;

const ENROLLMENT_MINUTES: i64 = 10;
const MAX_ATTEMPTS: i32 = 5;
const POLL_SECONDS: i64 = 5;
const ACCESS_HOURS: i64 = 1;
const USER_CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";

fn user_code() -> String {
    let mut rng = rand::rng();
    let code: String = (0..8)
        .map(|_| USER_CODE_ALPHABET[rng.random_range(0..USER_CODE_ALPHABET.len())] as char)
        .collect();
    format!("{}-{}", &code[..4], &code[4..])
}

/// Comparison ignores case, whitespace and the hyphen.
pub fn normalize_user_code(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_uppercase)
        .collect()
}

fn code_digest(code: &str) -> String {
    crate::sha256(normalize_user_code(code).as_bytes())
}

/// Builds `Device` messages for the given rows of one user, including parser statuses.
pub async fn devices(
    db: &mut PgConnection,
    user: Uuid,
    ids: Option<&[Uuid]>,
    now: DateTime<Utc>,
) -> Result<Vec<Device>> {
    let preferred: Option<Uuid> =
        sqlx::query_scalar("SELECT preferred_device_id FROM accounts WHERE user_id=$1")
            .bind(user)
            .fetch_optional(&mut *db)
            .await?
            .flatten();
    let rows = sqlx::query(
        "SELECT id,name,platform,architecture,collector_kind,collector_version,os_version,enrolled_at,last_heartbeat_at,last_upload_at,credential_state \
         FROM devices WHERE user_id=$1 AND deleted_at IS NULL AND ($2::uuid[] IS NULL OR id=ANY($2)) ORDER BY enrolled_at,id",
    )
    .bind(user)
    .bind(ids.map(|i| i.to_vec()))
    .fetch_all(&mut *db)
    .await?;
    let device_ids: Vec<Uuid> = rows.iter().map(|r| r.get("id")).collect();
    let statuses = sqlx::query(
        "SELECT device_id,tool,state,skipped_record_count,observed_at,parser_version,source_path_count FROM device_parser_status WHERE device_id=ANY($1) ORDER BY tool",
    )
    .bind(&device_ids)
    .fetch_all(&mut *db)
    .await?;
    let mut by_device: HashMap<Uuid, Vec<ParserStatus>> = HashMap::new();
    for s in statuses {
        by_device
            .entry(s.get("device_id"))
            .or_default()
            .push(ParserStatus {
                tool: enum_of::<LocalTool>(s.get("tool")).into(),
                state: enum_of::<ParserState>(s.get("state")).into(),
                skipped_record_count: s.get::<i64, _>("skipped_record_count") as u64,
                observed_at: s
                    .get::<Option<DateTime<Utc>>, _>("observed_at")
                    .map(ts)
                    .into(),
                parser_version: s.get("parser_version"),
                source_path_count: s.get::<i32, _>("source_path_count") as u32,
                ..Default::default()
            });
    }
    Ok(rows
        .into_iter()
        .map(|r| {
            let id: Uuid = r.get("id");
            let heartbeat: Option<DateTime<Utc>> = r.get("last_heartbeat_at");
            Device {
                id: id.to_string(),
                name: r.get("name"),
                platform: enum_of::<DevicePlatform>(r.get("platform")).into(),
                architecture: enum_of::<CpuArchitecture>(r.get("architecture")).into(),
                collector_kind: enum_of::<CollectorKind>(r.get("collector_kind")).into(),
                collector_version: r.get("collector_version"),
                os_version: r.get("os_version"),
                enrolled_at: Some(ts(r.get("enrolled_at"))).into(),
                last_heartbeat_at: heartbeat.map(ts).into(),
                last_upload_at: r
                    .get::<Option<DateTime<Utc>>, _>("last_upload_at")
                    .map(ts)
                    .into(),
                credential_state: enum_of::<DeviceCredentialState>(r.get("credential_state"))
                    .into(),
                preferred: preferred == Some(id),
                online: heartbeat.is_some_and(|h| now - h < Duration::seconds(time::LEASE_SECONDS)),
                parser_statuses: by_device.remove(&id).unwrap_or_default(),
                ..Default::default()
            }
        })
        .collect())
}

pub async fn device_message(
    db: &mut PgConnection,
    user: Uuid,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<Device> {
    devices(db, user, Some(&[id]), now)
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("device_id"))
}

pub async fn start_enrollment(
    s: &Server,
    call: &Call,
    q: StartEnrollmentRequest,
) -> Result<StartEnrollmentResponse> {
    let subject = format!("start:{}", call.client_ip);
    rate::check(
        &s.db,
        RateLimitScope::Enrollment,
        &subject,
        rate::START_ENROLLMENT_LIMIT,
    )
    .await?;
    let name = validate::text("device_name", &q.device_name, 1, 100)?;
    let platform = validate::known("platform", &q.platform)?;
    let architecture = validate::known("architecture", &q.architecture)?;
    let collector_kind = validate::known("collector_kind", &q.collector_kind)?;
    validate::version_text("collector_version", &q.collector_version)?;
    validate::version_text("os_version", &q.os_version)?;
    let previous = validate::optional_id("previous_device_id", &q.previous_device_id)?;
    let id = crate::new_id();
    let device_code = secret("");
    let user_code = user_code();
    let expires = call.now + Duration::minutes(ENROLLMENT_MINUTES);
    sqlx::query(
        "INSERT INTO enrollments(id,device_code_sha256,user_code_sha256,state,device_name,platform,architecture,collector_kind,collector_version,os_version,previous_device_id,created_at,expires_at) \
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
    )
    .bind(id)
    .bind(crate::sha256(device_code.as_bytes()))
    .bind(code_digest(&user_code))
    .bind(e16(EnrollmentState::Pending))
    .bind(&name)
    .bind(e16(platform))
    .bind(e16(architecture))
    .bind(e16(collector_kind))
    .bind(&q.collector_version)
    .bind(&q.os_version)
    .bind(previous)
    .bind(call.now)
    .bind(expires)
    .execute(&s.db)
    .await?;
    tracing::info!(enrollment_id = %id, "enrollment started");
    Ok(StartEnrollmentResponse {
        enrollment_id: id.to_string(),
        device_code,
        user_code,
        verification_url: format!("{}/connect/{id}", s.config.web_origin),
        expires_at: Some(ts(expires)).into(),
        poll_interval: Some(time::seconds(POLL_SECONDS)).into(),
        ..Default::default()
    })
}

struct EnrollmentRow {
    id: Uuid,
    state: EnrollmentState,
    device_name: String,
    platform: i16,
    architecture: i16,
    collector_kind: i16,
    collector_version: String,
    os_version: String,
    previous_device_id: Option<Uuid>,
    device_id: Option<Uuid>,
    approved_by: Option<Uuid>,
    failed_attempts: i32,
    expires_at: DateTime<Utc>,
    user_code_sha256: String,
    device_code_sha256: String,
    last_polled_at: Option<DateTime<Utc>>,
}

async fn load_enrollment(db: &mut PgConnection, id: Uuid, lock: bool) -> Result<EnrollmentRow> {
    let sql = format!(
        "SELECT id,state,device_name,platform,architecture,collector_kind,collector_version,os_version,previous_device_id,device_id,approved_by_user_id,failed_attempts,expires_at,user_code_sha256,device_code_sha256,last_polled_at FROM enrollments WHERE id=$1{}",
        if lock { " FOR UPDATE" } else { "" }
    );
    let r = sqlx::query(&sql)
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| ApiError::not_found("enrollment_id"))?;
    Ok(EnrollmentRow {
        id: r.get("id"),
        state: enum_of(r.get("state")),
        device_name: r.get("device_name"),
        platform: r.get("platform"),
        architecture: r.get("architecture"),
        collector_kind: r.get("collector_kind"),
        collector_version: r.get("collector_version"),
        os_version: r.get("os_version"),
        previous_device_id: r.get("previous_device_id"),
        device_id: r.get("device_id"),
        approved_by: r.get("approved_by_user_id"),
        failed_attempts: r.get("failed_attempts"),
        expires_at: r.get("expires_at"),
        user_code_sha256: r.get("user_code_sha256"),
        device_code_sha256: r.get("device_code_sha256"),
        last_polled_at: r.get("last_polled_at"),
    })
}

/// Whether the enrollment code lifetime ends the enrollment in its stored state. Approval does
/// not deliver credentials, so an approved enrollment that was not consumed by a poll before its
/// expiry expires like a pending one.
fn expires_in(state: EnrollmentState) -> bool {
    matches!(state, EnrollmentState::Pending | EnrollmentState::Approved)
}

/// The effective state, treating a pending or approved-but-unconsumed enrollment past its expiry
/// as expired.
fn effective(e: &EnrollmentRow, now: DateTime<Utc>) -> EnrollmentState {
    if expires_in(e.state) && e.expires_at <= now {
        EnrollmentState::Expired
    } else {
        e.state
    }
}

fn state_error(state: EnrollmentState) -> ApiError {
    match state {
        EnrollmentState::Expired => {
            ApiError::precondition(ErrorReason::EnrollmentExpired, "the enrollment expired")
        }
        EnrollmentState::Approved | EnrollmentState::Consumed => ApiError::precondition(
            ErrorReason::EnrollmentAlreadyUsed,
            "the enrollment was already used",
        ),
        _ => ApiError::precondition(
            ErrorReason::EnrollmentInvalidated,
            "the enrollment is no longer valid",
        ),
    }
}

async fn mark_expired(db: &mut PgConnection, e: &EnrollmentRow, now: DateTime<Utc>) -> Result<()> {
    if expires_in(e.state) && e.expires_at <= now {
        sqlx::query("UPDATE enrollments SET state=$2 WHERE id=$1")
            .bind(e.id)
            .bind(e16(EnrollmentState::Expired))
            .execute(db)
            .await?;
        if e.state == EnrollmentState::Approved {
            tracing::info!(enrollment_id = %e.id, device_id = ?e.device_id, "approved enrollment expired before credential delivery");
        }
    }
    Ok(())
}

async fn owned_device(db: &mut PgConnection, user: Uuid, id: Option<Uuid>) -> Result<Option<Uuid>> {
    let Some(id) = id else { return Ok(None) };
    Ok(sqlx::query_scalar(
        "SELECT id FROM devices WHERE id=$1 AND user_id=$2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user)
    .fetch_optional(db)
    .await?)
}

pub async fn get_enrollment(
    s: &Server,
    call: &Call,
    q: GetEnrollmentRequest,
) -> Result<GetEnrollmentResponse> {
    let user = s.active_user(call).await?;
    let id = validate::id("enrollment_id", &q.enrollment_id)?;
    let mut db = s.db.acquire().await?;
    let e = load_enrollment(&mut db, id, false).await?;
    let reattach = owned_device(&mut db, user.user_id, e.previous_device_id)
        .await?
        .is_some();
    Ok(GetEnrollmentResponse {
        enrollment: Some(EnrollmentSummary {
            enrollment_id: e.id.to_string(),
            state: effective(&e, call.now).into(),
            device_name: e.device_name.clone(),
            platform: enum_of::<DevicePlatform>(e.platform).into(),
            architecture: enum_of::<CpuArchitecture>(e.architecture).into(),
            collector_kind: enum_of::<CollectorKind>(e.collector_kind).into(),
            collector_version: e.collector_version.clone(),
            expires_at: Some(ts(e.expires_at)).into(),
            remaining_attempts: (MAX_ATTEMPTS - e.failed_attempts).max(0) as u32,
            reattach,
            ..Default::default()
        })
        .into(),
        ..Default::default()
    })
}

pub async fn approve_enrollment(
    s: &Server,
    call: &Call,
    q: ApproveEnrollmentRequest,
) -> Result<ApproveEnrollmentResponse> {
    let user = s.active_user(call).await?;
    let id = validate::id("enrollment_id", &q.enrollment_id)?;
    let mut tx = s.db.begin().await?;
    let e = load_enrollment(&mut tx, id, true).await?;
    let state = effective(&e, call.now);
    if state != EnrollmentState::Pending {
        mark_expired(&mut tx, &e, call.now).await?;
        tx.commit().await?;
        return Err(state_error(state));
    }
    if code_digest(&q.user_code) != e.user_code_sha256 {
        let attempts = e.failed_attempts + 1;
        let invalidated = attempts >= MAX_ATTEMPTS;
        sqlx::query("UPDATE enrollments SET failed_attempts=$2,state=$3 WHERE id=$1")
            .bind(e.id)
            .bind(attempts)
            .bind(e16(if invalidated {
                EnrollmentState::Invalidated
            } else {
                EnrollmentState::Pending
            }))
            .execute(&mut *tx)
            .await?;
        if invalidated {
            record(
                &mut tx,
                call,
                Event {
                    kind: ActivityKind::EnrollmentInvalidated,
                    actor_user: Some(user.user_id),
                    subject_user: Some(user.user_id),
                    ..Default::default()
                },
            )
            .await?;
        }
        tx.commit().await?;
        return Err(ApiError::new(
            ErrorReason::EnrollmentCodeMismatch,
            "the code does not match",
        )
        .field("user_code"));
    }
    let reattach = owned_device(&mut tx, user.user_id, e.previous_device_id).await?;
    let device = match reattach {
        Some(device) => {
            sqlx::query(
                "UPDATE devices SET name=$2,platform=$3,architecture=$4,collector_kind=$5,collector_version=$6,os_version=$7 WHERE id=$1",
            )
            .bind(device)
            .bind(&e.device_name)
            .bind(e.platform)
            .bind(e.architecture)
            .bind(e.collector_kind)
            .bind(&e.collector_version)
            .bind(&e.os_version)
            .execute(&mut *tx)
            .await?;
            device
        }
        None => {
            lock_account(&mut tx, user.user_id).await?;
            let used: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM devices WHERE user_id=$1 AND deleted_at IS NULL",
            )
            .bind(user.user_id)
            .fetch_one(&mut *tx)
            .await?;
            if used >= i64::from(QUOTA_DEVICES) {
                return Err(ApiError::quota(
                    QuotaKind::CollectorDevicesPerUser,
                    QUOTA_DEVICES,
                    used as u32,
                ));
            }
            let device = crate::new_id();
            sqlx::query(
                "INSERT INTO devices(id,user_id,name,platform,architecture,collector_kind,collector_version,os_version,enrolled_at,credential_state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(device)
            .bind(user.user_id)
            .bind(&e.device_name)
            .bind(e.platform)
            .bind(e.architecture)
            .bind(e.collector_kind)
            .bind(&e.collector_version)
            .bind(&e.os_version)
            .bind(call.now)
            .bind(e16(DeviceCredentialState::LoggedOut))
            .execute(&mut *tx)
            .await?;
            device
        }
    };
    sqlx::query("UPDATE enrollments SET state=$2,approved_by_user_id=$3,device_id=$4,approved_at=$5 WHERE id=$1")
        .bind(e.id)
        .bind(e16(EnrollmentState::Approved))
        .bind(user.user_id)
        .bind(device)
        .bind(call.now)
        .execute(&mut *tx)
        .await?;
    record(
        &mut tx,
        call,
        Event {
            kind: if reattach.is_some() {
                ActivityKind::DeviceReattached
            } else {
                ActivityKind::DeviceEnrolled
            },
            actor_user: Some(user.user_id),
            subject_user: Some(user.user_id),
            device: Some(device),
            ..Default::default()
        },
    )
    .await?;
    let message = device_message(&mut tx, user.user_id, device, call.now).await?;
    tx.commit().await?;
    Ok(ApproveEnrollmentResponse {
        device: Some(message).into(),
        ..Default::default()
    })
}

pub async fn reject_enrollment(
    s: &Server,
    call: &Call,
    q: RejectEnrollmentRequest,
) -> Result<RejectEnrollmentResponse> {
    let _user = s.active_user(call).await?;
    let id = validate::id("enrollment_id", &q.enrollment_id)?;
    let mut tx = s.db.begin().await?;
    let e = load_enrollment(&mut tx, id, true).await?;
    let state = effective(&e, call.now);
    if state != EnrollmentState::Pending {
        mark_expired(&mut tx, &e, call.now).await?;
        tx.commit().await?;
        return Err(state_error(state));
    }
    sqlx::query("UPDATE enrollments SET state=$2 WHERE id=$1")
        .bind(e.id)
        .bind(e16(EnrollmentState::Rejected))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(RejectEnrollmentResponse::default())
}

/// Issues a new access and refresh token pair and returns the plain tokens.
async fn issue(
    db: &mut PgConnection,
    device: Uuid,
    refresh_expires: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<DeviceCredentials> {
    let access = secret("ada_");
    let refresh = secret("adr_");
    let access_expires = now + Duration::hours(ACCESS_HOURS);
    sqlx::query(
        "INSERT INTO device_credentials(device_id,access_token_sha256,access_expires_at,refresh_token_sha256,refresh_expires_at,issued_at) VALUES($1,$2,$3,$4,$5,$6) \
         ON CONFLICT (device_id) DO UPDATE SET access_token_sha256=EXCLUDED.access_token_sha256,access_expires_at=EXCLUDED.access_expires_at,refresh_token_sha256=EXCLUDED.refresh_token_sha256,refresh_expires_at=EXCLUDED.refresh_expires_at,issued_at=EXCLUDED.issued_at",
    )
    .bind(device)
    .bind(crate::sha256(access.as_bytes()))
    .bind(access_expires)
    .bind(crate::sha256(refresh.as_bytes()))
    .bind(refresh_expires)
    .bind(now)
    .execute(db)
    .await?;
    Ok(DeviceCredentials {
        access_token: access,
        access_token_expires_at: Some(ts(access_expires)).into(),
        refresh_token: refresh,
        refresh_token_expires_at: Some(ts(refresh_expires)).into(),
        ..Default::default()
    })
}

pub async fn poll_enrollment(
    s: &Server,
    call: &Call,
    q: PollEnrollmentRequest,
) -> Result<PollEnrollmentResponse> {
    let id = validate::id("enrollment_id", &q.enrollment_id)?;
    let mut tx = s.db.begin().await?;
    // Only a missing enrollment is an authentication failure. A database error must stay a
    // retryable server error: the collector treats UNAUTHENTICATED as a permanent invalidation.
    let e = load_enrollment(&mut tx, id, true).await.map_err(|e| {
        if e.reason == ErrorReason::NotFound {
            ApiError::unauthenticated()
        } else {
            e
        }
    })?;
    if crate::sha256(q.device_code.as_bytes()) != e.device_code_sha256 {
        return Err(ApiError::unauthenticated());
    }
    if let Some(last) = e.last_polled_at {
        let wait = Duration::seconds(POLL_SECONDS) - (call.now - last);
        if wait > Duration::zero() {
            return Err(ApiError::rate_limited(
                RateLimitScope::Enrollment,
                wait.to_std().unwrap_or_default(),
            ));
        }
    }
    sqlx::query("UPDATE enrollments SET last_polled_at=$2 WHERE id=$1")
        .bind(e.id)
        .bind(call.now)
        .execute(&mut *tx)
        .await?;
    let state = effective(&e, call.now);
    match state {
        EnrollmentState::Pending => {
            tx.commit().await?;
            Ok(PollEnrollmentResponse {
                state: EnrollmentState::Pending.into(),
                ..Default::default()
            })
        }
        EnrollmentState::Approved => {
            let device = e.device_id.ok_or_else(ApiError::internal)?;
            let user = e.approved_by.ok_or_else(ApiError::internal)?;
            let deleted: Option<Option<DateTime<Utc>>> =
                sqlx::query_scalar("SELECT deleted_at FROM devices WHERE id=$1 FOR UPDATE")
                    .bind(device)
                    .fetch_optional(&mut *tx)
                    .await?;
            if !matches!(deleted, Some(None)) {
                sqlx::query("UPDATE enrollments SET state=$2 WHERE id=$1")
                    .bind(e.id)
                    .bind(e16(EnrollmentState::Invalidated))
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                return Err(state_error(EnrollmentState::Invalidated));
            }
            // Rotated tokens of an earlier credential generation cannot revoke the new one.
            sqlx::query("DELETE FROM device_rotated_refresh_tokens WHERE device_id=$1")
                .bind(device)
                .execute(&mut *tx)
                .await?;
            let credentials = issue(
                &mut tx,
                device,
                call.now + Duration::days(time::RETENTION_DAYS),
                call.now,
            )
            .await?;
            sqlx::query("UPDATE devices SET credential_state=$2 WHERE id=$1")
                .bind(device)
                .bind(e16(DeviceCredentialState::Active))
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE enrollments SET state=$2,consumed_at=$3 WHERE id=$1")
                .bind(e.id)
                .bind(e16(EnrollmentState::Consumed))
                .bind(call.now)
                .execute(&mut *tx)
                .await?;
            let message = device_message(&mut tx, user, device, call.now).await?;
            tx.commit().await?;
            tracing::info!(device_id = %device, "device credentials delivered");
            Ok(PollEnrollmentResponse {
                state: EnrollmentState::Consumed.into(),
                device: Some(message).into(),
                credentials: Some(credentials).into(),
                ..Default::default()
            })
        }
        other => {
            mark_expired(&mut tx, &e, call.now).await?;
            tx.commit().await?;
            Err(state_error(other))
        }
    }
}

fn inactive() -> ApiError {
    ApiError::precondition(
        ErrorReason::DeviceCredentialsInactive,
        "device credentials are not active",
    )
}

/// Rejects a refresh-token request before the token is looked up when its client IP address
/// has exhausted the invalid refresh-token limit.
async fn refresh_token_gate(s: &Server, call: &Call) -> Result<()> {
    rate::ensure_open(
        &s.db,
        RateLimitScope::DeviceToken,
        &rate::invalid_refresh_subject(&call.client_ip),
        rate::INVALID_REFRESH_TOKEN_LIMIT,
    )
    .await
}

/// Counts an invalid refresh token against its client IP address, never against the device
/// whose ID the caller sent, and returns `UNAUTHENTICATED` or, over the limit, `RATE_LIMITED`.
async fn invalid_refresh_token(s: &Server, call: &Call) -> ApiError {
    match rate::check(
        &s.db,
        RateLimitScope::DeviceToken,
        &rate::invalid_refresh_subject(&call.client_ip),
        rate::INVALID_REFRESH_TOKEN_LIMIT,
    )
    .await
    {
        Ok(()) => ApiError::unauthenticated(),
        Err(e) => e,
    }
}

/// Charges a request whose refresh token was authenticated to the device token limit.
///
/// The charge runs on the caller's transaction connection: the caller already holds the device
/// row lock, and waiting for a second pooled connection there could exhaust the pool when many
/// refreshes run at once. The count commits with the transaction, so a caller that rejects the
/// request after this charge commits the transaction before it returns the rejection.
async fn charge_device_token(db: &mut PgConnection, device: Uuid) -> Result<()> {
    rate::check(
        db,
        RateLimitScope::DeviceToken,
        &device.to_string(),
        rate::DEVICE_TOKEN_LIMIT,
    )
    .await
}

/// Commits the transaction that charged a rejected request, so the charge persists, and returns
/// the rejection.
async fn reject_charged(tx: Transaction<'_, Postgres>, rejection: ApiError) -> ApiError {
    match tx.commit().await {
        Ok(()) => rejection,
        Err(e) => e.into(),
    }
}

pub async fn refresh_device_token(
    s: &Server,
    call: &Call,
    q: RefreshDeviceTokenRequest,
) -> Result<RefreshDeviceTokenResponse> {
    let device = validate::id("device_id", &q.device_id)?;
    if !valid_token(&q.refresh_token, "adr_") {
        return Err(ApiError::unauthenticated());
    }
    call.note(|a| a.device = Some(device));
    refresh_token_gate(s, call).await?;
    let digest = crate::sha256(q.refresh_token.as_bytes());
    let mut tx = s.db.begin().await?;
    let row: Option<(i16, Option<DateTime<Utc>>, Uuid)> = sqlx::query_as(
        "SELECT credential_state,deleted_at,user_id FROM devices WHERE id=$1 FOR UPDATE",
    )
    .bind(device)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((state, deleted, user)) = row else {
        drop(tx);
        return Err(invalid_refresh_token(s, call).await);
    };
    let current: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT refresh_expires_at FROM device_credentials WHERE device_id=$1 AND refresh_token_sha256=$2",
    )
    .bind(device)
    .bind(&digest)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(expires) = current {
        // The token is authenticated; only now does the request count toward the device limit.
        if let Err(e) = charge_device_token(&mut tx, device).await {
            return Err(reject_charged(tx, e).await);
        }
        if deleted.is_some() || i32::from(state) != DeviceCredentialState::Active.to_i32() {
            return Err(reject_charged(tx, inactive()).await);
        }
        if expires <= call.now {
            sqlx::query("UPDATE devices SET credential_state=$2 WHERE id=$1")
                .bind(device)
                .bind(e16(DeviceCredentialState::Expired))
                .execute(&mut *tx)
                .await?;
            end_device_leases(&mut tx, device).await?;
            tx.commit().await?;
            return Err(inactive());
        }
        sqlx::query("INSERT INTO device_rotated_refresh_tokens(refresh_token_sha256,device_id,rotated_at) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
            .bind(&digest)
            .bind(device)
            .bind(call.now)
            .execute(&mut *tx)
            .await?;
        let credentials = issue(&mut tx, device, expires, call.now).await?;
        tx.commit().await?;
        return Ok(RefreshDeviceTokenResponse {
            credentials: Some(credentials).into(),
            ..Default::default()
        });
    }
    let reused: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM device_rotated_refresh_tokens WHERE refresh_token_sha256=$1 AND device_id=$2)",
    )
    .bind(&digest)
    .bind(device)
    .fetch_one(&mut *tx)
    .await?;
    if !reused {
        drop(tx);
        return Err(invalid_refresh_token(s, call).await);
    }
    // A rotated token was issued for this device, so reuse detection counts toward its limit.
    if let Err(e) = charge_device_token(&mut tx, device).await {
        return Err(reject_charged(tx, e).await);
    }
    if deleted.is_some() || i32::from(state) != DeviceCredentialState::Active.to_i32() {
        return Err(reject_charged(tx, inactive()).await);
    }
    sqlx::query("UPDATE devices SET credential_state=$2 WHERE id=$1")
        .bind(device)
        .bind(e16(DeviceCredentialState::ReuseRevoked))
        .execute(&mut *tx)
        .await?;
    end_device_leases(&mut tx, device).await?;
    record(
        &mut tx,
        call,
        Event {
            kind: ActivityKind::DeviceCredentialsRevoked,
            actor_device: Some(device),
            subject_user: Some(user),
            device: Some(device),
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await?;
    tracing::warn!(device_id = %device, "refresh token reuse detected; device credentials revoked");
    Err(ApiError::new(
        ErrorReason::RefreshTokenReused,
        "refresh token reuse detected; device credentials were revoked",
    ))
}

pub async fn revoke_device_credentials(
    s: &Server,
    call: &Call,
    q: RevokeDeviceCredentialsRequest,
) -> Result<RevokeDeviceCredentialsResponse> {
    let device = validate::id("device_id", &q.device_id)?;
    match call.bearer() {
        Bearer::None if !q.refresh_token.is_empty() => {
            if !valid_token(&q.refresh_token, "adr_") {
                return Err(ApiError::unauthenticated());
            }
            refresh_token_gate(s, call).await?;
            let mut tx = s.db.begin().await?;
            let user: Option<Uuid> = sqlx::query_scalar(
                "SELECT d.user_id FROM device_credentials c JOIN devices d ON d.id=c.device_id WHERE c.device_id=$1 AND c.refresh_token_sha256=$2 FOR UPDATE OF d",
            )
            .bind(device)
            .bind(crate::sha256(q.refresh_token.as_bytes()))
            .fetch_optional(&mut *tx)
            .await?;
            let Some(user) = user else {
                drop(tx);
                return Err(invalid_refresh_token(s, call).await);
            };
            if let Err(e) = charge_device_token(&mut tx, device).await {
                return Err(reject_charged(tx, e).await);
            }
            sqlx::query("DELETE FROM device_credentials WHERE device_id=$1")
                .bind(device)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM device_rotated_refresh_tokens WHERE device_id=$1")
                .bind(device)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE devices SET credential_state=$2 WHERE id=$1")
                .bind(device)
                .bind(e16(DeviceCredentialState::LoggedOut))
                .execute(&mut *tx)
                .await?;
            end_device_leases(&mut tx, device).await?;
            record(
                &mut tx,
                call,
                Event {
                    kind: ActivityKind::DeviceLoggedOut,
                    actor_device: Some(device),
                    subject_user: Some(user),
                    device: Some(device),
                    ..Default::default()
                },
            )
            .await?;
            tx.commit().await?;
            Ok(RevokeDeviceCredentialsResponse::default())
        }
        Bearer::User(_) if q.refresh_token.is_empty() => {
            let user = s.active_user(call).await?;
            let mut tx = s.db.begin().await?;
            let found = owned_device(&mut tx, user.user_id, Some(device)).await?;
            if found.is_none() {
                return Err(ApiError::not_found("device_id"));
            }
            sqlx::query("UPDATE devices SET credential_state=$2 WHERE id=$1")
                .bind(device)
                .bind(e16(DeviceCredentialState::ReuseRevoked))
                .execute(&mut *tx)
                .await?;
            end_device_leases(&mut tx, device).await?;
            record(
                &mut tx,
                call,
                Event {
                    kind: ActivityKind::DeviceCredentialsRevoked,
                    actor_user: Some(user.user_id),
                    subject_user: Some(user.user_id),
                    device: Some(device),
                    ..Default::default()
                },
            )
            .await?;
            tx.commit().await?;
            Ok(RevokeDeviceCredentialsResponse::default())
        }
        _ => Err(ApiError::unauthenticated()),
    }
}

pub async fn list_devices(
    s: &Server,
    call: &Call,
    q: ListDevicesRequest,
) -> Result<ListDevicesResponse> {
    let user = s.active_user(call).await?;
    let page = q.page.as_option().cloned().unwrap_or_default();
    let size = validate::page_size(page.size)?;
    let context = format!("{}|ListDevices", user.user_id);
    let after = decode_cursor(&context, &page.cursor)?;
    let mut db = s.db.acquire().await?;
    let ids: Vec<(Uuid, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id,enrolled_at FROM devices WHERE user_id=$1 AND deleted_at IS NULL \
         AND ($2::timestamptz IS NULL OR (enrolled_at,id)>($2,$3)) ORDER BY enrolled_at,id LIMIT $4",
    )
    .bind(user.user_id)
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
    Ok(ListDevicesResponse {
        devices: devices(&mut db, user.user_id, Some(&page_ids), call.now).await?,
        next_cursor,
        ..Default::default()
    })
}

/// Blocks a device immediately and enqueues its physical deletion.
pub async fn delete_device_in(
    db: &mut PgConnection,
    call: &Call,
    user: Uuid,
    device: Uuid,
) -> Result<DateTime<Utc>> {
    sqlx::query("UPDATE devices SET deleted_at=$2 WHERE id=$1")
        .bind(device)
        .bind(call.now)
        .execute(&mut *db)
        .await?;
    end_device_leases(db, device).await?;
    sqlx::query(
        "UPDATE accounts SET preferred_device_id=NULL WHERE user_id=$1 AND preferred_device_id=$2",
    )
    .bind(user)
    .bind(device)
    .execute(&mut *db)
    .await?;
    detach_source(db, call, SOURCE_DEVICE, device).await?;
    enqueue_deletion(
        db,
        DeletionKind::Device,
        Some(device),
        Some(user),
        None,
        call.now,
    )
    .await
}

pub async fn delete_device(
    s: &Server,
    call: &Call,
    q: DeleteDeviceRequest,
) -> Result<DeleteDeviceResponse> {
    let user = s.active_user(call).await?;
    let device = validate::id("device_id", &q.device_id)?;
    let mut tx = s.db.begin().await?;
    if owned_device(&mut tx, user.user_id, Some(device))
        .await?
        .is_none()
    {
        return Err(ApiError::not_found("device_id"));
    }
    let deadline = delete_device_in(&mut tx, call, user.user_id, device).await?;
    record(
        &mut tx,
        call,
        Event {
            kind: ActivityKind::DeviceDeleted,
            actor_user: Some(user.user_id),
            subject_user: Some(user.user_id),
            device: Some(device),
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await?;
    Ok(DeleteDeviceResponse {
        purge_deadline: Some(ts(deadline)).into(),
        ..Default::default()
    })
}

pub async fn set_preferred_device(
    s: &Server,
    call: &Call,
    q: SetPreferredDeviceRequest,
) -> Result<SetPreferredDeviceResponse> {
    let user = s.active_user(call).await?;
    let device = validate::optional_id("device_id", &q.device_id)?;
    let mut tx = s.db.begin().await?;
    if device.is_some() && owned_device(&mut tx, user.user_id, device).await?.is_none() {
        return Err(ApiError::not_found("device_id"));
    }
    sqlx::query("UPDATE accounts SET preferred_device_id=$2,updated_at=$3 WHERE user_id=$1")
        .bind(user.user_id)
        .bind(device)
        .bind(call.now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(SetPreferredDeviceResponse {
        preferred_device_id: device.map(|d| d.to_string()).unwrap_or_default(),
        ..Default::default()
    })
}

pub async fn request_device_sync(
    s: &Server,
    call: &Call,
    q: RequestDeviceSyncRequest,
) -> Result<RequestDeviceSyncResponse> {
    let user = s.active_user(call).await?;
    let device = validate::id("device_id", &q.device_id)?;
    let mut tx = s.db.begin().await?;
    let last: Option<Option<DateTime<Utc>>> = sqlx::query_scalar(
        "SELECT last_sync_requested_at FROM devices WHERE id=$1 AND user_id=$2 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(device)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(last) = last else {
        return Err(ApiError::not_found("device_id"));
    };
    manual_refresh_allowed(last, call.now)?;
    sqlx::query("UPDATE devices SET sync_requested=true,last_sync_requested_at=$2 WHERE id=$1")
        .bind(device)
        .bind(call.now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(RequestDeviceSyncResponse {
        next_allowed_at: Some(ts(call.now + Duration::seconds(60))).into(),
        ..Default::default()
    })
}

/// One manual refresh per minute per connection or device.
pub fn manual_refresh_allowed(last: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Result<()> {
    if let Some(last) = last {
        let wait = Duration::seconds(60) - (now - last);
        if wait > Duration::zero() {
            return Err(ApiError::rate_limited(
                RateLimitScope::ManualRefresh,
                wait.to_std().unwrap_or_default(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_codes_use_the_fixed_alphabet_and_normalize() {
        let code = user_code();
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");
        assert!(
            code.bytes()
                .filter(|b| *b != b'-')
                .all(|b| USER_CODE_ALPHABET.contains(&b))
        );
        assert_eq!(normalize_user_code(" bcdf - ghjk "), "BCDFGHJK");
        assert_eq!(code_digest("bcdf-ghjk"), code_digest("BCDFGHJK"));
    }
}
