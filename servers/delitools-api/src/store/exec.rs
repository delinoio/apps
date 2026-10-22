// SPDX-License-Identifier: FSL-1.1-ALv2
//! Storage for the execution pipeline: catalog price versions, requests, idempotency records and
//! attempts. Nothing here stores input text, schemas, results or credentials.
use super::retention::cutoff;
use crate::{
    error::Result,
    gateway::{CatalogPrice, Tokens, Usage},
    platform::ExecutionPrincipal,
    pricing::{self, PriceSnapshot},
};
use chrono::{DateTime, Duration, Utc};
use delino_protocol::delitools::v1::{Capability, ErrorReason};
use serde_json::Value;
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

/// Database value of `requests.capability`, `attempts.capability` and
/// `idempotency_records.method`.
pub fn capability_db(capability: Capability) -> i16 {
    match capability {
        Capability::CAPABILITY_EXTRACTOR => 2,
        _ => 1,
    }
}
pub fn capability_from_db(value: i16) -> Capability {
    if value == 2 {
        Capability::CAPABILITY_EXTRACTOR
    } else {
        Capability::CAPABILITY_TRANSLATOR
    }
}

/// `requests.state` and `idempotency_records.state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Processing,
    Completed,
    Unknown,
}
impl State {
    pub fn to_db(self) -> i16 {
        match self {
            Self::Processing => 1,
            Self::Completed => 2,
            Self::Unknown => 3,
        }
    }
    pub fn from_db(value: i16) -> Self {
        match value {
            2 => Self::Completed,
            3 => Self::Unknown,
            _ => Self::Processing,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Catalog prices
// ---------------------------------------------------------------------------------------------

fn snapshot_from_row(row: &PgRow) -> std::result::Result<PriceSnapshot, sqlx::Error> {
    let optional = |name: &str| -> std::result::Result<Option<String>, sqlx::Error> {
        row.try_get::<Option<String>, _>(name)
    };
    Ok(PriceSnapshot {
        id: row.try_get("id")?,
        model: row.try_get("model")?,
        route: row.try_get("provider_route")?,
        effective_at: row.try_get("effective_at")?,
        input: row.try_get("input")?,
        output: row.try_get("output")?,
        cache_read: optional("cache_read")?,
        cache_write: optional("cache_write")?,
    })
}

const PRICE_COLUMNS: &str = "id,model,provider_route,effective_at,input_usd_per_token::text AS input,output_usd_per_token::text AS output,cache_read_usd_per_token::text AS cache_read,cache_write_usd_per_token::text AS cache_write";

/// Serializes the writers of one model's price versions against the requests that freeze one.
/// Price synchronization takes it exclusively and a request start takes it shared, so a new
/// version is either committed before a request selects its price or becomes effective after the
/// request's start time.
const PRICE_LOCK: &str = "pg_advisory_xact_lock(hashtext($1))";
const PRICE_LOCK_SHARED: &str = "pg_advisory_xact_lock_shared(hashtext($1))";

/// The newest price version of a model effective at `now`, read inside the transaction of the
/// request start (after the shared price lock), so the snapshot and the persisted start time agree.
async fn latest_price_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    model: &str,
    now: DateTime<Utc>,
) -> Result<Option<PriceSnapshot>> {
    sqlx::query(&format!("SELECT {PRICE_LOCK_SHARED}"))
        .bind(model)
        .execute(&mut **tx)
        .await?;
    // A newest version that withdrew the price means the model is unpriced now, however old the
    // previous priced version is.
    let row = sqlx::query(&format!(
        "SELECT id,model,provider_route,effective_at,input,output,cache_read,cache_write FROM (SELECT {PRICE_COLUMNS},priced FROM catalog_prices WHERE model=$1 AND effective_at<=$2 ORDER BY effective_at DESC, synced_at DESC LIMIT 1) latest WHERE priced"
    ))
    .bind(model)
    .bind(now)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.as_ref().map(snapshot_from_row).transpose()?)
}

/// SQL predicate: at least one model whose newest price version is a real price. Readiness and
/// the price availability gauge use it, so a catalog where every model was withdrawn is not ready.
pub const PRICE_AVAILABLE_SQL: &str = "EXISTS(SELECT 1 FROM (SELECT DISTINCT ON (model) priced FROM catalog_prices ORDER BY model, effective_at DESC, synced_at DESC) latest WHERE priced)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceWrite {
    /// A new version became effective now.
    NewVersion,
    /// The latest version is unchanged; only its synchronization time moved.
    Refreshed,
    /// The model's price was withdrawn: it is unpriced from now on.
    Withdrawn,
}

