// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
use crate::{Op, events, metering};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use std::collections::BTreeMap;
pub fn attributes(input: &Value) -> Result<BTreeMap<String, metering::Value>> {
    if input.is_null() {
        return Ok(BTreeMap::new());
    }
    let input = input.as_object().ok_or_else(|| {
        events::reject(events::Reason::InvalidAttribute, "attributes must be a map")
    })?;
    if input.len() > events::MAX_ATTRIBUTES {
        return Err(events::reject(
            events::Reason::InvalidAttribute,
            "too many attributes",
        ));
    }
    input
        .iter()
        .map(|(name, v)| {
            let value = if let Some(s) = v.get("text").and_then(Value::as_str) {
                metering::Value::string(s)
            } else if let Some(b) = v.get("boolean").and_then(Value::as_bool) {
                metering::Value::boolean(b)
            } else if let Some(s) = v.get("number").and_then(Value::as_str) {
                if s.len() > events::MAX_NUMBER_BYTES {
                    return Err(events::reject(
                        events::Reason::InvalidAttribute,
                        "numeric attributes contain at most 64 characters",
                    ));
                }
                metering::Value::number(s).map_err(|_| {
                    events::reject(
                        events::Reason::InvalidAttribute,
                        "numeric attributes must be decimal strings",
                    )
                })?
            } else {
                return Err(events::reject(
                    events::Reason::InvalidAttribute,
                    "attribute values must be flat scalars",
                ));
            };
            Ok((name.clone(), value))
        })
        .collect()
}
pub fn wire_attributes(input: &BTreeMap<String, metering::Value>) -> Value {
    Value::Object(
        input
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    match v.kind {
                        metering::ValueKind::String => json!({"text":v.text}),
                        metering::ValueKind::Number => json!({"number":v.text}),
                        metering::ValueKind::Boolean => json!({"boolean":v.text=="true"}),
                    },
                )
            })
            .collect(),
    )
}
impl Call<'_> {
    fn event_policy(&self) -> events::Policy {
        events::Policy {
            lateness: Duration::seconds(self.config.usage_lateness_seconds),
            reporting_grace: Duration::seconds(self.config.reporting_grace_seconds),
            limits: metering::Limits {
                max_groups_per_meter: self.config.max_meter_groups,
                max_distinct_values_per_group: self.config.max_meter_group_values,
            },
        }
    }
    pub async fn events(&mut self, op: Op, q: &Value) -> Result<Value> {
        match op {
            Op::IngestEvent => {
                self.service_scope("usage:write").await?;
                let e = &q["event"];
                let occurred_at: DateTime<Utc> = string(e, "occurredAt")
                    .parse()
                    .map_err(|_| invalid("invalid event attribution or occurrence"))?;
                if !string(e, "id").is_empty()
                    || !canonical(string(e, "tenantId"))
                    || string(e, "serviceId") != self.service
                {
                    return Err(invalid("invalid event attribution or occurrence"));
                }
                let input = events::Input {
                    tenant_id: string(e, "tenantId").into(),
                    service_id: self.service.clone(),
                    key: string(e, "eventKey").into(),
                    event_type: string(e, "eventType").into(),
                    occurred_at,
                    attributes: attributes(&e["attributes"])?,
                };
                let policy = self.event_policy();
                let a = events::ingest(self.tx, self.now, &policy, input).await?;
                tracing::Span::current().record("delcore.event.matched_meters", a.meters);
                tracing::info!(category="event_ingestion",correlation_id=%self.correlation,revision=a.revision,replayed=a.replayed,matched_meters=a.meters,"metering event accepted");
                Ok(json!({"eventId":a.event_id,"revision":a.revision,"_replayed":a.replayed}))
            }
            Op::CorrectEvent => {
                self.service_scope("usage:write").await?;
                if !canonical(string(q, "tenantId")) {
                    return Err(invalid("invalid tenant ID"));
                }
                let kind = match q["kind"].as_str() {
                    Some("EVENT_CORRECTION_KIND_REPLACE") => metering::RevisionKind::Replace,
                    Some("EVENT_CORRECTION_KIND_CANCEL") => metering::RevisionKind::Cancel,
                    _ => match q["kind"].as_i64() {
                        Some(1) => metering::RevisionKind::Replace,
                        Some(2) => metering::RevisionKind::Cancel,
                        _ => return Err(invalid("correction kind must be replace or cancel")),
                    },
                };
                let c = events::Correction {
                    tenant_id: string(q, "tenantId").into(),
                    service_id: self.service.clone(),
                    key: string(q, "eventKey").into(),
                    expected_revision: q["expectedRevision"]
                        .as_i64()
                        .unwrap_or(0)
                        .try_into()
                        .map_err(|_| invalid("invalid expected revision"))?,
                    kind,
                    event_type: string(q, "eventType").into(),
                    attributes: attributes(&q["attributes"])?,
                };
                let policy = self.event_policy();
                let a = events::correct(self.tx, self.now, &policy, c).await?;
                if !a.replayed {
                    self.audit(
                        string(q, "tenantId"),
                        "event.correct",
                        &format!(
                            "kind={} revision={} matched_meters={}",
                            kind.as_str(),
                            a.revision,
                            a.meters
                        ),
                    )
                    .await?;
                }
                tracing::Span::current().record("delcore.event.matched_meters", a.meters);
                tracing::info!(category="event_correction",correlation_id=%self.correlation,kind=kind.as_str(),revision=a.revision,replayed=a.replayed,matched_meters=a.meters,"metering correction accepted");
                Ok(json!({"eventId":a.event_id,"revision":a.revision,"_replayed":a.replayed}))
            }
            Op::GetEvent => {
                if self.service_scope("usage:read").await.is_err() {
                    self.service_scope("usage:write").await?;
                }
                if !canonical(string(q, "tenantId")) {
                    return Err(invalid("invalid tenant ID"));
                }
                let s = events::get(
                    self.tx,
                    string(q, "tenantId"),
                    &self.service,
                    string(q, "eventKey"),
                )
                .await?;
                Ok(
                    json!({"event":{"id":s.id,"tenantId":s.tenant_id,"serviceId":s.service_id,"eventKey":s.key,"eventType":s.event_type,"occurredAt":s.occurred_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi,true),"attributes":wire_attributes(&s.attributes)},"revision":s.revision,"appliedRevision":s.applied_revision,"cancelled":s.cancelled}),
                )
            }
            _ => Err(invalid("unsupported event operation")),
        }
    }
}
