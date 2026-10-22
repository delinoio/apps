// SPDX-License-Identifier: FSL-1.1-ALv2
//! Authentication and authorization of paid calls.
//!
//! Two credential kinds arrive as `Authorization: Bearer <token>`:
//!
//! * `dlk_...` API keys (personal or tenant service keys): Delcore `AuthorizeApiKey` verifies the
//!   secret, the service/capability grant, the tenant and billing state, and for personal keys the
//!   owner's current permission. Delcore answers `unauthenticated` for any invalid secret, so no
//!   attribution leaks. A tenant in the request must equal the key's tenant.
//! * Any other token is a Logto user token: Delcore `GetCurrentUser` identifies the user (through
//!   `Authority::identify`), the request names the tenant, and Delcore `Authorize` checks
//!   permission and billing with `paid=true`.
//!
//! Both paths use the DeliTools service token and fail closed: a Delcore outage, a rejected
//! service token or a malformed answer is `DEPENDENCY_UNAVAILABLE`, never an allow.
//! [`Authenticator::recheck`] repeats the decision before every retry attempt after the first.

use crate::{
    config::Secret,
    delcore::{Block, DelcoreClient, DelcoreError, Verdict},
    error::ApiError,
};
use connectrpc::ErrorCode;
use server_common::authority::Authority;
use std::sync::Arc;

/// Prefix of Delcore API keys.
pub const KEY_PREFIX: &str = "dlk_";

enum Credential {
    ApiKey(Secret),
    User { user_id: String },
}

/// An authorized caller. Holds the API key secret in memory only, for the re-checks.
pub struct Principal {
    pub tenant_id: String,
    credential: Credential,
    capability: &'static str,
}

impl std::fmt::Debug for Principal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Principal")
            .field("tenant_id", &self.tenant_id)
            .field("capability", &self.capability)
            .finish_non_exhaustive()
    }
}

impl Principal {
    /// Credential kind label for logs.
    pub fn kind(&self) -> &'static str {
        match self.credential {
            Credential::ApiKey(_) => "api_key",
            Credential::User { .. } => "user",
        }
    }
}

pub struct Authenticator {
    pub authority: Authority,
    pub delcore: Arc<DelcoreClient>,
}

/// Extracts the bearer token from an `Authorization` header value.
pub fn bearer(header: &str) -> Option<&str> {
    let mut parts = header.split_whitespace();
    let (scheme, token) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && scheme.eq_ignore_ascii_case("Bearer") && !token.is_empty())
        .then_some(token)
}

fn dependency(error: DelcoreError) -> ApiError {
    tracing::error!(
        category = "delcore_authorization",
        failure = match error {
            DelcoreError::Unauthenticated => "service_unauthenticated",
            DelcoreError::PermissionDenied => "service_forbidden",
            DelcoreError::Unavailable => "unavailable",
        },
        "the Delcore authorization check failed; failing closed"
    );
    ApiError::dependency_unavailable()
}

fn from_verdict(verdict: &Verdict) -> Result<(), ApiError> {
    if verdict.allowed {
        Ok(())
    } else if verdict.reasons.contains(&Block::Permission) {
        Err(ApiError::permission_denied())
    } else {
        Err(ApiError::billing_blocked())
    }
}