fn same_price(snapshot: &PriceSnapshot, route: &str, price: &CatalogPrice) -> bool {
    snapshot.route == route
        && snapshot.input == price.input
        && snapshot.output == price.output
        && snapshot.cache_read == price.cache_read
        && snapshot.cache_write == price.cache_write
}

/// Record the synchronized public price of one model route. A changed price becomes a new version
/// effective when it is written; requests that already started keep the snapshot they froze.
pub async fn record_price(
    pool: &PgPool,
    model: &str,
    route: &str,
    price: &CatalogPrice,
    revision: &str,
) -> Result<PriceWrite> {
    let mut tx = pool.begin().await?;
    // Serialize concurrent writers of one model, and requests that freeze its price, without a
    // table lock.
    sqlx::query(&format!("SELECT {PRICE_LOCK}"))
        .bind(model)
        .execute(&mut *tx)
        .await?;
    // Read the clock only after the lock is held. A request that started earlier was committed
    // first (or is blocked behind this transaction with an earlier start time), so the effective
    // time of a new version never precedes the start time of a request that did not see it.
    let now = super::now();
    let latest = sqlx::query(&format!(
        "SELECT {PRICE_COLUMNS},priced FROM catalog_prices WHERE model=$1 ORDER BY effective_at DESC, synced_at DESC LIMIT 1"
    ))
    .bind(model)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(row) = &latest {
        let snapshot = snapshot_from_row(row)?;
        // A withdrawn model is never "unchanged": its price returns as a new version.
        if row.try_get::<bool, _>("priced")? && same_price(&snapshot, route, price) {
            sqlx::query("UPDATE catalog_prices SET synced_at=$2 WHERE id=$1")
                .bind(snapshot.id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(PriceWrite::Refreshed);
        }
    }
    sqlx::query("INSERT INTO catalog_prices(id,model,provider_route,effective_at,input_usd_per_token,output_usd_per_token,cache_read_usd_per_token,cache_write_usd_per_token,catalog_revision,synced_at,created_at) VALUES($1,$2,$3,$4,$5::numeric,$6::numeric,$7::numeric,$8::numeric,$9,$4,$4)")
        .bind(Uuid::now_v7())
        .bind(model)
        .bind(route)
        .bind(now)
        .bind(&price.input)
        .bind(&price.output)
        .bind(&price.cache_read)
        .bind(&price.cache_write)
        .bind(revision)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(PriceWrite::NewVersion)
}

/// Record that the public catalog was read successfully but no active provider endpoint charges
/// the published flat price of `model`, so the model has no applicable route or price. A previous
/// price is withdrawn by appending an unpriced version effective now (requests that already
/// started keep the snapshot they froze); a model that was never priced, or is already withdrawn,
/// only has its synchronization time refreshed.
pub async fn record_unpriced(pool: &PgPool, model: &str, revision: &str) -> Result<PriceWrite> {
    let mut tx = pool.begin().await?;
    sqlx::query(&format!("SELECT {PRICE_LOCK}"))
        .bind(model)
        .execute(&mut *tx)
        .await?;
    let now = super::now();
    let latest = sqlx::query(
        "SELECT id,priced FROM catalog_prices WHERE model=$1 ORDER BY effective_at DESC, synced_at DESC LIMIT 1",
    )
    .bind(model)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(latest) = latest else {
        tx.rollback().await?;
        return Ok(PriceWrite::Refreshed);
    };
    if !latest.try_get::<bool, _>("priced")? {
        sqlx::query("UPDATE catalog_prices SET synced_at=$2 WHERE id=$1")
            .bind(latest.try_get::<Uuid, _>("id")?)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(PriceWrite::Refreshed);
    }
    sqlx::query("INSERT INTO catalog_prices(id,model,provider_route,effective_at,input_usd_per_token,output_usd_per_token,catalog_revision,synced_at,created_at,priced) VALUES($1,$2,'',$3,0,0,$4,$3,$3,false)")
        .bind(Uuid::now_v7())
        .bind(model)
        .bind(now)
        .bind(revision)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(PriceWrite::Withdrawn)
}

// ---------------------------------------------------------------------------------------------
// Requests and idempotency
// ---------------------------------------------------------------------------------------------

pub struct NewRequest<'a> {
    pub id: Uuid,
    pub principal: &'a ExecutionPrincipal,
    pub model: &'a str,
}

