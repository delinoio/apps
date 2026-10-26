// SPDX-License-Identifier: Apache-2.0
use crate::network::SendRequest;
use crate::{
    auth::{Auth, endpoint},
    error::{Failure, Result, Violation, invalid},
    protocol::{self, strv},
    store::Store,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
pub fn call(
    auth: &Auth,
    s: &Store,
    override_endpoint: &str,
    method: &str,
    q: &Value,
) -> Result<Value> {
    let c = auth.token(s)?;
    let approved = endpoint(strv(&c, "endpoint"))?;
    if !override_endpoint.is_empty() && override_endpoint != approved {
        return Err(Failure::new(
            "FORBIDDEN",
            "Endpoint differs from the approved connection. Reconnect explicitly.",
        ));
    }
    with_credential(auth, &c, method, q)
}
pub fn with_credential(auth: &Auth, c: &Value, method: &str, q: &Value) -> Result<Value> {
    let m = protocol::method(method).ok_or_else(|| invalid("Unknown typed method."))?;
    let request = protocol::request(method, q)?;
    let response = auth
        .http
        .post(format!(
            "{}/{}/{}",
            endpoint(strv(c, "endpoint"))?,
            m.parent_service().full_name(),
            method
        ))
        .header("Connect-Protocol-Version", "1")
        .bearer_auth(strv(c, "access_token"))
        .json(&request)
        .send_cancellable(auth)
        .map_err(|_| Failure::new("DEPENDENCY_UNAVAILABLE", "Lane service is unavailable."))?;
    let status = response.status();
    let bytes = response.body;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Failure::new("DEPENDENCY_UNAVAILABLE", "Invalid Lane service response."))?;
    if !status.is_success() {
        let reason = match strv(&value, "code") {
            "invalid_argument" => "INVALID_INPUT",
            "unauthenticated" => "UNAUTHENTICATED",
            "permission_denied" => "FORBIDDEN",
            "not_found" => "NOT_FOUND",
            _ => "DEPENDENCY_UNAVAILABLE",
        };
        let mut error = Failure::new(reason, &format!("{}.", reason.replace('_', " ")));
        if let Some(details) = value["details"].as_array() {
            for detail in details {
                let name = strv(detail, "type");
                if let (Some(desc), Ok(raw)) = (
                    protocol::pool().get_message_by_name(name),
                    STANDARD.decode(strv(detail, "value")),
                ) && let Ok(msg) = prost_reflect::DynamicMessage::decode(desc, raw.as_slice())
                    && let Ok(v) = serde_json::to_value(msg)
                {
                    if name == "lane.v1.ErrorDetail" {
                        if let Some(reason) = v["reason"].as_str() {
                            error.reason = reason.into();
                            error.message = format!(
                                "{}.",
                                reason.trim_start_matches("ERROR_REASON_").replace('_', " ")
                            )
                            .into();
                        }
                        if !strv(&v, "correlationId").is_empty() {
                            error.correlation_id = strv(&v, "correlationId").into();
                        }
                        error.entity_id = strv(&v, "entityId").into();
                        error.violation = v["violation"].as_str().and_then(Violation::from_name);
                        error.retryable = v["retryable"] == true;
                        error.current_head_ids =
                            v["currentHeadIds"].as_array().map_or_default(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_owned)
                                    .collect()
                            });
                    } else if name == "lane.v1.EntityResult" {
                        error.conflict = Some(Box::new(v));
                    }
                }
            }
        }
        return Err(error);
    }
    protocol::typed(m.output(), &value, true)
}
