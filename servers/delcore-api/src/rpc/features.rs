// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Call, invalid, string, uuid};
use crate::{
    Result,
    accounting::Decimal,
    events,
    features::{self, BlockReason, FeatureKind},
    postgres,
};
use serde_json::{Value, json};
use sqlx::Row;

// The digest uses the primary key; the full key guards against digest collisions.
const FEATURE_USAGE_QUERY: &str = "SELECT value::text FROM metering_aggregates WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest=$4 AND group_key=$5";

pub struct FeatureQuery {
    key: String,
    id: String,
    kind: FeatureKind,
    value_type: i32,
    meter_id: String,
    group_key: String,
}

fn invalid_attribute(message: impl Into<String>) -> crate::Fault {
    crate::Fault::new(connectrpc::ErrorCode::InvalidArgument, 9, message)
}
fn append_reason(response: &mut Value, reason: BlockReason) {
    if !response["reasons"].is_array() {
        response["reasons"] = json!([]);
    }
    response["reasons"]
        .as_array_mut()
        .unwrap()
        .push(json!(reason as i32));
}

impl Call<'_> {
    pub async fn feature_request(&mut self, q: &Value) -> Result<Option<FeatureQuery>> {
        let key = string(q, "featureKey");
        if key.is_empty() {
            if q["groupValues"]
                .as_object()
                .is_some_and(|values| !values.is_empty())
            {
                return Err(invalid_attribute("group values require a feature key"));
            }
            return Ok(None);
        }
        if key.len() > 100
            || !key.as_bytes()[0].is_ascii_lowercase()
            || !key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(invalid("invalid feature key"));
        }
        let values = super::events::attributes(&q["groupValues"])?;
        let mut f = FeatureQuery {
            key: key.into(),
            id: String::new(),
            kind: FeatureKind::Unspecified,
            value_type: 0,
            meter_id: String::new(),
            group_key: String::new(),
        };
        let row = sqlx::query("SELECT id::text,kind,value_type,meter_key FROM features WHERE service_id=$1 AND key=$2").bind(uuid(&self.service)?).bind(key).fetch_optional(&mut *self.tx).await?;
        // Catalog deployment can follow a consumer deployment. Unknown keys block execution.
        let Some(row) = row else {
            return Ok(Some(f));
        };
        f.id = row.try_get("id")?;
        f.kind = FeatureKind::from(row.try_get::<i32, _>("kind")?);
        f.value_type = row.try_get("value_type")?;
        if f.kind != FeatureKind::Metered {
            if !values.is_empty() {
                return Err(invalid_attribute("only metered features take group values"));
            }
            return Ok(Some(f));
        }
        let meter_key: String = row.try_get("meter_key")?;
        let row = sqlx::query("SELECT ce.id::text,cv.definition FROM catalog_entities ce LEFT JOIN catalog_versions cv ON cv.id=ce.active_version_id WHERE ce.kind='meter' AND ce.scope=$1 AND ce.key=$2")
            .bind(&self.service).bind(meter_key).fetch_one(&mut *self.tx).await?;
        f.meter_id = row.try_get("id")?;
        let raw: Option<Value> = row.try_get("definition")?;
        let definition = features::decode_meter(raw.as_ref())?;
        f.group_key = features::group_key(&definition.group_by, &values)
            .map_err(|e| invalid_attribute(e.to_string()))?;
        Ok(Some(f))
    }

    pub async fn authorize_feature(
        &mut self,
        f: Option<&FeatureQuery>,
        tenant: &str,
        response: &mut Value,
    ) -> Result<()> {
        let Some(f) = f else {
            return Ok(());
        };
        if f.id.is_empty() {
            append_reason(response, BlockReason::FeatureUnknown);
            tracing::info!(category="feature_authorization",correlation_id=%self.correlation,service_id=%self.service,feature_key=%f.key,reason=?BlockReason::FeatureUnknown,"feature authorization evaluated");
            return Ok(());
        }
        let raws: Vec<Value> = sqlx::query_scalar("SELECT cvf.value FROM tenant_catalog_selections cs JOIN catalog_version_features cvf ON cvf.version_id=cs.version_id WHERE cs.tenant_id=$1 AND cvf.feature_id=$2 AND cs.starts_at<=$3 AND (cs.ends_at IS NULL OR cs.ends_at>$3) ORDER BY cs.starts_at,cs.id")
            .bind(uuid(tenant)?).bind(uuid(&f.id)?).bind(self.now).fetch_all(&mut *self.tx).await?;
        if raws.len() > 1 {
            // Old selections can predate conflict validation. Multiple grants fail closed.
            tracing::warn!(category="feature_authorization",correlation_id=%self.correlation,service_id=%self.service,feature_key=%f.key,grants=raws.len(),"conflicting pinned feature grants");
        }
        let grants = raws
            .iter()
            .map(|raw| features::decode_grant(f.kind, f.value_type, raw))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut result = json!({"featureId":f.id,"kind":f.kind as i32});
        let mut applied = Decimal::zero();
        if grants.len() == 1 && f.kind == FeatureKind::Metered {
            let period = postgres::billing_period(&mut *self.tx, tenant, self.now).await;
            let (start, end) = match period {
                Ok(period) => period,
                Err(error) if error.is::<postgres::PeriodUnavailable>() => {
                    result["entitled"] = json!(true);
                    result["limit"] = json!(grants[0].limit.to_string());
                    let renewal = json!(BlockReason::Renewal as i32);
                    if !response["reasons"].as_array().is_some_and(|v| {
                        v.iter()
                            .any(|v| v == &renewal || v.as_str() == Some("BLOCK_REASON_RENEWAL"))
                    }) {
                        append_reason(response, BlockReason::Renewal);
                    }
                    response["feature"] = result;
                    tracing::info!(category="feature_authorization",correlation_id=%self.correlation,service_id=%self.service,feature_key=%f.key,billing_period="unavailable",reason=?BlockReason::Renewal,"feature authorization evaluated");
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            };
            let text: Option<String> = sqlx::query_scalar(FEATURE_USAGE_QUERY)
                .bind(uuid(tenant)?)
                .bind(uuid(&f.meter_id)?)
                .bind(start)
                .bind(events::group_digest(&f.group_key))
                .bind(&f.group_key)
                .fetch_optional(&mut *self.tx)
                .await?;
            if let Some(text) = text {
                applied = Decimal::parse(&text).map_err(anyhow::Error::from)?;
            }
            result["limit"] = json!(grants[0].limit.to_string());
            result["usage"] = json!(applied.to_string());
            result["periodStart"] = json!(start.to_rfc3339());
            result["periodEnd"] = json!(end.to_rfc3339());
        }
        let decision = features::evaluate(&grants, &applied);
        result["entitled"] = json!(decision.entitled);
        if decision.entitled && f.kind == FeatureKind::Configuration {
            result["configuration"] = grants[0].configuration.clone().unwrap_or(Value::Null);
        }
        if decision.reason != BlockReason::Unspecified {
            append_reason(response, decision.reason);
        }
        response["feature"] = result;
        tracing::info!(category="feature_authorization",correlation_id=%self.correlation,service_id=%self.service,feature_key=%f.key,feature_kind=?f.kind,entitled=decision.entitled,reason=?decision.reason,"feature authorization evaluated");
        Ok(())
    }
}
