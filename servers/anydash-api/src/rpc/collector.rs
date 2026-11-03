// SPDX-License-Identifier: FSL-1.1-ALv2
//! CollectorService: heartbeat, leases, versioned usage uploads, parser status and snapshots.
use super::*;
use crate::{
    Server,
    audit::{Event, record},
    call::Device as DevicePrincipal,
    cost::{self, Prices},
    time, validate,
};
use buffa_types::google::protobuf::Timestamp;
use chrono::Duration;
use delino_protocol::anydash::v1::__buffa::oneof::metric_value::Value as MetricOneof;
use sqlx::Row;
use std::collections::{HashMap, HashSet};

/// Tool, model and bucket start of a stored bucket.
type StoredKey = (i16, String, DateTime<Utc>);

pub const MAX_BUCKETS: usize = 2000;
pub const MAX_METRICS: usize = 5000;
pub const MAX_RESOURCES: usize = 1000;
const HEARTBEAT_SECONDS: i64 = 60;

fn lease_ttl() -> Duration {
    Duration::seconds(time::LEASE_SECONDS)
}

/// Selects the collection device for a connection without a valid lease: among registered,
/// online, `ACTIVE` devices that report the connection as ready, the user's preferred device
/// first, otherwise the most recent heartbeat, ties to the lower device ID.
pub async fn candidate(
    db: &mut PgConnection,
    connection: Uuid,
    user: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT d.id FROM connection_devices cd JOIN devices d ON d.id=cd.device_id \
         WHERE cd.connection_id=$1 AND cd.ready AND d.deleted_at IS NULL AND d.credential_state=$3 AND d.last_heartbeat_at>$4 \
         ORDER BY COALESCE(d.id=(SELECT preferred_device_id FROM accounts WHERE user_id=$2),false) DESC, d.last_heartbeat_at DESC, d.id ASC LIMIT 1",
    )
    .bind(connection)
    .bind(user)
    .bind(e16(DeviceCredentialState::Active))
    .bind(now - lease_ttl())
    .fetch_optional(db)
    .await?)
}

struct LeaseRow {
    provider: i16,
    epoch: i64,
    holder: Option<Uuid>,
    acquired: Option<DateTime<Utc>>,
    expires: Option<DateTime<Utc>>,
    last_holder: Option<Uuid>,
    refresh_requested: bool,
}
impl LeaseRow {
    fn valid_holder(&self, now: DateTime<Utc>) -> Option<Uuid> {
        self.holder
            .filter(|_| self.expires.is_some_and(|e| e > now))
    }
    fn message(&self, connection: Uuid, now: DateTime<Utc>) -> Option<Lease> {
        self.valid_holder(now).map(|h| Lease {
            connection_id: connection.to_string(),
            holder_device_id: h.to_string(),
            epoch: self.epoch as u64,
            acquired_at: self.acquired.map(ts).into(),
            expires_at: self.expires.map(ts).into(),
            ..Default::default()
        })
    }
    fn current_epoch(&self, now: DateTime<Utc>) -> u64 {
        if self.valid_holder(now).is_some() {
            self.epoch as u64
        } else {
            0
        }
    }
}

/// Locks a connection the device is registered for.
async fn lock_lease(
    db: &mut PgConnection,
    d: &DevicePrincipal,
    connection: Uuid,
) -> Result<LeaseRow> {
    let row = sqlx::query(
        "SELECT c.provider,c.lease_epoch,c.lease_device_id,c.lease_acquired_at,c.lease_expires_at,c.last_holder_device_id,c.refresh_requested \
         FROM connections c WHERE c.id=$1 AND c.user_id=$2 AND c.deleted_at IS NULL \
         AND EXISTS(SELECT 1 FROM connection_devices cd WHERE cd.connection_id=c.id AND cd.device_id=$3) FOR UPDATE OF c",
    )
    .bind(connection)
    .bind(d.user_id)
    .bind(d.device_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("connection_id"))?;
    Ok(LeaseRow {
        provider: row.get("provider"),
        epoch: row.get("lease_epoch"),
        holder: row.get("lease_device_id"),
        acquired: row.get("lease_acquired_at"),
        expires: row.get("lease_expires_at"),
        last_holder: row.get("last_holder_device_id"),
        refresh_requested: row.get("refresh_requested"),
    })
}

fn version_status(s: &Server, call: &Call) -> ClientVersionStatus {
    let kind = call.client_kind.to_i32();
    let minimum = s.config.min_versions.get(&kind);
    let latest = s.config.latest_versions.get(&kind);
    let current = call.client_version.as_ref();
    ClientVersionStatus {
        minimum_version: minimum.map(|v| v.to_string()).unwrap_or_default(),
        latest_version: latest.map(|v| v.to_string()).unwrap_or_default(),
        update_available: matches!((latest, current), (Some(l), Some(c)) if l > c),
        update_required: matches!((minimum, current), (Some(m), Some(c)) if c < m),
        ..Default::default()
    }
}

