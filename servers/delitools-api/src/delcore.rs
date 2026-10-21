// SPDX-License-Identifier: FSL-1.1-ALv2
//! Delcore integration: the production [`TenantAuthority`] and the usage reporter.
//!
//! Both use the service's Logto machine-to-machine identity. Authorization calls use the
//! `authorize` scope through `server-common`'s [`Authority`]; usage reports need the registered
//! `usage:write` scope and use their own cached token. Every failure to reach or understand
//! delcore is an [`AuthorityError`] so callers fail closed. Request bodies and delcore messages
//! are never logged.
use crate::{
    authority::{
        AuthorityError, Consumer, Decision, ExecutionCheck, ManagementScope, TenantAuthority,
    },
    pricing,
    store::exec::DueAttempt,
};
use async_trait::async_trait;
use delino_protocol::delitools::v1::Capability;
use serde_json::{Value, json};
use server_common::authority::Authority;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use uuid::Uuid;

/// Delcore capability names registered for the `delitools` service.
fn capability_name(capability: Capability) -> &'static str {
    match capability {
        Capability::CAPABILITY_EXTRACTOR => "extractor",
        _ => "translator",
    }
}

const MODE_TENANT_SERVICE_KEY: &str = "CONSUMER_AUTHORIZATION_TENANT_SERVICE_KEY";
const MODE_PERSONAL_KEY_MANAGEMENT: &str = "CONSUMER_AUTHORIZATION_PERSONAL_KEY_MANAGEMENT";
const MODE_SERVICE_KEY_MANAGEMENT: &str = "CONSUMER_AUTHORIZATION_SERVICE_KEY_MANAGEMENT";
const BLOCK_REASON_PERMISSION: &str = "BLOCK_REASON_PERMISSION";

/// Request body of delcore `Authorize` for an execution check.
pub fn execution_body(check: &ExecutionCheck) -> Value {
    let mut body = json!({
        "tenantId": check.tenant_id.to_string(),
        "capability": capability_name(check.capability),
        "paid": true,
    });
    match check.consumer {
        // A service key acts for the tenant: tenant-service mode with an empty user.
        Consumer::TenantService => {
            body["consumerAuthorization"] = json!(MODE_TENANT_SERVICE_KEY);
        }
        // Personal keys and user tokens use the unchanged user check with the key owner or user.
        Consumer::User(user) => {
            body["userId"] = json!(user.to_string());
        }
    }
    body
}

/// Request body of delcore `Authorize` for key management.
pub fn management_body(tenant_id: Uuid, user_id: Uuid, scope: ManagementScope) -> Value {
    json!({
        "tenantId": tenant_id.to_string(),
        "userId": user_id.to_string(),
        "paid": false,
        "consumerAuthorization": match scope {
            ManagementScope::PersonalKeys => MODE_PERSONAL_KEY_MANAGEMENT,
            ManagementScope::ServiceKeys => MODE_SERVICE_KEY_MANAGEMENT,
        },
    })
}

/// Interpret an `AuthorizeResponse`. A denied permission is forbidden; any other block reason
/// (operator block, renewal, dispute, balance, deletion) is a billing block.
pub fn decision(response: &Value) -> Result<Decision, AuthorityError> {
    match response["allowed"].as_bool() {
        Some(true) => Ok(Decision::Allowed),
        Some(false) => {
            let reasons = response["reasons"].as_array().cloned().unwrap_or_default();
            let permission = reasons.is_empty()
                || reasons
                    .iter()
                    .any(|r| r.as_str() == Some(BLOCK_REASON_PERMISSION) || r.as_i64() == Some(1));
            Ok(if permission {
                Decision::Forbidden
            } else {
                Decision::BillingBlocked
            })
        }
        None => Err(AuthorityError),
    }
}

/// Cached machine-to-machine token for one scope.
struct ScopedToken {
    scope: &'static str,
    cache: Mutex<Option<(String, Instant)>>,
}

pub struct DelcoreAuthority {
    authority: Arc<Authority>,
    reporting: ScopedToken,
    token_url: String,
    resource: String,
    client_id: String,
    secret: String,
}