impl Authenticator {
    /// Identifies the caller and authorizes paid `capability` for the tenant.
    pub async fn authenticate(
        &self,
        authorization: &str,
        request_tenant: &str,
        capability: &'static str,
    ) -> Result<Principal, ApiError> {
        let token = bearer(authorization).ok_or_else(ApiError::unauthenticated)?;
        if token.starts_with(KEY_PREFIX) {
            let verdict = self
                .delcore
                .authorize_api_key(token, capability)
                .await
                .map_err(|e| match e {
                    DelcoreError::Unauthenticated => ApiError::unauthenticated(),
                    other => dependency(other),
                })?;
            if !request_tenant.is_empty() && request_tenant != verdict.tenant_id {
                return Err(ApiError::permission_denied());
            }
            from_verdict(&verdict)?;
            return Ok(Principal {
                tenant_id: verdict.tenant_id,
                credential: Credential::ApiKey(Secret::new(token)),
                capability,
            });
        }
        let user_id = self
            .authority
            .identify(&format!("Bearer {token}"))
            .await
            .map_err(|fault| match fault.code {
                ErrorCode::Unauthenticated => ApiError::unauthenticated(),
                ErrorCode::PermissionDenied => ApiError::permission_denied(),
                _ => {
                    tracing::error!(
                        category = "delcore_identify",
                        "the user token could not be verified; failing closed"
                    );
                    ApiError::dependency_unavailable()
                }
            })?;
        if !server_common::canonical_id(request_tenant) {
            return Err(ApiError::invalid_input(
                "tenant_id must be a canonical UUID v7",
            ));
        }
        let verdict = self
            .delcore
            .authorize(request_tenant, &user_id, capability)
            .await
            .map_err(dependency)?;
        from_verdict(&verdict)?;
        Ok(Principal {
            tenant_id: request_tenant.to_owned(),
            credential: Credential::User { user_id },
            capability,
        })
    }

    /// Repeats the authorization decision. Revocation, expiry, permission loss or a billing block
    /// since the previous check stop further provider attempts.
    pub async fn recheck(&self, principal: &Principal) -> Result<(), ApiError> {
        match &principal.credential {
            Credential::ApiKey(secret) => {
                let verdict = self
                    .delcore
                    .authorize_api_key(secret.expose(), principal.capability)
                    .await
                    .map_err(|e| match e {
                        DelcoreError::Unauthenticated => ApiError::unauthenticated(),
                        other => dependency(other),
                    })?;
                if verdict.tenant_id != principal.tenant_id {
                    return Err(ApiError::permission_denied());
                }
                from_verdict(&verdict)
            }
            Credential::User { user_id } => {
                let verdict = self
                    .delcore
                    .authorize(&principal.tenant_id, user_id, principal.capability)
                    .await
                    .map_err(dependency)?;
                from_verdict(&verdict)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_parsing_is_strict() {
        assert_eq!(bearer("Bearer abc"), Some("abc"));
        assert_eq!(bearer("bearer   abc"), Some("abc"));
        assert_eq!(bearer("BEARER dlk_x"), Some("dlk_x"));
        for bad in ["", "Bearer", "Basic abc", "Bearer a b", "abc", "Bearer  "] {
            assert_eq!(bearer(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn verdicts_map_to_public_errors() {
        use delino_protocol::delitools::v1::ErrorReason as R;
        let allowed = Verdict {
            allowed: true,
            reasons: vec![],
            tenant_id: String::new(),
        };
        assert!(from_verdict(&allowed).is_ok());
        let permission = Verdict {
            allowed: false,
            reasons: vec![Block::Permission],
            tenant_id: String::new(),
        };
        assert_eq!(
            from_verdict(&permission).unwrap_err().reason,
            R::ERROR_REASON_PERMISSION_DENIED
        );
        let billing = Verdict {
            allowed: false,
            reasons: vec![Block::Other],
            tenant_id: String::new(),
        };
        assert_eq!(
            from_verdict(&billing).unwrap_err().reason,
            R::ERROR_REASON_BILLING_BLOCKED
        );
        let mixed = Verdict {
            allowed: false,
            reasons: vec![Block::Other, Block::Permission],
            tenant_id: String::new(),
        };
        assert_eq!(
            from_verdict(&mixed).unwrap_err().reason,
            R::ERROR_REASON_PERMISSION_DENIED
        );
    }

    #[test]
    fn principal_debug_never_prints_the_secret() {
        let principal = Principal {
            tenant_id: "t".into(),
            credential: Credential::ApiKey(Secret::new("dlk_super_secret")),
            capability: "translate",
        };
        assert!(!format!("{principal:?}").contains("super_secret"));
    }
}