pub struct IdempotencyClaim<'a> {
    pub key: &'a str,
    pub input_hash: [u8; 32],
}

/// Existing deduplication state, returned instead of executing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Duplicate {
    pub request_id: Uuid,
    pub state: State,
    pub input_hash: Vec<u8>,
}

/// The unexpired deduplication record for a key, if any. A duplicate call answers from this
/// without consuming tenant admission or contacting the Gateway; a concurrent first call that
/// races past this check is still decided atomically by [`start_request`].
pub async fn peek_duplicate(
    pool: &PgPool,
    tenant_id: Uuid,
    capability: Capability,
    key: &str,
    now: DateTime<Utc>,
) -> Result<Option<Duplicate>> {
    let row = sqlx::query("SELECT request_id,input_hash,state FROM idempotency_records WHERE tenant_id=$1 AND method=$2 AND idempotency_key=$3 AND created_at>$4")
        .bind(tenant_id)
        .bind(capability_db(capability))
        .bind(key)
        .bind(cutoff(now))
        .fetch_optional(pool)
        .await?;
    row.map(|row| -> std::result::Result<Duplicate, sqlx::Error> {
        Ok(Duplicate {
            request_id: row.try_get("request_id")?,
            state: State::from_db(row.try_get("state")?),
            input_hash: row.try_get("input_hash")?,
        })
    })
    .transpose()
    .map_err(Into::into)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Start {
    /// The request was created with this price version frozen.
    Started(PriceSnapshot),
    /// The model has no price effective at the start time; nothing was created or claimed.
    NoPrice,
    Duplicate(Duplicate),
}

/// Create the request record, claiming the idempotency key in the same transaction. The frozen
/// price is the newest version effective at `now`, selected in this transaction and persisted with
/// the same `now` as the request's start time. A concurrent or earlier request with the same
/// tenant, method and key wins; a record older than the 30-day boundary is expired and replaced,
/// so the key starts a fresh execution.
pub async fn start_request(
    pool: &PgPool,
    request: &NewRequest<'_>,
    claim: Option<&IdempotencyClaim<'_>>,
    now: DateTime<Utc>,
) -> Result<Start> {
    let principal = request.principal;
    let method = capability_db(principal.capability);
    let mut tx = pool.begin().await?;
    let Some(price) = latest_price_in(&mut tx, request.model, now).await? else {
        tx.rollback().await?;
        return Ok(Start::NoPrice);
    };
    if let Some(claim) = claim {
        let mut attempts = 0;
        loop {
            let inserted = sqlx::query("INSERT INTO idempotency_records(tenant_id,method,idempotency_key,request_id,input_hash,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,1,$6,$6) ON CONFLICT DO NOTHING")
                .bind(principal.tenant_id)
                .bind(method)
                .bind(claim.key)
                .bind(request.id)
                .bind(claim.input_hash.as_slice())
                .bind(now)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            if inserted == 1 {
                break;
            }
            let existing = sqlx::query("SELECT request_id,input_hash,state,created_at FROM idempotency_records WHERE tenant_id=$1 AND method=$2 AND idempotency_key=$3 FOR UPDATE")
                .bind(principal.tenant_id)
                .bind(method)
                .bind(claim.key)
                .fetch_optional(&mut *tx)
                .await?;
            let Some(existing) = existing else {
                // Deleted by retention between the two statements; claim again.
                attempts += 1;
                if attempts > 3 {
                    return Err(crate::error::Fault::internal());
                }
                continue;
            };
            let created: DateTime<Utc> = existing.try_get("created_at")?;
            if created <= cutoff(now) {
                // Expired: deduplication is guaranteed only inside the retention window.
                sqlx::query("DELETE FROM idempotency_records WHERE tenant_id=$1 AND method=$2 AND idempotency_key=$3")
                    .bind(principal.tenant_id)
                    .bind(method)
                    .bind(claim.key)
                    .execute(&mut *tx)
                    .await?;
                attempts += 1;
                if attempts > 3 {
                    return Err(crate::error::Fault::internal());
                }
                continue;
            }
            let duplicate = Duplicate {
                request_id: existing.try_get("request_id")?,
                state: State::from_db(existing.try_get("state")?),
                input_hash: existing.try_get("input_hash")?,
            };
            tx.rollback().await?;
            return Ok(Start::Duplicate(duplicate));
        }
    }
    sqlx::query("INSERT INTO requests(id,tenant_id,capability,state,model,credential_kind,api_key_id,user_id,price_id,price_effective_at,created_at,updated_at) VALUES($1,$2,$3,1,$4,$5,$6,$7,$8,$9,$10,$10)")
        .bind(request.id)
        .bind(principal.tenant_id)
        .bind(method)
        .bind(request.model)
        .bind(principal.credential.to_db())
        .bind(principal.api_key_id)
        .bind(principal.user_id)
        .bind(price.id)
        .bind(price.effective_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Start::Started(price))
}

/// Terminal (or interrupted) state of a request, mirrored onto its idempotency record.
pub struct Terminal<'a> {
    pub request_id: Uuid,
    pub tenant_id: Uuid,
    pub capability: Capability,
    pub idempotency_key: Option<&'a str>,
    pub state: State,
    pub reason: ErrorReason,
}