impl DelcoreAuthority {
    pub fn new(config: &crate::config::Config, timeout: Duration) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let authority = Authority::new(
            http,
            config.delcore_url.clone(),
            config.logto_token_url.clone(),
            config.delcore_resource.clone(),
            config.client_id.clone(),
            config.client_secret.clone(),
        );
        Ok(Self {
            authority: Arc::new(authority),
            reporting: ScopedToken {
                scope: "usage:write",
                cache: Mutex::new(None),
            },
            token_url: config.logto_token_url.clone(),
            resource: config.delcore_resource.clone(),
            client_id: config.client_id.clone(),
            secret: config.client_secret.clone(),
        })
    }

    async fn authorize(&self, body: Value) -> Result<Decision, AuthorityError> {
        let token = self
            .authority
            .service_token()
            .await
            .map_err(|_| AuthorityError)?;
        let response = self
            .authority
            .call("Authorize", &format!("Bearer {token}"), body)
            .await
            .map_err(|_| AuthorityError)?;
        decision(&response)
    }

    /// Token for the reporting scope, cached until shortly before expiry.
    async fn reporting_token(&self) -> Result<String, DeliveryError> {
        let mut cache = self.reporting.cache.lock().await;
        if let Some((token, expiry)) = &*cache
            && Instant::now() < *expiry
        {
            return Ok(token.clone());
        }
        let response = self
            .authority
            .http
            .post(&self.token_url)
            .basic_auth(&self.client_id, Some(&self.secret))
            .form(&[
                ("grant_type", "client_credentials"),
                ("resource", self.resource.as_str()),
                ("scope", self.reporting.scope),
            ])
            .send()
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        if !response.status().is_success() {
            return Err(DeliveryError::Unavailable);
        }
        let body: Value = server_common::authority::bounded_json(response, 64 << 10)
            .await
            .map_err(|_| DeliveryError::Unavailable)?;
        let token = body["access_token"].as_str().unwrap_or_default();
        if token.is_empty() {
            return Err(DeliveryError::Unavailable);
        }
        let expires = body["expires_in"].as_u64().unwrap_or(0);
        if expires > 60
            && let Some(at) = Instant::now().checked_add(Duration::from_secs(expires - 30))
        {
            *cache = Some((token.to_owned(), at));
        }
        Ok(token.to_owned())
    }

    /// Report one attempt's measured usage with `IngestEvent`. The attempt UUID is both the
    /// request key and the event key, so a repeated delivery is answered from delcore's retained
    /// identity and never charges twice.
    pub async fn report(
        &self,
        service_id: Uuid,
        attempt: &DueAttempt,
    ) -> Result<(), DeliveryError> {
        let tokens = attempt
            .measured_usage
            .as_ref()
            .and_then(pricing::tokens_from_json)
            .ok_or(DeliveryError::UsageUnknown)?;
        let attributes = pricing::event_attributes(
            &attempt.model,
            attempt.price_effective_at,
            &tokens,
            &attempt.catalog_price,
        )
        .map_err(|_| DeliveryError::MissingCategoryPrice)?;
        let body = json!({
            "requestKey": attempt.id.to_string(),
            "event": {
                "tenantId": attempt.tenant_id.to_string(),
                "serviceId": service_id.to_string(),
                "eventKey": attempt.id.to_string(),
                "eventType": pricing::event_type(attempt.capability),
                "occurredAt": pricing::rfc3339(attempt.occurred_at),
                "attributes": Value::Object(attributes),
            }
        });
        let token = self.reporting_token().await?;
        self.authority
            .call("IngestEvent", &format!("Bearer {token}"), body)
            .await
            .map(|_| ())
            .map_err(|_| DeliveryError::Rejected)
    }
}

/// Why a usage report did not reach delcore. Every variant is retried until the record expires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryError {
    /// The token or transport failed.
    Unavailable,
    /// Delcore answered with an error.
    Rejected,
    /// The attempt carries no confirmed usage.
    UsageUnknown,
    /// Measured usage needs a category the frozen price does not publish.
    MissingCategoryPrice,
}
impl DeliveryError {
    /// Bounded log and metric category.
    pub fn category(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Rejected => "rejected",
            Self::UsageUnknown => "usage_unknown",
            Self::MissingCategoryPrice => "missing_category_price",
        }
    }
}