pub async fn heartbeat(s: &Server, call: &Call, q: HeartbeatRequest) -> Result<HeartbeatResponse> {
    let d = s.device(call).await?;
    validate::version_text("collector_version", &q.collector_version)?;
    validate::version_text("os_version", &q.os_version)?;
    if q.ready_connection_ids.len() > 200 {
        return Err(ApiError::payload_too_large());
    }
    let ready = q
        .ready_connection_ids
        .iter()
        .map(|id| validate::id("ready_connection_ids", id))
        .collect::<Result<Vec<_>>>()?;
    let mut tx = s.db.begin().await?;
    let sync_requested: bool = sqlx::query_scalar(
        "UPDATE devices d SET last_heartbeat_at=$2,collector_version=$3,os_version=$4,sync_requested=false \
         FROM (SELECT sync_requested FROM devices WHERE id=$1 FOR UPDATE) old WHERE d.id=$1 RETURNING old.sync_requested",
    )
    .bind(d.device_id)
    .bind(call.now)
    .bind(&q.collector_version)
    .bind(&q.os_version)
    .fetch_one(&mut *tx)
    .await?;
    // Lock every connection this heartbeat touches (registered or leased by this device) in ID
    // order before changing any of them. Locking the device's own leases first and the other
    // registered connections afterwards lets two devices that hold different leases on the same
    // connections wait for each other.
    let locked: Vec<(Uuid, bool)> = sqlx::query_as(
        "SELECT c.id,EXISTS(SELECT 1 FROM connection_devices cd WHERE cd.connection_id=c.id AND cd.device_id=$1) \
         FROM connections c WHERE c.user_id=$2 AND c.deleted_at IS NULL \
         AND (c.lease_device_id=$1 OR EXISTS(SELECT 1 FROM connection_devices cd WHERE cd.connection_id=c.id AND cd.device_id=$1)) \
         ORDER BY c.id FOR UPDATE OF c",
    )
    .bind(d.device_id)
    .bind(d.user_id)
    .fetch_all(&mut *tx)
    .await?;
    let all: Vec<Uuid> = locked.iter().map(|(id, _)| *id).collect();
    // Registration rows change only after their connection rows are locked, the order that
    // `RegisterConnectionDevice` and `UnregisterConnectionDevice` also follow. IDs of connections the
    // device is not registered for match no row and are ignored.
    sqlx::query("UPDATE connection_devices SET ready=(connection_id=ANY($2)),ready_reported_at=$3 WHERE device_id=$1")
        .bind(d.device_id)
        .bind(&ready)
        .bind(call.now)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE connections SET lease_expires_at=$2 WHERE id=ANY($4) AND lease_device_id=$1 AND lease_expires_at>$3",
    )
    .bind(d.device_id)
    .bind(call.now + lease_ttl())
    .bind(call.now)
    .bind(&all)
    .execute(&mut *tx)
    .await?;
    let ids: Vec<Uuid> = locked
        .into_iter()
        .filter_map(|(id, registered)| registered.then_some(id))
        .collect();
    let mut assignments = Vec::with_capacity(ids.len());
    for id in ids {
        let lease = lock_lease(&mut tx, &d, id).await?;
        let holder = lease.valid_holder(call.now);
        let acquire_recommended = holder.is_none()
            && candidate(&mut tx, id, d.user_id, call.now).await? == Some(d.device_id);
        let refresh_requested = holder == Some(d.device_id) && lease.refresh_requested;
        if refresh_requested {
            sqlx::query("UPDATE connections SET refresh_requested=false WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        assignments.push(ConnectionAssignment {
            connection_id: id.to_string(),
            provider: enum_of::<Provider>(lease.provider).into(),
            lease: lease.message(id, call.now).into(),
            acquire_recommended,
            refresh_requested,
            ..Default::default()
        });
    }
    tx.commit().await?;
    Ok(HeartbeatResponse {
        server_time: Some(ts(call.now)).into(),
        version_status: Some(version_status(s, call)).into(),
        assignments,
        local_sync_requested: sync_requested,
        heartbeat_interval: Some(time::seconds(HEARTBEAT_SECONDS)).into(),
        ..Default::default()
    })
}

pub async fn acquire_lease(
    s: &Server,
    call: &Call,
    q: AcquireLeaseRequest,
) -> Result<AcquireLeaseResponse> {
    let d = s.device(call).await?;
    let id = validate::id("connection_id", &q.connection_id)?;
    let mut tx = s.db.begin().await?;
    let lease = lock_lease(&mut tx, &d, id).await?;
    if let Some(holder) = lease.valid_holder(call.now) {
        tx.commit().await?;
        return Ok(AcquireLeaseResponse {
            granted: holder == d.device_id,
            lease: lease.message(id, call.now).into(),
            ..Default::default()
        });
    }
    if candidate(&mut tx, id, d.user_id, call.now).await? != Some(d.device_id) {
        tx.commit().await?;
        return Ok(AcquireLeaseResponse::default());
    }
    let previous = lease.holder.or(lease.last_holder);
    let epoch = lease.epoch + 1;
    sqlx::query(
        "UPDATE connections SET lease_epoch=$2,lease_device_id=$3,lease_acquired_at=$4,lease_expires_at=$5,last_holder_device_id=$3 WHERE id=$1",
    )
    .bind(id)
    .bind(epoch)
    .bind(d.device_id)
    .bind(call.now)
    .bind(call.now + lease_ttl())
    .execute(&mut *tx)
    .await?;
    if previous.is_some_and(|p| p != d.device_id) {
        record(
            &mut tx,
            call,
            Event {
                kind: ActivityKind::CollectionDeviceChanged,
                actor_device: Some(d.device_id),
                subject_user: Some(d.user_id),
                device: Some(d.device_id),
                connection: Some(id),
                ..Default::default()
            },
        )
        .await?;
    }
    tx.commit().await?;
    tracing::info!(connection_id = %id, device_id = %d.device_id, epoch, "lease granted");
    Ok(AcquireLeaseResponse {
        granted: true,
        lease: Some(Lease {
            connection_id: id.to_string(),
            holder_device_id: d.device_id.to_string(),
            epoch: epoch as u64,
            acquired_at: Some(ts(call.now)).into(),
            expires_at: Some(ts(call.now + lease_ttl())).into(),
            ..Default::default()
        })
        .into(),
        ..Default::default()
    })
}

