// SPDX-License-Identifier: FSL-1.1-ALv2

use super::*;
use chrono::{DateTime, Timelike, Utc};
use sqlx::{PgConnection, Row};
use std::collections::{BTreeMap, BTreeSet};

struct Meter {
    id: String,
    definition: metering::Definition,
    untyped: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Contribution {
    pub meter_id: String,
    pub meter_key: String,
    pub aggregation: metering::Aggregation,
    pub group_key: String,
    pub group_values: BTreeMap<String, metering::Value>,
    pub value: metering::Value,
}

fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}
fn valid_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err(reject(
            Reason::InvalidInput,
            &format!("event key must contain 1 to {MAX_KEY_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// Accept an original event and queue its processing in the caller transaction.
pub async fn ingest(
    tx: &mut PgConnection,
    now: DateTime<Utc>,
    policy: &Policy,
    mut input: Input,
) -> Result<Accepted> {
    valid_key(&input.key)?;
    validate_content(&input.event_type, &input.attributes)?;
    // PostgreSQL retains microseconds. Hash exactly the timestamp that is stored.
    input.occurred_at = input
        .occurred_at
        .with_nanosecond(input.occurred_at.nanosecond() / 1000 * 1000)
        .unwrap();
    let digest = event_fingerprint(&input);
    let existing = sqlx::query("SELECT id::text,fingerprint FROM metering_events WHERE service_id=$1::text::uuid AND event_key=$2 UNION ALL SELECT event_id::text,fingerprint FROM metering_replay WHERE service_id=$1::text::uuid AND event_key=$2 LIMIT 1")
        .bind(&input.service_id).bind(&input.key).fetch_optional(&mut *tx).await?;
    if let Some(row) = existing {
        if row.get::<String, _>("fingerprint") != digest {
            return Err(reject(
                Reason::IdentityConflict,
                "event key was accepted with different content",
            ));
        }
        return Ok(Accepted {
            event_id: row.get(0),
            revision: 1,
            replayed: true,
            ..Default::default()
        });
    }
    if input.occurred_at > now {
        return Err(reject(
            Reason::InvalidInput,
            "event occurrence is in the future",
        ));
    }
    if input.occurred_at < now - policy.lateness {
        return Err(reject(
            Reason::WindowClosed,
            "event occurrence is older than the accepted lateness",
        ));
    }
    tenant_open(tx, &input.tenant_id, input.occurred_at, now, policy).await?;
    let (start, end) =
        crate::postgres::period::billing_period(tx, &input.tenant_id, input.occurred_at)
            .await
            .map_err(|e| {
                if e.downcast_ref::<crate::postgres::period::PeriodUnavailable>()
                    .is_some()
                {
                    reject(
                        Reason::PeriodUnavailable,
                        "historical billing period unavailable",
                    )
                } else {
                    Fault::from(e)
                }
            })?;
    let meters = service_meters(tx, &input.service_id).await?;
    let added = match_meters(
        &meters,
        &metering::Event {
            event_type: input.event_type.clone(),
            attributes: input.attributes.clone(),
        },
    )?;
    admit(tx, &input.tenant_id, start, &[], &added, &policy.limits).await?;
    let id = new_id();
    let meter_ids: Vec<String> = meters.iter().map(|m| m.id.clone()).collect();
    sqlx::query("INSERT INTO metering_events(id,tenant_id,service_id,event_key,fingerprint,occurred_at,accepted_at,period_start,period_end,meter_ids,latest_revision,applied_revision) VALUES($1::text::uuid,$2::text::uuid,$3::text::uuid,$4,$5,$6,$7,$8,$9,$10::text[]::uuid[],1,0)")
        .bind(&id).bind(&input.tenant_id).bind(&input.service_id).bind(&input.key).bind(&digest).bind(input.occurred_at).bind(now).bind(start).bind(end).bind(&meter_ids).execute(&mut *tx).await?;
    attribute_prices(
        tx,
        &id,
        &input.tenant_id,
        &input.service_id,
        input.occurred_at,
        &meter_ids,
    )
    .await?;
    record(
        tx,
        &id,
        &input.tenant_id,
        1,
        metering::RevisionKind::Insert,
        &input.event_type,
        &input.attributes,
        &digest,
        &added,
        now,
    )
    .await?;
    Ok(Accepted {
        event_id: id,
        revision: 1,
        meters: added.len(),
        ..Default::default()
    })
}

/// Append a replacement or cancellation using the latest revision as a CAS.
pub async fn correct(
    tx: &mut PgConnection,
    now: DateTime<Utc>,
    policy: &Policy,
    c: Correction,
) -> Result<Accepted> {
    valid_key(&c.key)?;
    if c.expected_revision < 1 {
        return Err(reject(
            Reason::InvalidInput,
            "expected revision is required",
        ));
    }
    match c.kind {
        metering::RevisionKind::Replace => validate_content(&c.event_type, &c.attributes)?,
        metering::RevisionKind::Cancel => {
            if !c.event_type.is_empty() || !c.attributes.is_empty() {
                return Err(reject(
                    Reason::InvalidInput,
                    "a cancellation carries no event content",
                ));
            }
        }
        _ => {
            return Err(reject(
                Reason::InvalidInput,
                "correction kind must be replace or cancel",
            ));
        }
    }
    let row = sqlx::query("SELECT id::text,tenant_id::text,occurred_at,period_start,meter_ids::text[],latest_revision FROM metering_events WHERE service_id=$1::text::uuid AND event_key=$2 FOR UPDATE")
        .bind(&c.service_id).bind(&c.key).fetch_optional(&mut *tx).await?.ok_or_else(||reject(Reason::NotFound,"event is not retained for this service"))?;
    let id: String = row.get(0);
    let tenant: String = row.get(1);
    let occurred: DateTime<Utc> = row.get(2);
    let start: DateTime<Utc> = row.get(3);
    let meter_ids: Vec<String> = row.get(4);
    let latest: i32 = row.get(5);
    if tenant != c.tenant_id {
        return Err(reject(Reason::Forbidden, "operation is not permitted"));
    }
    let digest = correction_fingerprint(&id, &c);
    if c.expected_revision != latest {
        if c.expected_revision < latest {
            let next: String = sqlx::query_scalar("SELECT fingerprint FROM metering_event_revisions WHERE event_id=$1::text::uuid AND revision=$2").bind(&id).bind(c.expected_revision+1).fetch_one(&mut *tx).await?;
            if next == digest {
                return Ok(Accepted {
                    event_id: id,
                    revision: c.expected_revision + 1,
                    replayed: true,
                    ..Default::default()
                });
            }
        }
        return Err(reject(
            Reason::StaleRevision,
            "expected revision is not the latest revision",
        ));
    }
    let kind: String = sqlx::query_scalar(
        "SELECT kind FROM metering_event_revisions WHERE event_id=$1::text::uuid AND revision=$2",
    )
    .bind(&id)
    .bind(latest)
    .fetch_one(&mut *tx)
    .await?;
    if kind == "cancel" {
        return Err(reject(
            Reason::Cancelled,
            "a cancelled event cannot be corrected",
        ));
    }
    if occurred < now - policy.lateness {
        return Err(reject(
            Reason::WindowClosed,
            "event occurrence is older than the accepted lateness",
        ));
    }
    tenant_open(tx, &tenant, occurred, now, policy).await?;
    let removed = load_contributions(tx, &id, latest).await?;
    let added = if c.kind == metering::RevisionKind::Replace {
        // Keep the original evaluation set; later meters do not backfill events.
        let meters = load_meters(tx, None, Some(&meter_ids)).await?;
        match_meters(
            &meters,
            &metering::Event {
                event_type: c.event_type.clone(),
                attributes: c.attributes.clone(),
            },
        )?
    } else {
        Vec::new()
    };
    admit(tx, &tenant, start, &removed, &added, &policy.limits).await?;
    let next = latest + 1;
    sqlx::query("UPDATE metering_events SET latest_revision=$2 WHERE id=$1::text::uuid")
        .bind(&id)
        .bind(next)
        .execute(&mut *tx)
        .await?;
    record(
        tx,
        &id,
        &tenant,
        next,
        c.kind,
        &c.event_type,
        &c.attributes,
        &digest,
        &added,
        now,
    )
    .await?;
    Ok(Accepted {
        event_id: id,
        revision: next,
        meters: added.len(),
        ..Default::default()
    })
}

pub async fn get(
    tx: &mut PgConnection,
    tenant_id: &str,
    service_id: &str,
    key: &str,
) -> Result<Snapshot> {
    valid_key(key)?;
    let row = sqlx::query("SELECT e.id::text,e.tenant_id::text,e.occurred_at,e.latest_revision,e.applied_revision,r.kind,r.event_type,r.attributes FROM metering_events e JOIN metering_event_revisions r ON r.event_id=e.id AND r.revision=e.latest_revision WHERE e.service_id=$1::text::uuid AND e.event_key=$2")
        .bind(service_id).bind(key).fetch_optional(&mut *tx).await?.ok_or_else(||reject(Reason::NotFound,"event is not retained for this service"))?;
    if row.get::<String, _>(1) != tenant_id {
        return Err(reject(Reason::Forbidden, "operation is not permitted"));
    }
    Ok(Snapshot {
        id: row.get(0),
        tenant_id: row.get(1),
        service_id: service_id.into(),
        key: key.into(),
        occurred_at: row.get(2),
        revision: row.get(3),
        applied_revision: row.get(4),
        cancelled: row.get::<String, _>(5) == "cancel",
        event_type: row.get(6),
        attributes: decode_values(&row.get(7))?,
    })
}

async fn tenant_open(
    tx: &mut PgConnection,
    tenant: &str,
    occurred: DateTime<Utc>,
    now: DateTime<Utc>,
    policy: &Policy,
) -> Result<()> {
    let deleting: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT deleting_at FROM tenants WHERE id=$1::text::uuid AND deleted_at IS NULL",
    )
    .bind(tenant)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| reject(Reason::Forbidden, "operation is not permitted"))?;
    if deleting.is_some_and(|at| occurred > at || now > at + policy.reporting_grace) {
        return Err(reject(
            Reason::WindowClosed,
            "tenant reporting grace has ended",
        ));
    }
    Ok(())
}

async fn load_meters(
    tx: &mut PgConnection,
    service: Option<&str>,
    ids: Option<&[String]>,
) -> Result<Vec<Meter>> {
    let rows = sqlx::query("SELECT ce.id::text,ce.key,cv.definition,COALESCE(cv.definition->>'event_type','')='' FROM catalog_entities ce LEFT JOIN catalog_versions cv ON cv.id=ce.active_version_id WHERE (($1::text IS NOT NULL AND ce.scope=$1) OR ($2::text[] IS NOT NULL AND ce.id=ANY($2::text[]::uuid[]))) AND ce.kind='meter' ORDER BY ce.key,ce.id")
        .bind(service).bind(ids).fetch_all(&mut *tx).await?;
    rows.into_iter()
        .map(|row| {
            let raw: Option<serde_json::Value> = row.get(2);
            Ok(Meter {
                id: row.get(0),
                definition: decode_meter(
                    &row.get::<String, _>(1),
                    &raw.unwrap_or(serde_json::Value::Null),
                )?,
                untyped: row.get(3),
            })
        })
        .collect()
}

async fn service_meters(tx: &mut PgConnection, service: &str) -> Result<Vec<Meter>> {
    let mut locked = BTreeSet::new();
    loop {
        let meters = load_meters(tx, Some(service), None).await?;
        let pending: Vec<String> = meters
            .iter()
            .filter(|m| m.untyped && !locked.contains(&m.id))
            .map(|m| m.id.clone())
            .collect();
        if pending.is_empty() {
            return Ok(meters);
        }
        // Typing an unused compatibility meter must serialize with acceptance.
        // Re-read after locking so a prior typing transaction becomes visible.
        sqlx::query("SELECT id FROM catalog_entities WHERE id=ANY($1::text[]::uuid[]) ORDER BY id FOR SHARE").bind(&pending).fetch_all(&mut *tx).await?;
        locked.extend(pending);
    }
}

async fn attribute_prices(
    tx: &mut PgConnection,
    event: &str,
    tenant: &str,
    service: &str,
    occurred: DateTime<Utc>,
    ids: &[String],
) -> Result<()> {
    // Product ownership is historical, but its usage prices are selected at
    // execution start. New price keys must also apply to existing subscribers.
    sqlx::query(r#"WITH products AS (
 SELECT cs.product_id FROM tenant_catalog_selections cs
 WHERE cs.tenant_id=$2::text::uuid AND cs.starts_at<=$4 AND (cs.ends_at IS NULL OR cs.ends_at>$4)
 UNION
 SELECT t.plan_id FROM tenants t WHERE t.id=$2::text::uuid AND NOT EXISTS(
  SELECT 1 FROM tenant_catalog_selections cs JOIN catalog_entities ce ON ce.id=cs.product_id
  WHERE cs.tenant_id=t.id AND ce.kind='plan' AND cs.starts_at<=$4 AND (cs.ends_at IS NULL OR cs.ends_at>$4))
 UNION
 SELECT p.addon_id FROM entitlements en JOIN prices p ON p.id=en.price_id
 WHERE en.tenant_id=$2::text::uuid AND p.addon_id IS NOT NULL
 AND en.starts_at<=$4 AND (en.ends_at IS NULL OR en.ends_at>$4)
), applicable AS (
 SELECT p.id,p.service_id,p.meter,p.kind,a.effective_at,1 AS versioned
 FROM products held JOIN catalog_entities product ON product.id=held.product_id
 JOIN catalog_entities pk ON pk.kind='price' AND pk.scope=product.kind||':'||product.id::text
 CROSS JOIN LATERAL (SELECT version_id,effective_at FROM catalog_price_activations
  WHERE price_id=pk.id AND effective_at<=$4 ORDER BY effective_at DESC LIMIT 1) a
 JOIN prices p ON p.id=a.version_id
 UNION ALL
 SELECT p.id,p.service_id,p.meter,p.kind,en.starts_at,0 FROM entitlements en JOIN prices p ON p.id=en.price_id
 LEFT JOIN catalog_versions cv ON cv.id=p.id
 WHERE en.tenant_id=$2::text::uuid AND en.starts_at<=$4 AND (en.ends_at IS NULL OR en.ends_at>$4)
 AND NOT EXISTS(SELECT 1 FROM catalog_price_activations a WHERE a.price_id=cv.entity_id AND a.effective_at<=$4)
)
INSERT INTO metering_event_prices(event_id,tenant_id,meter_id,price_id)
 SELECT $1::text::uuid,$2::text::uuid,ce.id,pr.id FROM catalog_entities ce
 CROSS JOIN LATERAL (SELECT p.id FROM applicable p
  WHERE p.service_id=$3::text::uuid AND p.meter=ce.key AND p.kind=1
  ORDER BY p.versioned DESC,p.effective_at DESC,p.id DESC LIMIT 1) pr
 WHERE ce.id=ANY($5::text[]::uuid[]) AND ce.kind='meter' AND ce.scope=$3::text"#)
        .bind(event).bind(tenant).bind(service).bind(occurred).bind(ids).execute(&mut *tx).await?;
    Ok(())
}

fn match_meters(meters: &[Meter], event: &metering::Event) -> Result<Vec<Contribution>> {
    if meters.is_empty() {
        return Err(reject(
            Reason::NoMatchingMeter,
            "no meter matches the event",
        ));
    }
    let definitions: Vec<_> = meters.iter().map(|m| m.definition.clone()).collect();
    let matched = metering::match_event(&definitions, event).map_err(|err| match err {
        metering::MeteringError::NoMatch => {
            reject(Reason::NoMatchingMeter, "no meter matches the event")
        }
        metering::MeteringError::Event(_) => reject(
            Reason::InvalidAttribute,
            "a matched meter lacks a required attribute or receives an invalid type",
        ),
        _ => permanent("stored meter definition is invalid"),
    })?;
    matched
        .into_iter()
        .map(|c| {
            let m = meters
                .iter()
                .find(|m| m.definition.key == c.meter_key)
                .ok_or_else(|| permanent("matched meter is missing"))?;
            Ok(Contribution {
                meter_id: m.id.clone(),
                meter_key: c.meter_key,
                aggregation: m.definition.aggregation,
                group_key: c.group_key,
                group_values: c.group_values,
                value: c.value.unwrap_or_else(|| metering::Value::string("")),
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn record(
    tx: &mut PgConnection,
    event: &str,
    tenant: &str,
    revision: i32,
    kind: metering::RevisionKind,
    event_type: &str,
    attributes: &BTreeMap<String, metering::Value>,
    digest: &str,
    contributions: &[Contribution],
    now: DateTime<Utc>,
) -> Result<()> {
    let empty = BTreeMap::new();
    let encoded = encode_values(if kind == metering::RevisionKind::Cancel {
        &empty
    } else {
        attributes
    })?;
    sqlx::query("INSERT INTO metering_event_revisions(event_id,tenant_id,revision,kind,event_type,attributes,fingerprint,created_at) VALUES($1::text::uuid,$2::text::uuid,$3,$4,$5,$6,$7,$8)")
        .bind(event).bind(tenant).bind(revision).bind(kind.as_str()).bind(event_type).bind(encoded).bind(digest).bind(now).execute(&mut *tx).await?;
    for c in contributions {
        sqlx::query("INSERT INTO metering_contributions(event_id,tenant_id,revision,meter_id,meter_key,aggregation,group_key,group_values,value_kind,value) VALUES($1::text::uuid,$2::text::uuid,$3,$4::text::uuid,$5,$6,$7,$8,$9,$10)")
            .bind(event).bind(tenant).bind(revision).bind(&c.meter_id).bind(&c.meter_key).bind(c.aggregation.as_str()).bind(&c.group_key).bind(encode_values(&c.group_values)?).bind(if c.aggregation==metering::Aggregation::Count {""} else {c.value.kind.as_str()}).bind(&c.value.text).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1::text::uuid,$2::text::uuid,$3,$4,$5,$6,$6,$6)")
        .bind(new_id()).bind(tenant).bind(JOB_KIND).bind(format!("{JOB_KIND}:{event}:{revision}")).bind(serde_json::json!({"event_id":event,"revision":revision})).bind(now).execute(&mut *tx).await?;
    Ok(())
}

pub(super) async fn load_contributions(
    tx: &mut PgConnection,
    event: &str,
    revision: i32,
) -> Result<Vec<Contribution>> {
    let rows = sqlx::query("SELECT meter_id::text,meter_key,aggregation,group_key,group_values,value_kind,value FROM metering_contributions WHERE event_id=$1::text::uuid AND revision=$2 ORDER BY meter_key,meter_id").bind(event).bind(revision).fetch_all(&mut *tx).await?;
    rows.into_iter()
        .map(|r| {
            Ok(Contribution {
                meter_id: r.get(0),
                meter_key: r.get(1),
                aggregation: serde_json::from_value(serde_json::Value::String(r.get(2)))
                    .map_err(|_| permanent("invalid stored aggregation"))?,
                group_key: r.get(3),
                group_values: decode_values(&r.get(4))?,
                value: metering::Value {
                    kind: if r.get::<String, _>(5).is_empty() && r.get::<String, _>(2) == "count" {
                        metering::ValueKind::String
                    } else {
                        serde_json::from_value(serde_json::Value::String(r.get(5)))
                            .map_err(|_| permanent("invalid stored value kind"))?
                    },
                    text: r.get(6),
                },
            })
        })
        .collect()
}

/// How a removed contribution relates to the replacement of the same correction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Retention {
    /// The group and every retained value are shared; admission rows stay as committed.
    Identity,
    /// Only the group is shared; the retained value moves to another typed value.
    Group,
}

/// Pair removed and added contributions of one meter that keep an admission identity.
/// Returns, per removed index, the paired added index and what is shared. Unpaired
/// contributions are released and acquired in full.
fn pair_retained(
    removed: &[Contribution],
    added: &[Contribution],
) -> Vec<Option<(usize, Retention)>> {
    let mut used = vec![false; added.len()];
    let mut pairs = vec![None; removed.len()];
    for retention in [Retention::Identity, Retention::Group] {
        for (i, r) in removed.iter().enumerate() {
            if pairs[i].is_some() {
                continue;
            }
            let found = added.iter().enumerate().position(|(j, a)| {
                !used[j]
                    && a.meter_id == r.meter_id
                    && a.group_key == r.group_key
                    && (retention == Retention::Group
                        || !metering::retains_values(r.aggregation)
                        || (a.value.kind == r.value.kind && a.value.text == r.value.text))
            });
            if let Some(j) = found {
                used[j] = true;
                pairs[i] = Some((j, retention));
            }
        }
    }
    pairs
}

// Lock period rows in a stable order. Account for accepted, unprocessed revisions,
// releasing removed identities first so replacements need only destination capacity.
// Identities that the previous and replacement revisions share keep their rows and
// counts, so a bound lowered below existing usage never rejects a retained identity.
async fn admit(
    tx: &mut PgConnection,
    tenant: &str,
    period: DateTime<Utc>,
    removed: &[Contribution],
    added: &[Contribution],
    limits: &metering::Limits,
) -> Result<()> {
    let ids: BTreeSet<_> = removed
        .iter()
        .chain(added)
        .map(|c| c.meter_id.clone())
        .collect();
    let mut groups = BTreeMap::new();
    for id in &ids {
        sqlx::query("INSERT INTO metering_admission_periods(tenant_id,meter_id,period_start,groups) VALUES($1::text::uuid,$2::text::uuid,$3,0) ON CONFLICT DO NOTHING").bind(tenant).bind(id).bind(period).execute(&mut *tx).await?;
        let n:i64 = sqlx::query_scalar("SELECT groups FROM metering_admission_periods WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 FOR UPDATE").bind(tenant).bind(id).bind(period).fetch_one(&mut *tx).await?;
        groups.insert(id.clone(), n);
    }
    let pairs = pair_retained(removed, added);
    let mut retained = vec![None; added.len()];
    for (j, retention) in pairs.iter().flatten() {
        retained[*j] = Some(*retention);
    }
    for (i, c) in removed.iter().enumerate() {
        match pairs[i] {
            Some((_, Retention::Identity)) => {}
            Some((_, Retention::Group)) => {
                release(tx, tenant, period, c, &mut groups, true).await?
            }
            None => release(tx, tenant, period, c, &mut groups, false).await?,
        }
    }
    for (j, c) in added.iter().enumerate() {
        match retained[j] {
            Some(Retention::Identity) => {}
            Some(Retention::Group) => {
                acquire(tx, tenant, period, c, &mut groups, limits, true).await?
            }
            None => acquire(tx, tenant, period, c, &mut groups, limits, false).await?,
        }
    }
    for id in ids {
        sqlx::query("UPDATE metering_admission_periods SET groups=$4 WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3").bind(tenant).bind(&id).bind(period).bind(groups[&id]).execute(&mut *tx).await?;
    }
    Ok(())
}

async fn release(
    tx: &mut PgConnection,
    tenant: &str,
    period: DateTime<Utc>,
    c: &Contribution,
    groups: &mut BTreeMap<String, i64>,
    keep_group: bool,
) -> Result<()> {
    let digest = group_digest(&c.group_key);
    let row = sqlx::query("SELECT events,distinct_values FROM metering_admission_groups WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4 FOR UPDATE").bind(tenant).bind(&c.meter_id).bind(period).bind(&digest).fetch_one(&mut *tx).await?;
    let events: i64 = row.get(0);
    let mut distinct: i64 = row.get(1);
    if metering::retains_values(c.aggregation) {
        let value_events:i64 = sqlx::query_scalar("SELECT events FROM metering_admission_values WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4 AND value_kind=$5 AND value=$6 FOR UPDATE").bind(tenant).bind(&c.meter_id).bind(period).bind(&digest).bind(c.value.kind.as_str()).bind(&c.value.text).fetch_one(&mut *tx).await?;
        let query = if value_events == 1 {
            distinct -= 1;
            "DELETE FROM metering_admission_values WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4 AND value_kind=$5 AND value=$6"
        } else {
            "UPDATE metering_admission_values SET events=events-1 WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4 AND value_kind=$5 AND value=$6"
        };
        sqlx::query(query)
            .bind(tenant)
            .bind(&c.meter_id)
            .bind(period)
            .bind(&digest)
            .bind(c.value.kind.as_str())
            .bind(&c.value.text)
            .execute(&mut *tx)
            .await?;
    }
    if keep_group {
        // The replacement keeps this group, so only its distinct-value count changes.
        sqlx::query("UPDATE metering_admission_groups SET distinct_values=$5 WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4").bind(tenant).bind(&c.meter_id).bind(period).bind(&digest).bind(distinct).execute(&mut *tx).await?;
    } else if events == 1 {
        sqlx::query("DELETE FROM metering_admission_groups WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4").bind(tenant).bind(&c.meter_id).bind(period).bind(&digest).execute(&mut *tx).await?;
        *groups.get_mut(&c.meter_id).unwrap() -= 1;
    } else {
        sqlx::query("UPDATE metering_admission_groups SET events=events-1,distinct_values=$5 WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4").bind(tenant).bind(&c.meter_id).bind(period).bind(&digest).bind(distinct).execute(&mut *tx).await?;
    }
    Ok(())
}

async fn acquire(
    tx: &mut PgConnection,
    tenant: &str,
    period: DateTime<Utc>,
    c: &Contribution,
    groups: &mut BTreeMap<String, i64>,
    limits: &metering::Limits,
    existing_group: bool,
) -> Result<()> {
    let digest = group_digest(&c.group_key);
    let current:Option<i64> = sqlx::query_scalar("SELECT distinct_values FROM metering_admission_groups WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4 FOR UPDATE").bind(tenant).bind(&c.meter_id).bind(period).bind(&digest).fetch_optional(&mut *tx).await?;
    let distinct = current.unwrap_or(0);
    if existing_group {
        // A correction keeps the group; release left it in place with its event count.
        if current.is_none() {
            return Err(permanent("retained admission group is missing"));
        }
    } else if current.is_none() {
        if limits.max_groups_per_meter > 0 && groups[&c.meter_id] + 1 > limits.max_groups_per_meter
        {
            return Err(reject_limit(
                Limit::Groups,
                &format!(
                    "meter {:?} reached its group limit for the billing period",
                    c.meter_key
                ),
            ));
        }
        *groups.get_mut(&c.meter_id).unwrap() += 1;
        sqlx::query("INSERT INTO metering_admission_groups(tenant_id,meter_id,period_start,group_digest,events,distinct_values) VALUES($1::text::uuid,$2::text::uuid,$3,$4,1,0)").bind(tenant).bind(&c.meter_id).bind(period).bind(&digest).execute(&mut *tx).await?;
    } else {
        sqlx::query("UPDATE metering_admission_groups SET events=events+1 WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4").bind(tenant).bind(&c.meter_id).bind(period).bind(&digest).execute(&mut *tx).await?;
    }
    if !metering::retains_values(c.aggregation) {
        return Ok(());
    }
    let updated = sqlx::query("UPDATE metering_admission_values SET events=events+1 WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4 AND value_kind=$5 AND value=$6").bind(tenant).bind(&c.meter_id).bind(period).bind(&digest).bind(c.value.kind.as_str()).bind(&c.value.text).execute(&mut *tx).await?;
    if updated.rows_affected() == 1 {
        return Ok(());
    }
    if limits.max_distinct_values_per_group > 0
        && distinct + 1 > limits.max_distinct_values_per_group
    {
        return Err(reject_limit(
            Limit::DistinctValues,
            &format!(
                "meter {:?} reached its distinct-value limit for a group",
                c.meter_key
            ),
        ));
    }
    sqlx::query("INSERT INTO metering_admission_values(tenant_id,meter_id,period_start,group_digest,value_kind,value,events) VALUES($1::text::uuid,$2::text::uuid,$3,$4,$5,$6,1)").bind(tenant).bind(&c.meter_id).bind(period).bind(&digest).bind(c.value.kind.as_str()).bind(&c.value.text).execute(&mut *tx).await?;
    sqlx::query("UPDATE metering_admission_groups SET distinct_values=distinct_values+1 WHERE tenant_id=$1::text::uuid AND meter_id=$2::text::uuid AND period_start=$3 AND group_digest=$4").bind(tenant).bind(&c.meter_id).bind(period).bind(&digest).execute(&mut *tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contribution(
        meter: &str,
        aggregation: metering::Aggregation,
        group: &str,
        value: &str,
    ) -> Contribution {
        Contribution {
            meter_id: meter.into(),
            meter_key: meter.into(),
            aggregation,
            group_key: group.into(),
            group_values: BTreeMap::new(),
            value: metering::Value::string(value),
        }
    }

    #[test]
    fn corrections_pair_shared_admission_identities() {
        use metering::Aggregation::{Count, UniqueCount};
        let old = [
            contribution("count", Count, "a", ""),
            contribution("users", UniqueCount, "a", "u1"),
            contribution("moved", Count, "a", ""),
        ];
        let new = [
            contribution("users", UniqueCount, "a", "u2"),
            contribution("count", Count, "a", ""),
            contribution("moved", Count, "b", ""),
        ];
        assert_eq!(
            pair_retained(&old, &new),
            vec![
                Some((1, Retention::Identity)),
                Some((0, Retention::Group)),
                None
            ]
        );
        assert_eq!(
            pair_retained(&old[1..2], &[contribution("users", UniqueCount, "a", "u1")]),
            vec![Some((0, Retention::Identity))]
        );
        assert_eq!(pair_retained(&old, &[]), vec![None; 3]);
    }

    #[test]
    fn event_keys_use_byte_bounds() {
        assert!(valid_key("").is_err());
        assert!(valid_key(&"a".repeat(MAX_KEY_BYTES)).is_ok());
        assert!(valid_key(&"a".repeat(MAX_KEY_BYTES + 1)).is_err());
        assert!(valid_key(&"界".repeat(MAX_KEY_BYTES / 3 + 1)).is_err());
    }

    #[test]
    fn matching_preserves_meter_identity_and_rejects_partial_matches() {
        let first = compatibility_definition("requests");
        let mut second = first.clone();
        second.key = "grouped".into();
        second.group_by = vec!["region".into()];
        let meters = vec![
            Meter {
                id: "first".into(),
                definition: first,
                untyped: true,
            },
            Meter {
                id: "second".into(),
                definition: second,
                untyped: false,
            },
        ];
        let mut event = metering::Event {
            event_type: "requests".into(),
            attributes: BTreeMap::from([(
                "quantity".into(),
                metering::Value::number("2").unwrap(),
            )]),
        };
        // The first meter matches, but the second lacks a group attribute.
        // No partial contribution list may escape to the admission transaction.
        assert!(match_meters(&meters, &event).is_err());
        event
            .attributes
            .insert("region".into(), metering::Value::string("west"));
        let contributions = match_meters(&meters, &event).unwrap();
        assert_eq!(contributions.len(), 2);
        let first = contributions
            .iter()
            .find(|c| c.meter_id == "first")
            .unwrap();
        assert_eq!(first.meter_key, "requests");
        assert_eq!(first.value.text, "2");
        let second = contributions
            .iter()
            .find(|c| c.meter_id == "second")
            .unwrap();
        assert_eq!(second.group_values["region"].text, "west");
        event.event_type = "unmatched".into();
        assert!(match_meters(&meters, &event).is_err());
        assert!(match_meters(&[], &event).is_err());
    }
}