#[async_trait]
impl TenantAuthority for DelcoreAuthority {
    async fn resolve_user(&self, bearer: &str) -> Result<Option<Uuid>, AuthorityError> {
        match self.authority.identify(&format!("Bearer {bearer}")).await {
            Ok(user) => Uuid::parse_str(&user).map(Some).map_err(|_| AuthorityError),
            Err(fault) => match fault.code {
                // Delcore does not accept the token or does not know the user: not an error of
                // the authority, the caller simply is not a known platform user.
                connectrpc::ErrorCode::Unauthenticated
                | connectrpc::ErrorCode::PermissionDenied => Ok(None),
                _ => Err(AuthorityError),
            },
        }
    }

    async fn authorize_execution(&self, check: ExecutionCheck) -> Result<Decision, AuthorityError> {
        self.authorize(execution_body(&check)).await
    }

    async fn authorize_management(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        scope: ManagementScope,
    ) -> Result<Decision, AuthorityError> {
        self.authorize(management_body(tenant_id, user_id, scope))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_keys_use_tenant_service_mode_without_a_user() {
        let tenant = Uuid::now_v7();
        let body = execution_body(&ExecutionCheck {
            tenant_id: tenant,
            capability: Capability::CAPABILITY_EXTRACTOR,
            consumer: Consumer::TenantService,
        });
        assert_eq!(body["consumerAuthorization"], MODE_TENANT_SERVICE_KEY);
        assert_eq!(body["capability"], "extractor");
        assert_eq!(body["paid"], true);
        assert!(body.get("userId").is_none());
        assert_eq!(body["tenantId"], tenant.to_string());
    }

    #[test]
    fn personal_keys_and_user_tokens_use_the_unspecified_mode_with_the_user() {
        let (tenant, user) = (Uuid::now_v7(), Uuid::now_v7());
        let body = execution_body(&ExecutionCheck {
            tenant_id: tenant,
            capability: Capability::CAPABILITY_TRANSLATOR,
            consumer: Consumer::User(user),
        });
        assert!(body.get("consumerAuthorization").is_none());
        assert_eq!(body["userId"], user.to_string());
        assert_eq!(body["capability"], "translator");
        assert_eq!(body["paid"], true);
    }

    #[test]
    fn management_checks_are_unpaid_and_capability_free() {
        let (tenant, user) = (Uuid::now_v7(), Uuid::now_v7());
        let personal = management_body(tenant, user, ManagementScope::PersonalKeys);
        let service = management_body(tenant, user, ManagementScope::ServiceKeys);
        assert_eq!(
            personal["consumerAuthorization"],
            MODE_PERSONAL_KEY_MANAGEMENT
        );
        assert_eq!(
            service["consumerAuthorization"],
            MODE_SERVICE_KEY_MANAGEMENT
        );
        for body in [&personal, &service] {
            assert_eq!(body["paid"], false);
            assert!(body.get("capability").is_none());
            assert_eq!(body["userId"], user.to_string());
        }
    }

    #[test]
    fn decisions_separate_permission_from_billing_blocks() {
        assert_eq!(decision(&json!({"allowed": true})), Ok(Decision::Allowed));
        assert_eq!(
            decision(&json!({"allowed": false, "reasons": ["BLOCK_REASON_PERMISSION"]})),
            Ok(Decision::Forbidden)
        );
        for reason in [
            "BLOCK_REASON_OPERATOR",
            "BLOCK_REASON_RENEWAL",
            "BLOCK_REASON_DISPUTE",
            "BLOCK_REASON_BALANCE",
            "BLOCK_REASON_DELETING",
        ] {
            assert_eq!(
                decision(&json!({"allowed": false, "reasons": [reason]})),
                Ok(Decision::BillingBlocked),
                "{reason}"
            );
        }
        // Permission wins over a simultaneous billing block, and an unexplained denial is
        // treated as a permission denial.
        assert_eq!(
            decision(&json!({"allowed": false,
                "reasons": ["BLOCK_REASON_BALANCE", "BLOCK_REASON_PERMISSION"]})),
            Ok(Decision::Forbidden)
        );
        assert_eq!(
            decision(&json!({"allowed": false})),
            Ok(Decision::Forbidden)
        );
        // An answer without a decision is not an answer.
        assert_eq!(decision(&json!({})), Err(AuthorityError));
        assert_eq!(decision(&json!({"allowed": "yes"})), Err(AuthorityError));
    }
}