pub async fn renew_lease(
    s: &Server,
    call: &Call,
    q: RenewLeaseRequest,
) -> Result<RenewLeaseResponse> {
    let d = s.device(call).await?;
    let id = validate::id("connection_id", &q.connection_id)?;
    let mut tx = s.db.begin().await?;
    let lease = lock_lease(&mut tx, &d, id).await?;
    if lease.valid_holder(call.now) != Some(d.device_id) || lease.epoch as u64 != q.epoch {
        return Err(ApiError::lease_expired(lease.current_epoch(call.now)));
    }
    let expires = call.now + lease_ttl();
    sqlx::query("UPDATE connections SET lease_expires_at=$2,refresh_requested=false WHERE id=$1")
        .bind(id)
        .bind(expires)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(RenewLeaseResponse {
        lease: Some(Lease {
            connection_id: id.to_string(),
            holder_device_id: d.device_id.to_string(),
            epoch: lease.epoch as u64,
            acquired_at: lease.acquired.map(ts).into(),
            expires_at: Some(ts(expires)).into(),
            ..Default::default()
        })
        .into(),
        refresh_requested: lease.refresh_requested,
        ..Default::default()
    })
}

pub async fn release_lease(
    s: &Server,
    call: &Call,
    q: ReleaseLeaseRequest,
) -> Result<ReleaseLeaseResponse> {
    let d = s.device(call).await?;
    let id = validate::id("connection_id", &q.connection_id)?;
    let mut tx = s.db.begin().await?;
    let lease = lock_lease(&mut tx, &d, id).await?;
    if lease.holder == Some(d.device_id) && lease.epoch as u64 == q.epoch {
        sqlx::query(
            "UPDATE connections SET last_holder_device_id=lease_device_id,lease_device_id=NULL,lease_expires_at=NULL WHERE id=$1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(ReleaseLeaseResponse::default())
}

/// Records an accepted upload: the refresh token expires 90 days after the latest upload.
async fn mark_upload(db: &mut PgConnection, device: Uuid, now: DateTime<Utc>) -> Result<()> {
    sqlx::query("UPDATE devices SET last_upload_at=$2 WHERE id=$1")
        .bind(device)
        .bind(now)
        .execute(&mut *db)
        .await?;
    sqlx::query("UPDATE device_credentials SET refresh_expires_at=$2 WHERE device_id=$1")
        .bind(device)
        .bind(now + Duration::days(time::RETENTION_DAYS))
        .execute(db)
        .await?;
    Ok(())
}

fn count(field: &str, v: Option<u64>) -> Result<Option<i64>> {
    v.map(|n| i64::try_from(n).map_err(|_| ApiError::invalid(field, "count is too large")))
        .transpose()
}

#[derive(Debug)]
struct Bucket {
    tool: LocalTool,
    model: String,
    start: DateTime<Utc>,
    version: i64,
    sessions: i64,
    tokens: cost::Tokens,
}

/// Reads an optional timestamp: an absent field is `None`, a present but invalid one is
/// INVALID_ARGUMENT for `field` instead of being stored as absent.
fn optional_time(field: &str, t: Option<&Timestamp>) -> Result<Option<DateTime<Utc>>> {
    match t {
        None => Ok(None),
        Some(t) => time::from_timestamp(t)
            .map(Some)
            .ok_or_else(|| ApiError::invalid(field, "invalid timestamp")),
    }
}

fn parse_bucket(i: usize, b: &UsageBucket) -> Result<Bucket> {
    let f = |name: &str| format!("buckets[{i}].{name}");
    let tool = validate::known(&f("tool"), &b.tool)?;
    validate::token(&f("model"), &b.model, 1, 200)?;
    let start = b
        .bucket_start
        .as_option()
        .and_then(time::from_timestamp)
        .filter(|t| time::aligned(*t))
        .ok_or_else(|| ApiError::invalid(&f("bucket_start"), "must be a 15-minute UTC boundary"))?;
    if b.version == 0 {
        return Err(ApiError::invalid(&f("version"), "version must be positive"));
    }
    let version = i64::try_from(b.version)
        .map_err(|_| ApiError::invalid(&f("version"), "version is too large"))?;
    let sessions = i64::try_from(b.session_count)
        .map_err(|_| ApiError::invalid(&f("session_count"), "count is too large"))?;
    let t = b.tokens.as_option().cloned().unwrap_or_default();
    Ok(Bucket {
        tool,
        model: b.model.clone(),
        start,
        version,
        sessions,
        tokens: [
            count(&f("tokens.input_tokens"), t.input_tokens)?,
            count(&f("tokens.output_tokens"), t.output_tokens)?,
            count(&f("tokens.cache_read_tokens"), t.cache_read_tokens)?,
            count(&f("tokens.cache_write_tokens"), t.cache_write_tokens)?,
        ],
    })
}

/// The price version in effect at `at`; the earliest version also covers older usage.
pub fn version_at(versions: &[(Uuid, DateTime<Utc>)], at: DateTime<Utc>) -> Option<Uuid> {
    versions
        .iter()
        .rev()
        .find(|(_, from)| *from <= at)
        .or_else(|| versions.first())
        .map(|(id, _)| *id)
}

async fn model_prices(
    db: &mut PgConnection,
    version: Uuid,
    keys: &[String],
) -> Result<Option<Prices>> {
    let row = sqlx::query(
        "SELECT input_cost_per_token::text AS i,output_cost_per_token::text AS o,cache_read_cost_per_token::text AS r,cache_write_cost_per_token::text AS w \
         FROM model_prices WHERE price_version_id=$1 AND model=ANY($2) ORDER BY array_position($2,model) LIMIT 1",
    )
    .bind(version)
    .bind(keys)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| Prices {
        input: r.get("i"),
        output: r.get("o"),
        cache_read: r.get("r"),
        cache_write: r.get("w"),
    }))
}

