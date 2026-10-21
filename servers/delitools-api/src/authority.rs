// SPDX-License-Identifier: FSL-1.1-ALv2
//! Current tenant authorization and billing checks.
//!
//! DeliTools never caches tenant membership, roles or billing state: every key-management call and
//! every new paid execution asks the [`TenantAuthority`]. The production adapter, backed by the
//! delcore service identity, is delivered with the execution pipeline once delcore exposes the
//! consumer authorization modes it needs. This module defines the contract and an in-memory
//! implementation for tests and local fixtures.
use async_trait::async_trait;
use delino_protocol::delitools::v1::Capability;
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};
use uuid::Uuid;

/// Tenant membership role, mirroring the delcore roles that DeliTools distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TenantRole {
    Owner,
    Admin,
    Billing,
    Member,
}
impl TenantRole {
    /// Owner and Admin can manage tenant service keys.
    pub fn can_manage_service_keys(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

/// Who the paid work is performed for. This selects the authorization mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consumer {
    /// A tenant service key acts for the tenant independently of the issuing member.
    TenantService,
    /// A Logto user token or a personal key acts for a user, who must currently have access.
    User(Uuid),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionCheck {
    pub tenant_id: Uuid,
    pub capability: Capability,
    pub consumer: Consumer,
}

/// Answer of a completed check. A check that cannot complete is an [`AuthorityError`] instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    /// The tenant, user or capability is not currently authorized.
    Forbidden,
    /// Authorization holds but billing blocks new paid execution.
    BillingBlocked,
}

/// Which key population a management call touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagementScope {
    /// Personal keys require current tenant membership of the acting user.
    PersonalKeys,
    /// Service keys require the Owner or Admin role.
    ServiceKeys,
}

/// The authority could not answer. Callers fail closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorityError;
impl std::fmt::Display for AuthorityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tenant authority unavailable")
    }
}
impl std::error::Error for AuthorityError {}

#[async_trait]
pub trait TenantAuthority: Send + Sync {
    /// Resolve the platform user for a Logto user bearer token whose signature, issuer, audience
    /// and expiry were already verified locally. Returns `None` for a user the platform does not
    /// know, which is not an error.
    async fn resolve_user(&self, bearer: &str) -> Result<Option<Uuid>, AuthorityError>;

    /// Current tenant authorization and billing check before new paid work.
    async fn authorize_execution(&self, check: ExecutionCheck) -> Result<Decision, AuthorityError>;

    /// Current authorization to manage keys of the given scope in a tenant.
    async fn authorize_management(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        scope: ManagementScope,
    ) -> Result<Decision, AuthorityError>;
}

/// Answers every question with [`AuthorityError`], so callers fail closed. The binary uses it
/// until the delcore-backed adapter is installed.
pub struct FailClosedAuthority;

#[async_trait]
impl TenantAuthority for FailClosedAuthority {
    async fn resolve_user(&self, _bearer: &str) -> Result<Option<Uuid>, AuthorityError> {
        Err(AuthorityError)
    }
    async fn authorize_execution(
        &self,
        _check: ExecutionCheck,
    ) -> Result<Decision, AuthorityError> {
        Err(AuthorityError)
    }
    async fn authorize_management(
        &self,
        _tenant_id: Uuid,
        _user_id: Uuid,
        _scope: ManagementScope,
    ) -> Result<Decision, AuthorityError> {
        Err(AuthorityError)
    }
}

#[derive(Default)]
struct State {
    users: HashMap<String, Uuid>,
    roles: HashMap<(Uuid, Uuid), TenantRole>,
    tenants: HashSet<Uuid>,
    billing_blocked: HashSet<Uuid>,
    denied: HashSet<(Uuid, i32)>,
    unavailable: bool,
    execution_checks: u64,
}

/// In-memory [`TenantAuthority`]. Intended for tests and local fixtures only; state is lost on
/// restart and nothing here talks to delcore.
#[derive(Default)]
pub struct InMemoryAuthority {
    state: Mutex<State>,
}
impl InMemoryAuthority {
    pub fn new() -> Self {
        Self::default()
    }
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Register a user bearer token (the full token string) for a platform user.
    pub fn add_user(&self, bearer: &str, user_id: Uuid) {
        self.state().users.insert(bearer.to_owned(), user_id);
    }
    pub fn set_role(&self, tenant_id: Uuid, user_id: Uuid, role: TenantRole) {
        let mut state = self.state();
        state.tenants.insert(tenant_id);
        state.roles.insert((tenant_id, user_id), role);
    }
    pub fn remove_member(&self, tenant_id: Uuid, user_id: Uuid) {
        self.state().roles.remove(&(tenant_id, user_id));
    }
    pub fn add_tenant(&self, tenant_id: Uuid) {
        self.state().tenants.insert(tenant_id);
    }
    pub fn set_billing_blocked(&self, tenant_id: Uuid, blocked: bool) {
        let mut state = self.state();
        if blocked {
            state.billing_blocked.insert(tenant_id);
        } else {
            state.billing_blocked.remove(&tenant_id);
        }
    }
    /// Deny one capability for every consumer of a tenant (for example a plan without the feature).
    pub fn deny_capability(&self, tenant_id: Uuid, capability: Capability) {
        self.state().denied.insert((tenant_id, capability as i32));
    }
    pub fn set_unavailable(&self, unavailable: bool) {
        self.state().unavailable = unavailable;
    }
    pub fn execution_checks(&self) -> u64 {
        self.state().execution_checks
    }
}

#[async_trait]
impl TenantAuthority for InMemoryAuthority {
    async fn resolve_user(&self, bearer: &str) -> Result<Option<Uuid>, AuthorityError> {
        let state = self.state();
        if state.unavailable {
            return Err(AuthorityError);
        }
        Ok(state.users.get(bearer).copied())
    }

