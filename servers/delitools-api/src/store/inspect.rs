// SPDX-License-Identifier: FSL-1.1-ALv2
//! Operator request inspection: safe state, attempt, price and usage metadata only.
use crate::error::Result;
use buffa::Enumeration;
use buffa_types::google::protobuf::Timestamp;
use chrono::{DateTime, Utc};
use delino_protocol::delitools::v1::{Attempt, Capability, ErrorReason, RequestState};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug)]
pub struct Inspection {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub capability: Capability,
    pub state: RequestState,
    pub model: String,
    pub error_reason: ErrorReason,
    pub created_at: DateTime<Utc>,
    pub attempts: Vec<Attempt>,
}

fn timestamp(value: DateTime<Utc>) -> Timestamp {
    Timestamp {
        seconds: value.timestamp(),
        nanos: value.timestamp_subsec_nanos() as i32,
        ..Default::default()
    }
}

pub async fn request(pool: &PgPool, id: Uuid) -> Result<Option<Inspection>> {
    let Some(row) = sqlx::query("SELECT id,tenant_id,capability,state,model,error_reason,created_at FROM requests WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(None);
    };
    let attempts = sqlx::query("SELECT id,generation_id,measured_usage IS NOT NULL AS usage_known,delivered_at IS NOT NULL AS delivered,created_at,catalog_price::text AS catalog_price,measured_usage::text AS measured_usage,occurred_at,retry_at,delivery_failures,price_effective_at FROM attempts WHERE request_id=$1 ORDER BY created_at,id")
        .bind(id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|a| -> Result<Attempt> {
            Ok(Attempt {
                id: a.try_get::<Uuid, _>("id")?.to_string(),
                generation_id: a.try_get("generation_id")?,
                usage_known: a.try_get("usage_known")?,
                delivered: a.try_get("delivered")?,
                created_at: timestamp(a.try_get("created_at")?).into(),
                catalog_price_json: a.try_get("catalog_price")?,
                measured_usage_json: a
                    .try_get::<Option<String>, _>("measured_usage")?
                    .unwrap_or_default(),
                occurred_at: timestamp(a.try_get("occurred_at")?).into(),
                retry_at: a
                    .try_get::<Option<DateTime<Utc>>, _>("retry_at")?
                    .map(timestamp)
                    .into(),
                delivery_failures: a.try_get::<i32, _>("delivery_failures")?.max(0) as u32,
                price_effective_at: timestamp(a.try_get("price_effective_at")?).into(),
                ..Default::default()
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(Inspection {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        capability: Capability::from_i32(i32::from(row.try_get::<i16, _>("capability")?))
            .unwrap_or(Capability::CAPABILITY_UNSPECIFIED),
        state: RequestState::from_i32(i32::from(row.try_get::<i16, _>("state")?))
            .unwrap_or(RequestState::REQUEST_STATE_UNSPECIFIED),
        model: row.try_get("model")?,
        error_reason: ErrorReason::from_i32(i32::from(row.try_get::<i16, _>("error_reason")?))
            .unwrap_or(ErrorReason::ERROR_REASON_UNSPECIFIED),
        created_at: row.try_get("created_at")?,
        attempts,
    }))
}