pub async fn upload_usage(
    s: &Server,
    call: &Call,
    q: UploadUsageRequest,
) -> Result<UploadUsageResponse> {
    let d = s.device(call).await?;
    if q.buckets.len() > MAX_BUCKETS {
        return Err(ApiError::payload_too_large());
    }
    let buckets = q
        .buckets
        .iter()
        .enumerate()
        .map(|(i, b)| parse_bucket(i, b))
        .collect::<Result<Vec<_>>>()?;
    let mut keys = HashSet::new();
    for (i, b) in buckets.iter().enumerate() {
        if !keys.insert((b.tool.to_i32(), b.model.clone(), b.start)) {
            return Err(ApiError::invalid(
                &format!("buckets[{i}]"),
                "duplicated bucket key",
            ));
        }
    }
    let retention = time::retention_start(call.now);
    let mut tx = s.db.begin().await?;
    let versions: Vec<(Uuid, DateTime<Utc>)> =
        sqlx::query_as("SELECT id,effective_from FROM price_versions ORDER BY effective_from")
            .fetch_all(&mut *tx)
            .await?;
    let existing: HashMap<StoredKey, (i64, Option<Uuid>)> = sqlx::query(
        "SELECT b.tool,b.model,b.bucket_start,b.version,b.price_version_id FROM usage_buckets b \
         JOIN UNNEST($2::smallint[],$3::text[],$4::timestamptz[]) AS k(tool,model,bucket_start) \
         ON b.tool=k.tool AND b.model=k.model AND b.bucket_start=k.bucket_start WHERE b.device_id=$1 FOR UPDATE OF b",
    )
    .bind(d.device_id)
    .bind(buckets.iter().map(|b| e16(b.tool)).collect::<Vec<_>>())
    .bind(buckets.iter().map(|b| b.model.clone()).collect::<Vec<_>>())
    .bind(buckets.iter().map(|b| b.start).collect::<Vec<_>>())
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|r| {
        (
            (r.get("tool"), r.get("model"), r.get("bucket_start")),
            (r.get("version"), r.get("price_version_id")),
        )
    })
    .collect();
    let mut price_cache: HashMap<(Uuid, i32, String), Option<Prices>> = HashMap::new();
    let mut results = Vec::with_capacity(buckets.len());
    for b in &buckets {
        let key = BucketKey {
            tool: b.tool.into(),
            model: b.model.clone(),
            bucket_start: Some(ts(b.start)).into(),
            ..Default::default()
        };
        let stored = existing.get(&(e16(b.tool), b.model.clone(), b.start));
        let (outcome, stored_version) = if b.start < retention {
            (BucketOutcome::OutsideRetention, 0)
        } else if b.start > call.now + Duration::seconds(time::BUCKET_SECONDS) {
            (BucketOutcome::Invalid, stored.map(|s| s.0).unwrap_or(0))
        } else if let Some((v, _)) = stored.filter(|(v, _)| *v == b.version) {
            (BucketOutcome::SameVersionIgnored, *v)
        } else if let Some((v, _)) = stored.filter(|(v, _)| *v > b.version) {
            (BucketOutcome::OlderVersionIgnored, *v)
        } else {
            // A corrected bucket keeps its pinned price version; past values are never repriced.
            let version = stored
                .and_then(|s| s.1)
                .or_else(|| version_at(&versions, b.start));
            let prices = match version {
                Some(v) => {
                    let cache_key = (v, b.tool.to_i32(), b.model.clone());
                    if !price_cache.contains_key(&cache_key) {
                        let found =
                            model_prices(&mut tx, v, &cost::model_keys(b.tool, &b.model)).await?;
                        price_cache.insert(cache_key.clone(), found);
                    }
                    price_cache[&cache_key].clone()
                }
                None => None,
            };
            let computable = cost::computable(b.tool, &b.tokens, prices.as_ref());
            let p = prices.unwrap_or_default();
            sqlx::query(
                "INSERT INTO usage_buckets(device_id,tool,model,bucket_start,user_id,version,session_count,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,price_version_id,estimated_cost_usd,cost_computable,updated_at) \
                 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12, \
                 CASE WHEN $13 THEN COALESCE($8::numeric*$14::numeric,0)+COALESCE($9::numeric*$15::numeric,0)+COALESCE($10::numeric*$16::numeric,0)+COALESCE($11::numeric*$17::numeric,0) END,$13,$18) \
                 ON CONFLICT (device_id,tool,model,bucket_start) DO UPDATE SET version=EXCLUDED.version,session_count=EXCLUDED.session_count,input_tokens=EXCLUDED.input_tokens,\
                 output_tokens=EXCLUDED.output_tokens,cache_read_tokens=EXCLUDED.cache_read_tokens,cache_write_tokens=EXCLUDED.cache_write_tokens,price_version_id=EXCLUDED.price_version_id,\
                 estimated_cost_usd=EXCLUDED.estimated_cost_usd,cost_computable=EXCLUDED.cost_computable,updated_at=EXCLUDED.updated_at WHERE usage_buckets.version<EXCLUDED.version",
            )
            .bind(d.device_id)
            .bind(e16(b.tool))
            .bind(&b.model)
            .bind(b.start)
            .bind(d.user_id)
            .bind(b.version)
            .bind(b.sessions)
            .bind(b.tokens[0])
            .bind(b.tokens[1])
            .bind(b.tokens[2])
            .bind(b.tokens[3])
            .bind(version)
            .bind(computable)
            .bind(p.input)
            .bind(p.output)
            .bind(p.cache_read)
            .bind(p.cache_write)
            .bind(call.now)
            .execute(&mut *tx)
            .await?;
            (BucketOutcome::Applied, b.version)
        };
        results.push(BucketResult {
            key: Some(key).into(),
            outcome: outcome.into(),
            stored_version: stored_version as u64,
            ..Default::default()
        });
    }
    mark_upload(&mut tx, d.device_id, call.now).await?;
    tx.commit().await?;
    Ok(UploadUsageResponse {
        results,
        retention_start: Some(ts(retention)).into(),
        ..Default::default()
    })
}

