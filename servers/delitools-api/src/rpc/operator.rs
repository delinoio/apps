// SPDX-License-Identifier: FSL-1.1-ALv2
//! Platform-operator RPCs: `GetSettings`, `UpdateSettings` and `InspectRequest`.
//!
//! All three require the dedicated Logto platform-operator role from signed claims. Tenant
//! Owner/Admin never implies platform authority. Mutations require a reason and write an audit
//! record in the same transaction. Inspection returns safe state, attempt, price and usage
//! metadata only and is itself audited.
use super::now_checked;
use crate::{
    error::{Fault, Result},
    platform::{OperatorContext, Platform},
    settings::Settings,
    store::{
        self,
        audit::{self, Action, AuditEntry, EventCategory, Severity},
    },
};
use buffa_types::google::protobuf::Timestamp;
use delino_protocol::delitools::v1::*;
use uuid::Uuid;

const MIN_REASON_CHARS: usize = 3;
const MAX_REASON_CHARS: usize = 1000;

/// Validate the mandatory operator reason.
pub fn valid_reason(reason: &str) -> Result<&str> {
    let chars = reason.trim().chars().count();
    if !(MIN_REASON_CHARS..=MAX_REASON_CHARS).contains(&chars) {
        return Err(Fault::invalid_input(format!(
            "reason must contain {MIN_REASON_CHARS} to {MAX_REASON_CHARS} characters"
        )));
    }
    Ok(reason.trim())
}

impl Platform {
    pub async fn get_settings(
        &self,
        request_id: &str,
        authorization: &str,
    ) -> Result<GetSettingsResponse> {
        self.authenticate_operator(authorization).await?;
        Ok(GetSettingsResponse {
            request_id: request_id.to_owned(),
            settings: Some(self.settings().to_proto()).into(),
            ..Default::default()
        })
    }

    pub async fn update_settings(
        &self,
        request_id: &str,
        authorization: &str,
        request: UpdateSettingsRequest,
    ) -> Result<UpdateSettingsResponse> {
        let operator = self.authenticate_operator(authorization).await?;
        let reason = valid_reason(&request.reason)?;
        let wire = request
            .settings
            .as_option()
            .ok_or_else(|| Fault::invalid_input("settings are required"))?;
        let settings =
            Settings::from_proto(wire).map_err(|e| Fault::invalid_input(e.to_string()))?;
        let request_uuid = parse_request_id(request_id);
        let (previous, version) = store::settings::update(
            &self.store.pool,
            &settings,
            AuditEntry {
                action: Action::SettingsUpdate,
                actor: &operator.actor,
                reason,
                request_id: request_uuid,
                tenant_id: None,
                target_id: None,
                details: serde_json::Value::Null,
            },
            now_checked(),
        )
        .await?;
        let changed = previous.changed_fields(&settings);
        self.apply_settings(settings);
        tracing::info!(request_id, version, changed = ?changed, "operator settings updated");
        audit::record_event(
            &self.store.pool,
            EventCategory::SettingsUpdated,
            Severity::Info,
            Some(request_uuid),
            request_uuid,
            serde_json::json!({"version": version, "changed": changed}),
        )
        .await;
        Ok(UpdateSettingsResponse {
            request_id: request_id.to_owned(),
            ..Default::default()
        })
    }

    pub async fn inspect_request(
        &self,
        request_id: &str,
        authorization: &str,
        request: InspectRequestRequest,
    ) -> Result<InspectRequestResponse> {
        let operator = self.authenticate_operator(authorization).await?;
        if !server_common::canonical_id(&request.request_id) {
            return Err(Fault::invalid_input("requestId must be a UUID v7"));
        }
        let target = Uuid::parse_str(&request.request_id).expect("canonical");
        let found = store::inspect::request(&self.store.pool, target)
            .await?
            .ok_or_else(|| Fault::not_found("request not found or already expired"))?;
        self.audit_inspection(&operator, request_id, &found).await?;
        Ok(InspectRequestResponse {
            request_id: request_id.to_owned(),
            inspected_request_id: found.id.to_string(),
            tenant_id: found.tenant_id.to_string(),
            capability: found.capability.into(),
            state: found.state.into(),
            model: found.model,
            created_at: Timestamp {
                seconds: found.created_at.timestamp(),
                nanos: found.created_at.timestamp_subsec_nanos() as i32,
                ..Default::default()
            }
            .into(),
            attempts: found.attempts,
            error_reason: found.error_reason.into(),
            ..Default::default()
        })
    }

    async fn audit_inspection(
        &self,
        operator: &OperatorContext,
        request_id: &str,
        found: &store::inspect::Inspection,
    ) -> Result<()> {
        let mut conn = self.store.pool.acquire().await?;
        audit::insert(
            &mut conn,
            AuditEntry {
                action: Action::RequestInspect,
                actor: &operator.actor,
                reason: "",
                request_id: parse_request_id(request_id),
                tenant_id: Some(found.tenant_id),
                target_id: Some(found.id),
                details: serde_json::json!({}),
            },
            now_checked(),
        )
        .await
    }
}

fn parse_request_id(request_id: &str) -> Uuid {
    Uuid::parse_str(request_id).unwrap_or_else(|_| Uuid::now_v7())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_are_trimmed_and_bounded() {
        assert_eq!(valid_reason("  raise limit  ").unwrap(), "raise limit");
        assert!(valid_reason("").is_err());
        assert!(valid_reason("  ab  ").is_err());
        assert!(valid_reason("   ").is_err());
        assert!(valid_reason(&"x".repeat(1000)).is_ok());
        assert!(valid_reason(&"x".repeat(1001)).is_err());
        assert!(
            valid_reason(&"한".repeat(1000)).is_ok(),
            "bounded by characters"
        );
    }
}