    async fn authorize_execution(&self, check: ExecutionCheck) -> Result<Decision, AuthorityError> {
        let mut state = self.state();
        state.execution_checks += 1;
        if state.unavailable {
            return Err(AuthorityError);
        }
        if !state.tenants.contains(&check.tenant_id)
            || state
                .denied
                .contains(&(check.tenant_id, check.capability as i32))
        {
            return Ok(Decision::Forbidden);
        }
        if let Consumer::User(user) = check.consumer
            && !state.roles.contains_key(&(check.tenant_id, user))
        {
            return Ok(Decision::Forbidden);
        }
        if state.billing_blocked.contains(&check.tenant_id) {
            return Ok(Decision::BillingBlocked);
        }
        Ok(Decision::Allowed)
    }

    async fn authorize_management(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        scope: ManagementScope,
    ) -> Result<Decision, AuthorityError> {
        let state = self.state();
        if state.unavailable {
            return Err(AuthorityError);
        }
        Ok(match (state.roles.get(&(tenant_id, user_id)), scope) {
            (None, _) => Decision::Forbidden,
            (Some(_), ManagementScope::PersonalKeys) => Decision::Allowed,
            (Some(role), ManagementScope::ServiceKeys) if role.can_manage_service_keys() => {
                Decision::Allowed
            }
            (Some(_), ManagementScope::ServiceKeys) => Decision::Forbidden,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (Uuid, Uuid, Uuid) {
        (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7())
    }
    fn check(tenant: Uuid, consumer: Consumer) -> ExecutionCheck {
        ExecutionCheck {
            tenant_id: tenant,
            capability: Capability::CAPABILITY_TRANSLATOR,
            consumer,
        }
    }

    #[tokio::test]
    async fn service_consumers_are_tenant_level_and_user_consumers_need_membership() {
        let (tenant, user, outsider) = ids();
        let authority = InMemoryAuthority::new();
        authority.add_tenant(tenant);
        assert_eq!(
            authority
                .authorize_execution(check(tenant, Consumer::TenantService))
                .await,
            Ok(Decision::Allowed),
            "a service key does not depend on any member"
        );
        assert_eq!(
            authority
                .authorize_execution(check(tenant, Consumer::User(user)))
                .await,
            Ok(Decision::Forbidden)
        );
        authority.set_role(tenant, user, TenantRole::Member);
        assert_eq!(
            authority
                .authorize_execution(check(tenant, Consumer::User(user)))
                .await,
            Ok(Decision::Allowed)
        );
        authority.remove_member(tenant, user);
        assert_eq!(
            authority
                .authorize_execution(check(tenant, Consumer::User(user)))
                .await,
            Ok(Decision::Forbidden),
            "removing access removes personal permissions at once"
        );
        assert_eq!(
            authority
                .authorize_execution(check(tenant, Consumer::User(outsider)))
                .await,
            Ok(Decision::Forbidden)
        );
        assert_eq!(
            authority
                .authorize_execution(check(Uuid::now_v7(), Consumer::TenantService))
                .await,
            Ok(Decision::Forbidden),
            "unknown tenant"
        );
    }

    #[tokio::test]
    async fn billing_and_capability_checks_apply_to_both_consumer_modes() {
        let (tenant, user, _) = ids();
        let authority = InMemoryAuthority::new();
        authority.set_role(tenant, user, TenantRole::Member);
        authority.set_billing_blocked(tenant, true);
        for consumer in [Consumer::TenantService, Consumer::User(user)] {
            assert_eq!(
                authority.authorize_execution(check(tenant, consumer)).await,
                Ok(Decision::BillingBlocked)
            );
        }
        authority.set_billing_blocked(tenant, false);
        authority.deny_capability(tenant, Capability::CAPABILITY_TRANSLATOR);
        for consumer in [Consumer::TenantService, Consumer::User(user)] {
            assert_eq!(
                authority.authorize_execution(check(tenant, consumer)).await,
                Ok(Decision::Forbidden)
            );
        }
        assert_eq!(authority.execution_checks(), 4);
    }

    #[tokio::test]
    async fn management_rules_follow_the_key_population() {
        let (tenant, owner, member) = ids();
        let billing = Uuid::now_v7();
        let authority = InMemoryAuthority::new();
        authority.set_role(tenant, owner, TenantRole::Owner);
        authority.set_role(tenant, member, TenantRole::Member);
        authority.set_role(tenant, billing, TenantRole::Billing);
        let admin = Uuid::now_v7();
        authority.set_role(tenant, admin, TenantRole::Admin);
        for (user, scope, expected) in [
            (owner, ManagementScope::ServiceKeys, Decision::Allowed),
            (admin, ManagementScope::ServiceKeys, Decision::Allowed),
            (member, ManagementScope::ServiceKeys, Decision::Forbidden),
            (billing, ManagementScope::ServiceKeys, Decision::Forbidden),
            (member, ManagementScope::PersonalKeys, Decision::Allowed),
            (billing, ManagementScope::PersonalKeys, Decision::Allowed),
            (
                Uuid::now_v7(),
                ManagementScope::PersonalKeys,
                Decision::Forbidden,
            ),
        ] {
            assert_eq!(
                authority.authorize_management(tenant, user, scope).await,
                Ok(expected)
            );
        }
        authority.set_unavailable(true);
        assert!(
            authority
                .authorize_management(tenant, owner, ManagementScope::ServiceKeys)
                .await
                .is_err()
        );
        assert!(authority.resolve_user("token").await.is_err());
    }
}