pub async fn finish_request(pool: &PgPool, end: &Terminal<'_>, now: DateTime<Utc>) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE requests SET state=$2,error_reason=$3,updated_at=$4,completed_at=CASE WHEN $2=2 THEN $4 ELSE completed_at END WHERE id=$1")
        .bind(end.request_id)
        .bind(end.state.to_db())
        .bind(end.reason as i16)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    if let Some(key) = end.idempotency_key {
        sqlx::query("UPDATE idempotency_records SET state=$5,updated_at=$6 WHERE tenant_id=$1 AND method=$2 AND idempotency_key=$3 AND request_id=$4")
            .bind(end.tenant_id)
            .bind(capability_db(end.capability))
            .bind(key)
            .bind(end.request_id)
            .bind(end.state.to_db())
            .bind(now)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Attempts
// ---------------------------------------------------------------------------------------------

pub struct NewAttempt<'a> {
    pub id: Uuid,
    pub request_id: Uuid,
    pub tenant_id: Uuid,
    pub capability: Capability,
    pub price: &'a PriceSnapshot,
    pub occurred_at: DateTime<Utc>,
}

/// Persist the attempt before the AI call so a crash leaves a durable, reconcilable record.
pub async fn insert_attempt(pool: &PgPool, attempt: &NewAttempt<'_>) -> Result<()> {
    sqlx::query("INSERT INTO attempts(id,request_id,tenant_id,capability,model,provider_route,price_effective_at,catalog_price,occurred_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$9,$9)")
        .bind(attempt.id)
        .bind(attempt.request_id)
        .bind(attempt.tenant_id)
        .bind(capability_db(attempt.capability))
        .bind(&attempt.price.model)
        .bind(&attempt.price.route)
        .bind(attempt.price.effective_at)
        .bind(attempt.price.snapshot_json())
        .bind(attempt.occurred_at)
        .execute(pool)
        .await?;
    Ok(())
}

/// Store the Gateway generation ID. A generation already attached to another attempt is ignored
/// (the unique index protects against double association).
pub async fn set_generation(
    pool: &PgPool,
    attempt_id: Uuid,
    generation_id: &str,
    now: DateTime<Utc>,
) -> bool {
    sqlx::query(
        "UPDATE attempts SET generation_id=$2,updated_at=$3 WHERE id=$1 AND generation_id=''",
    )
    .bind(attempt_id)
    .bind(generation_id)
    .bind(now)
    .execute(pool)
    .await
    .is_ok_and(|r| r.rows_affected() == 1)
}

/// Store the generation ID reported by a finished call. Idempotent: it succeeds when the ID is
/// already stored, so a failed first-chunk write can be repaired with retries.
pub async fn ensure_generation(
    pool: &PgPool,
    attempt_id: Uuid,
    generation_id: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "UPDATE attempts SET generation_id=$2,updated_at=$3 WHERE id=$1 AND generation_id=''",
    )
    .bind(attempt_id)
    .bind(generation_id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record what is known about an attempt's usage once the call ended. Known usage becomes due for
/// delivery at once; provably zero usage has nothing to report; unknown usage waits for Gateway
/// reconciliation, which needs a generation ID.
pub async fn finish_attempt(
    pool: &PgPool,
    attempt_id: Uuid,
    usage: Usage,
    reconcile_delay: Duration,
    now: DateTime<Utc>,
) -> Result<()> {
    match usage {
        Usage::Known(tokens) => {
            sqlx::query("UPDATE attempts SET measured_usage=$2,retry_at=$3,updated_at=$3 WHERE id=$1 AND measured_usage IS NULL")
                .bind(attempt_id)
                .bind(pricing::tokens_json(&tokens))
                .bind(now)
                .execute(pool)
                .await?;
        }
        Usage::Zero => {
            sqlx::query("UPDATE attempts SET measured_usage=$2,delivered_at=$3,retry_at=NULL,updated_at=$3 WHERE id=$1 AND measured_usage IS NULL")
                .bind(attempt_id)
                .bind(pricing::tokens_json(&Tokens::default()))
                .bind(now)
                .execute(pool)
                .await?;
        }
        Usage::Unknown => {
            sqlx::query("UPDATE attempts SET retry_at=CASE WHEN generation_id<>'' THEN $2 ELSE NULL END,updated_at=$3 WHERE id=$1 AND measured_usage IS NULL")
                .bind(attempt_id)
                .bind(now + reconcile_delay)
                .bind(now)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// Attempts of one request whose usage is known and not yet delivered.
pub async fn deliverable_attempts(pool: &PgPool, request_id: Uuid) -> Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM attempts WHERE request_id=$1 AND delivered_at IS NULL AND measured_usage IS NOT NULL ORDER BY created_at,id",
    )
    .bind(request_id)
    .fetch_all(pool)
    .await?)
}

/// One attempt as background delivery and reconciliation need it.
#[derive(Clone, Debug)]
pub struct DueAttempt {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub capability: Capability,
    pub model: String,
    pub generation_id: String,
    pub price_effective_at: DateTime<Utc>,
    pub catalog_price: Value,
    pub occurred_at: DateTime<Utc>,
    pub measured_usage: Option<Value>,
    pub delivery_failures: u32,
}

fn due_from_row(row: &PgRow) -> std::result::Result<DueAttempt, sqlx::Error> {
    Ok(DueAttempt {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        capability: capability_from_db(row.try_get("capability")?),
        model: row.try_get("model")?,
        generation_id: row.try_get("generation_id")?,
        price_effective_at: row.try_get("price_effective_at")?,
        catalog_price: row.try_get("catalog_price")?,
        occurred_at: row.try_get("occurred_at")?,
        measured_usage: row.try_get("measured_usage")?,
        delivery_failures: row.try_get::<i32, _>("delivery_failures")?.max(0) as u32,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// Known usage waiting for delivery to delcore.
    Delivery,
    /// Unknown usage waiting for a Gateway generation lookup.
    Reconciliation,
}

/// Claim due attempts by moving their next-try time to a lease, so a concurrent immediate delivery
/// and the periodic task never send the same attempt twice at once.
pub async fn claim_due(
    pool: &PgPool,
    work: Work,
    now: DateTime<Utc>,
    lease: Duration,
    limit: i64,
) -> Result<Vec<DueAttempt>> {
    let condition = match work {
        Work::Delivery => "delivered_at IS NULL AND measured_usage IS NOT NULL",
        Work::Reconciliation => {
            "delivered_at IS NULL AND measured_usage IS NULL AND generation_id<>'' AND NOT EXISTS(SELECT 1 FROM requests r WHERE r.id=attempts.request_id AND r.state=1)"
        }
    };
    let rows = sqlx::query(&format!(
        "WITH due AS (SELECT id FROM attempts WHERE {condition} AND (retry_at IS NULL OR retry_at<=$1) AND created_at>$4 ORDER BY retry_at NULLS FIRST,id LIMIT $3 FOR UPDATE SKIP LOCKED) UPDATE attempts SET retry_at=$2,updated_at=$1 FROM due WHERE attempts.id=due.id RETURNING attempts.id,attempts.tenant_id,attempts.capability,attempts.model,attempts.generation_id,attempts.price_effective_at,attempts.catalog_price,attempts.occurred_at,attempts.measured_usage,attempts.delivery_failures"
    ))
    .bind(now)
    .bind(now + lease)
    .bind(limit)
    .bind(cutoff(now))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(due_from_row)
        .collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Claim one specific attempt for immediate delivery if it is due and still inside its 30-day
/// retention window at `now`.
pub async fn claim_one(
    pool: &PgPool,
    id: Uuid,
    now: DateTime<Utc>,
    lease: Duration,
) -> Result<Option<DueAttempt>> {
    let row = sqlx::query("UPDATE attempts SET retry_at=$3,updated_at=$2 WHERE id=$1 AND delivered_at IS NULL AND measured_usage IS NOT NULL AND (retry_at IS NULL OR retry_at<=$2) AND created_at>$4 RETURNING id,tenant_id,capability,model,generation_id,price_effective_at,catalog_price,occurred_at,measured_usage,delivery_failures")
        .bind(id)
        .bind(now)
        .bind(now + lease)
        .bind(cutoff(now))
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(due_from_row).transpose()?)
}

pub async fn mark_delivered(pool: &PgPool, id: Uuid, now: DateTime<Utc>) -> Result<()> {
    sqlx::query("UPDATE attempts SET delivered_at=$2,retry_at=NULL,updated_at=$2 WHERE id=$1 AND delivered_at IS NULL")
        .bind(id)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

/// Count a failed delivery or lookup and schedule the next try. Lookups and deliveries of one
/// attempt never overlap in time, so they share the counter; confirming usage resets it.
pub async fn mark_retry(
    pool: &PgPool,
    id: Uuid,
    failures: u32,
    retry_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query("UPDATE attempts SET delivery_failures=$2,retry_at=$3,updated_at=$4 WHERE id=$1 AND delivered_at IS NULL")
        .bind(id)
        .bind(failures.min(i32::MAX as u32) as i32)
        .bind(retry_at)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

/// Store usage confirmed by Gateway reconciliation; it is then due for delivery. `now` must be read
/// after the Gateway lookup: usage that was not established before the 30-day boundary is
/// abandoned with the record, so an attempt that expired during the lookup is left unchanged.
///
/// The failure counter is reset because unsuccessful lookups share it with delivery (`mark_retry`):
/// delivery backoff and the first-failure event must start from the first real delivery failure,
/// not from the number of lookups that preceded the confirmation.
pub async fn set_measured(
    pool: &PgPool,
    id: Uuid,
    tokens: &Tokens,
    now: DateTime<Utc>,
) -> Result<bool> {
    let result = sqlx::query("UPDATE attempts SET measured_usage=$2,delivery_failures=0,retry_at=$3,updated_at=$3 WHERE id=$1 AND measured_usage IS NULL AND created_at>$4")
        .bind(id)
        .bind(pricing::tokens_json(tokens))
        .bind(now)
        .bind(cutoff(now))
        .execute(pool)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// What the operator association needs to know about an attempt.
pub struct AttemptInfo {
    pub tenant_id: Uuid,
    pub request_id: Uuid,
    pub model: String,
    pub generation_id: String,
    pub usage_known: bool,
    pub delivered: bool,
    /// The parent request is still `processing`, so the attempt's own call may still be running
    /// and will store its own generation and usage.
    pub request_running: bool,
}

/// The retained attempt, if any. An attempt at or past its 30-day boundary is expired even before
/// the retention sweep deletes it, so it is reported as absent.
pub async fn attempt_info(
    pool: &PgPool,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<AttemptInfo>> {
    let row = sqlx::query("SELECT tenant_id,request_id,model,generation_id,measured_usage IS NOT NULL AS usage_known,delivered_at IS NOT NULL AS delivered,EXISTS(SELECT 1 FROM requests r WHERE r.id=attempts.request_id AND r.state=1) AS request_running FROM attempts WHERE id=$1 AND created_at>$2")
        .bind(id)
        .bind(cutoff(now))
        .fetch_optional(pool)
        .await?;
    row.map(|row| -> std::result::Result<AttemptInfo, sqlx::Error> {
        Ok(AttemptInfo {
            tenant_id: row.try_get("tenant_id")?,
            request_id: row.try_get("request_id")?,
            model: row.try_get("model")?,
            generation_id: row.try_get("generation_id")?,
            usage_known: row.try_get("usage_known")?,
            delivered: row.try_get("delivered")?,
            request_running: row.try_get("request_running")?,
        })
    })
    .transpose()
    .map_err(Into::into)
}

pub enum Association {
    Associated,
    /// The generation is already attached to another retained attempt.
    GenerationInUse,
    /// The attempt no longer qualifies (usage known, delivered, already has a generation, its
    /// request is still running or it reached its 30-day expiry).
    NotEligible,
}

/// Attach a validated generation ID and schedule reconciliation, auditing in the same
/// transaction.
pub async fn associate_generation(
    pool: &PgPool,
    attempt_id: Uuid,
    generation_id: &str,
    audit: super::audit::AuditEntry<'_>,
    now: DateTime<Utc>,
) -> Result<Association> {
    let mut tx = pool.begin().await?;
    let updated = sqlx::query("UPDATE attempts SET generation_id=$2,retry_at=$3,updated_at=$3 WHERE id=$1 AND generation_id='' AND measured_usage IS NULL AND delivered_at IS NULL AND created_at>$4 AND NOT EXISTS(SELECT 1 FROM requests r WHERE r.id=attempts.request_id AND r.state=1)")
        .bind(attempt_id)
        .bind(generation_id)
        .bind(now)
        .bind(cutoff(now))
        .execute(&mut *tx)
        .await;
    match updated {
        Ok(result) if result.rows_affected() == 1 => {}
        Ok(_) => {
            tx.rollback().await?;
            return Ok(Association::NotEligible);
        }
        Err(sqlx::Error::Database(db)) if db.code().as_deref() == Some("23505") => {
            let _ = tx.rollback().await;
            return Ok(Association::GenerationInUse);
        }
        Err(error) => return Err(error.into()),
    }
    super::audit::insert(&mut tx, audit, now).await?;
    tx.commit().await?;
    Ok(Association::Associated)
}