pub async fn report_parser_status(
    s: &Server,
    call: &Call,
    q: ReportParserStatusRequest,
) -> Result<ReportParserStatusResponse> {
    let d = s.device(call).await?;
    if q.statuses.len() > 16 {
        return Err(ApiError::payload_too_large());
    }
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for (i, st) in q.statuses.iter().enumerate() {
        let f = |name: &str| format!("statuses[{i}].{name}");
        let tool = validate::known(&f("tool"), &st.tool)?;
        let state = validate::known(&f("state"), &st.state)?;
        if !seen.insert(tool.to_i32()) {
            return Err(ApiError::invalid(&f("tool"), "duplicated tool"));
        }
        validate::version_text(&f("parser_version"), &st.parser_version)?;
        let skipped = i64::try_from(st.skipped_record_count)
            .map_err(|_| ApiError::invalid(&f("skipped_record_count"), "too large"))?;
        rows.push((
            tool,
            state,
            skipped,
            optional_time(&f("observed_at"), st.observed_at.as_option())?,
            st.parser_version.clone(),
            st.source_path_count as i32,
        ));
    }
    let mut tx = s.db.begin().await?;
    for (tool, state, skipped, observed, version, paths) in rows {
        sqlx::query(
            "INSERT INTO device_parser_status(device_id,tool,state,skipped_record_count,observed_at,parser_version,source_path_count,reported_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8) \
             ON CONFLICT (device_id,tool) DO UPDATE SET state=EXCLUDED.state,skipped_record_count=EXCLUDED.skipped_record_count,observed_at=EXCLUDED.observed_at,\
             parser_version=EXCLUDED.parser_version,source_path_count=EXCLUDED.source_path_count,reported_at=EXCLUDED.reported_at",
        )
        .bind(d.device_id)
        .bind(e16(tool))
        .bind(e16(state))
        .bind(skipped)
        .bind(observed)
        .bind(version)
        .bind(paths)
        .bind(call.now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(ReportParserStatusResponse::default())
}

/// A validated metric observation ready for storage.
struct Metric {
    resource_id: String,
    kind: MetricKind,
    dimension: String,
    state: MetricState,
    value: serde_json::Value,
    unit: String,
    temporality: i16,
    period_start: Option<DateTime<Utc>>,
    period_end: Option<DateTime<Utc>>,
}

fn check_value(field: &str, v: &MetricValue, state: MetricState) -> Result<()> {
    let carries = matches!(state, MetricState::Value | MetricState::Partial);
    match (&v.value, carries) {
        (None, true) => {
            return Err(ApiError::invalid(
                field,
                "a value is required for VALUE and PARTIAL",
            ));
        }
        (Some(_), false) => {
            return Err(ApiError::invalid(
                field,
                "a value is allowed only for VALUE and PARTIAL",
            ));
        }
        _ => {}
    }
    match &v.value {
        Some(MetricOneof::Percent(p)) if !p.is_finite() => {
            Err(ApiError::invalid(field, "invalid percent"))
        }
        Some(MetricOneof::Decimal(d)) => validate::decimal(field, d),
        Some(MetricOneof::Money(m)) => {
            validate::decimal(field, &m.amount)?;
            if !(m.currency_code.is_empty()
                || (m.currency_code.len() == 3
                    && m.currency_code.bytes().all(|b| b.is_ascii_uppercase())))
            {
                return Err(ApiError::invalid(field, "invalid currency code"));
            }
            Ok(())
        }
        Some(MetricOneof::Text(t))
            if t.chars().count() > 200 || t.chars().any(char::is_control) =>
        {
            Err(ApiError::invalid(field, "invalid text"))
        }
        Some(MetricOneof::Time(t)) if time::from_timestamp(t).is_none() => {
            Err(ApiError::invalid(field, "invalid time"))
        }
        _ => Ok(()),
    }
}

fn parse_metric(field: &str, resource_id: &str, m: &MetricObservation) -> Result<Metric> {
    let kind = validate::known(&format!("{field}.kind"), &m.kind)?;
    if kind == MetricKind::SubscriptionRemainingPercent {
        return Err(ApiError::invalid(
            &format!("{field}.kind"),
            "collectors never report the remaining percentage",
        ));
    }
    validate::token(&format!("{field}.dimension"), &m.dimension, 0, 200)?;
    let value = m.value.as_option().cloned().unwrap_or_default();
    let state = validate::known(&format!("{field}.value.state"), &value.state)?;
    check_value(&format!("{field}.value"), &value, state)?;
    if m.unit.len() > 100 || m.unit.chars().any(char::is_control) {
        return Err(ApiError::invalid(&format!("{field}.unit"), "invalid unit"));
    }
    let temporality = validate::optional_enum(&format!("{field}.temporality"), &m.temporality)?;
    let period_start = optional_time(&format!("{field}.period_start"), m.period_start.as_option())?;
    let period_end = optional_time(&format!("{field}.period_end"), m.period_end.as_option())?;
    if temporality == Some(MetricTemporality::Cumulative)
        && state == MetricState::Value
        && (period_start.is_none() || period_end.is_none() || period_start >= period_end)
    {
        return Err(ApiError::invalid(
            &format!("{field}.period_start"),
            "cumulative values need a period",
        ));
    }
    Ok(Metric {
        resource_id: resource_id.into(),
        kind,
        dimension: m.dimension.clone(),
        state,
        value: serde_json::to_value(&value)?,
        unit: m.unit.clone(),
        temporality: temporality.map(e16).unwrap_or(0),
        period_start,
        period_end,
    })
}

struct Resource {
    id: String,
    kind: ResourceKind,
    display_name: String,
    parent_id: String,
    health: i16,
    provider_status: String,
    status_state: i16,
}

fn parse_resource(field: &str, r: &ResourceObservation) -> Result<Resource> {
    validate::token(&format!("{field}.resource_id"), &r.resource_id, 1, 200)?;
    let kind = validate::known(&format!("{field}.kind"), &r.kind)?;
    let display_name = validate::text(&format!("{field}.display_name"), &r.display_name, 0, 200)?;
    if r.parent_id.len() > 200 || r.provider_status.len() > 100 {
        return Err(ApiError::invalid(field, "invalid resource fields"));
    }
    let health = validate::optional_enum(&format!("{field}.health"), &r.health)?;
    let status_state = validate::optional_enum(&format!("{field}.status_state"), &r.status_state)?;
    Ok(Resource {
        id: r.resource_id.clone(),
        kind,
        display_name,
        parent_id: r.parent_id.clone(),
        health: health.map(e16).unwrap_or(0),
        provider_status: r.provider_status.clone(),
        status_state: status_state.map(e16).unwrap_or(0),
    })
}

pub async fn report_snapshot(
    s: &Server,
    call: &Call,
    q: ReportSnapshotRequest,
) -> Result<ReportSnapshotResponse> {
    let d = s.device(call).await?;
    let id = validate::id("connection_id", &q.connection_id)?;
    let snap = q
        .snapshot
        .as_option()
        .ok_or_else(|| ApiError::invalid("snapshot", "snapshot is required"))?;
    let observed = snap
        .observed_at
        .as_option()
        .and_then(time::from_timestamp)
        .ok_or_else(|| ApiError::invalid("snapshot.observed_at", "observed_at is required"))?;
    let status = validate::known("snapshot.status", &snap.status)?;
    let failure = validate::optional_enum("snapshot.failure_class", &snap.failure_class)?;
    let _source = validate::optional_enum("snapshot.source", &snap.source)?;
    let total_metrics = snap.metrics.len()
        + snap
            .resources
            .iter()
            .map(|r| r.metrics.len())
            .sum::<usize>();
    if total_metrics > MAX_METRICS || snap.resources.len() > MAX_RESOURCES {
        return Err(ApiError::payload_too_large());
    }
    let mut metrics = Vec::with_capacity(total_metrics);
    for (i, m) in snap.metrics.iter().enumerate() {
        metrics.push(parse_metric(&format!("snapshot.metrics[{i}]"), "", m)?);
    }
    let mut resources = Vec::with_capacity(snap.resources.len());
    let mut resource_ids = HashSet::new();
    for (i, r) in snap.resources.iter().enumerate() {
        let field = format!("snapshot.resources[{i}]");
        let resource = parse_resource(&field, r)?;
        if !resource_ids.insert(resource.id.clone()) {
            return Err(ApiError::invalid(&field, "duplicated resource"));
        }
        for (j, m) in r.metrics.iter().enumerate() {
            metrics.push(parse_metric(
                &format!("{field}.metrics[{j}]"),
                &resource.id,
                m,
            )?);
        }
        resources.push(resource);
    }
    let mut metric_keys = HashSet::new();
    for m in &metrics {
        if !metric_keys.insert((m.resource_id.clone(), m.kind.to_i32(), m.dimension.clone())) {
            return Err(ApiError::invalid(
                "snapshot.metrics",
                "duplicated metric key",
            ));
        }
    }
    let mut tx = s.db.begin().await?;
    // `mark_upload` changes the device row after the snapshot is applied. Lock the device row
    // before the connection row, as `Heartbeat` does, so a concurrent heartbeat of the same
    // device cannot wait for this connection while this call waits for the device.
    lock_device(&mut tx, d.device_id).await?;
    let lease = lock_lease(&mut tx, &d, id).await?;
    if lease.valid_holder(call.now) != Some(d.device_id) || lease.epoch as u64 != q.lease_epoch {
        return Err(ApiError::lease_expired(lease.current_epoch(call.now)));
    }
    if lease.acquired.is_some_and(|a| observed < a) || observed > call.now + Duration::seconds(60) {
        return Err(ApiError::invalid(
            "snapshot.observed_at",
            "observed_at is outside the lease",
        ));
    }
    let latest: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT latest_observed_at FROM connections WHERE id=$1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    let outcome = match latest {
        Some(l) if l == observed => SnapshotOutcome::Duplicate,
        Some(l) if l > observed => SnapshotOutcome::HistoryOnly,
        _ => SnapshotOutcome::LatestUpdated,
    };
    let provider: Provider = enum_of(lease.provider);
    if outcome != SnapshotOutcome::Duplicate {
        apply_snapshot(
            &mut tx, call, id, observed, status, failure, &metrics, &resources, outcome,
        )
        .await?;
    }
    mark_upload(&mut tx, d.device_id, call.now).await?;
    tx.commit().await?;
    s.metrics
        .snapshots
        .with_label_values(&[
            &crate::worker::label(provider, "PROVIDER_"),
            &crate::worker::label(status, "COLLECTION_STATUS_"),
        ])
        .inc();
    Ok(ReportSnapshotResponse {
        outcome: outcome.into(),
        ..Default::default()
    })
}

#[allow(clippy::too_many_arguments)]
async fn apply_snapshot(
    tx: &mut PgConnection,
    call: &Call,
    id: Uuid,
    observed: DateTime<Utc>,
    status: CollectionStatus,
    failure: Option<FailureClass>,
    metrics: &[Metric],
    resources: &[Resource],
    outcome: SnapshotOutcome,
) -> Result<()> {
    let failure_code = failure.map(e16).unwrap_or(0);
    let success = matches!(
        status,
        CollectionStatus::Succeeded | CollectionStatus::PartiallySucceeded
    );
    if outcome == SnapshotOutcome::LatestUpdated {
        sqlx::query(
            "UPDATE connections SET latest_observed_at=$2,last_status=$3,last_failure_class=$4,\
             last_success_at=CASE WHEN $5 THEN $2 ELSE last_success_at END,\
             last_error_correlation_id=CASE WHEN $6 THEN $7 ELSE last_error_correlation_id END WHERE id=$1",
        )
        .bind(id)
        .bind(observed)
        .bind(e16(status))
        .bind(if success { 0 } else { failure_code })
        .bind(success)
        .bind(matches!(status, CollectionStatus::Failed | CollectionStatus::RateLimited | CollectionStatus::AuthExpired))
        .bind(&call.correlation)
        .execute(&mut *tx)
        .await?;
        let failed_state = match status {
            CollectionStatus::AuthExpired => Some(MetricState::AuthExpired),
            CollectionStatus::Failed | CollectionStatus::RateLimited => Some(MetricState::Failed),
            _ => None,
        };
        if let Some(state) = failed_state.filter(|_| metrics.is_empty()) {
            // A failed cycle without metrics replaces the latest state but keeps the last value.
            let class = if status == CollectionStatus::RateLimited {
                e16(FailureClass::RateLimited)
            } else {
                failure_code
            };
            sqlx::query(
                "UPDATE metric_latest SET state=$2,value=NULL,failure_class=$3,observed_at=$4 WHERE connection_id=$1 AND observed_at<$4",
            )
            .bind(id)
            .bind(e16(state))
            .bind(class)
            .bind(observed)
            .execute(&mut *tx)
            .await?;
        }
        if status == CollectionStatus::Succeeded {
            let ids: Vec<String> = resources.iter().map(|r| r.id.clone()).collect();
            sqlx::query(
                "DELETE FROM resources_latest WHERE connection_id=$1 AND NOT (resource_id=ANY($2))",
            )
            .bind(id)
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM metric_latest WHERE connection_id=$1 AND resource_id<>'' AND NOT (resource_id=ANY($2))")
                .bind(id)
                .bind(&ids)
                .execute(&mut *tx)
                .await?;
        }
        for r in resources {
            sqlx::query(
                "INSERT INTO resources_latest(connection_id,resource_id,kind,display_name,parent_id,health,provider_status,status_state,observed_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) \
                 ON CONFLICT (connection_id,resource_id) DO UPDATE SET kind=EXCLUDED.kind,display_name=EXCLUDED.display_name,parent_id=EXCLUDED.parent_id,health=EXCLUDED.health,\
                 provider_status=EXCLUDED.provider_status,status_state=EXCLUDED.status_state,observed_at=EXCLUDED.observed_at WHERE resources_latest.observed_at<EXCLUDED.observed_at",
            )
            .bind(id)
            .bind(&r.id)
            .bind(e16(r.kind))
            .bind(&r.display_name)
            .bind(&r.parent_id)
            .bind(r.health)
            .bind(&r.provider_status)
            .bind(r.status_state)
            .bind(observed)
            .execute(&mut *tx)
            .await?;
        }
    }
    for m in metrics {
        let carries = matches!(m.state, MetricState::Value | MetricState::Partial);
        // A late (HISTORY_ONLY) snapshot updates hourly history only. Writing latest rows would
        // recreate resource metrics that a newer successful snapshot removed, because the
        // observation-time guard cannot apply to a row that no longer exists.
        if outcome == SnapshotOutcome::LatestUpdated {
            sqlx::query(
                "INSERT INTO metric_latest(connection_id,resource_id,kind,dimension,state,value,unit,temporality,period_start,period_end,observed_at,failure_class,last_success_value,last_success_at) \
                 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,CASE WHEN $13 THEN $6 END,CASE WHEN $13 THEN $11 END) \
                 ON CONFLICT (connection_id,resource_id,kind,dimension) DO UPDATE SET state=EXCLUDED.state,value=EXCLUDED.value,unit=EXCLUDED.unit,temporality=EXCLUDED.temporality,\
                 period_start=EXCLUDED.period_start,period_end=EXCLUDED.period_end,observed_at=EXCLUDED.observed_at,failure_class=EXCLUDED.failure_class,\
                 last_success_value=CASE WHEN $13 THEN EXCLUDED.value ELSE metric_latest.last_success_value END,\
                 last_success_at=CASE WHEN $13 THEN EXCLUDED.observed_at ELSE metric_latest.last_success_at END \
                 WHERE metric_latest.observed_at<EXCLUDED.observed_at",
            )
            .bind(id)
            .bind(&m.resource_id)
            .bind(e16(m.kind))
            .bind(&m.dimension)
            .bind(e16(m.state))
            .bind(if carries { Some(&m.value) } else { None })
            .bind(&m.unit)
            .bind(m.temporality)
            .bind(m.period_start)
            .bind(m.period_end)
            .bind(observed)
            .bind(if carries { 0 } else { failure_code })
            .bind(carries)
            .execute(&mut *tx)
            .await?;
        }
        if m.state == MetricState::Value {
            // The last successful value in each UTC hour; a late result lands in its own hour.
            sqlx::query(
                "INSERT INTO metric_hourly(connection_id,resource_id,kind,dimension,hour_start,value,unit,temporality,period_start,period_end,observed_at) \
                 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT (connection_id,resource_id,kind,dimension,hour_start) DO UPDATE SET \
                 value=EXCLUDED.value,unit=EXCLUDED.unit,temporality=EXCLUDED.temporality,period_start=EXCLUDED.period_start,period_end=EXCLUDED.period_end,observed_at=EXCLUDED.observed_at \
                 WHERE metric_hourly.observed_at<EXCLUDED.observed_at",
            )
            .bind(id)
            .bind(&m.resource_id)
            .bind(e16(m.kind))
            .bind(&m.dimension)
            .bind(time::floor_hour(observed))
            .bind(&m.value)
            .bind(&m.unit)
            .bind(m.temporality)
            .bind(m.period_start)
            .bind(m.period_end)
            .bind(observed)
            .execute(&mut *tx)
            .await?;
        }
    }
    Ok(())
}

/// Test timestamp helper.
#[cfg(test)]
fn t(seconds: i64) -> buffa_types::google::protobuf::Timestamp {
    buffa_types::google::protobuf::Timestamp {
        seconds,
        nanos: 0,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    #[test]
    fn price_version_selection_uses_the_version_in_effect() {
        let v1 = crate::new_id();
        let v2 = crate::new_id();
        let at = |d| Utc.with_ymd_and_hms(2026, 5, d, 3, 0, 0).unwrap();
        let versions = vec![(v1, at(2)), (v2, at(5))];
        assert_eq!(
            version_at(&versions, at(1)),
            Some(v1),
            "earliest covers older usage"
        );
        assert_eq!(version_at(&versions, at(2)), Some(v1));
        assert_eq!(version_at(&versions, at(4)), Some(v1));
        assert_eq!(version_at(&versions, at(5)), Some(v2));
        assert_eq!(version_at(&[], at(5)), None);
    }
    #[test]
    fn buckets_reject_misaligned_starts_and_zero_versions() {
        let good = UsageBucket {
            tool: LocalTool::Codex.into(),
            model: "gpt-5".into(),
            bucket_start: Some(t(1_800_000_000)).into(),
            version: 1,
            session_count: 1,
            ..Default::default()
        };
        assert!(parse_bucket(0, &good).is_ok());
        let mut bad = good.clone();
        bad.bucket_start = Some(t(1_800_000_060)).into();
        assert_eq!(
            parse_bucket(0, &bad).unwrap_err().detail.field,
            "buckets[0].bucket_start"
        );
        let mut bad = good.clone();
        bad.version = 0;
        assert!(parse_bucket(0, &bad).is_err());
        let mut bad = good;
        bad.tool = LocalTool::Unspecified.into();
        assert!(parse_bucket(0, &bad).is_err());
    }
}
