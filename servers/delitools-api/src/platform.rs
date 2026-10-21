// SPDX-License-Identifier: FSL-1.1-ALv2
//! Shared service state and credential-to-principal resolution.
//!
//! [`Platform`] owns everything a request handler, background task or execution layer needs:
//! storage, verified configuration, the JWT verifier, the tenant authority, live operator
//! settings, admission control and metrics. The execution layer receives an `Arc<Platform>`.
use crate::{
    admission::Admission,
    auth::{self, Credential, Verifier},
    authority::{Consumer, Decision, ExecutionCheck, ManagementScope, TenantAuthority},
    config::Config,
    error::{Fault, Result},
    keys,
    readiness::{CatalogPriceReadiness, ReadinessCheck, SchemaReadiness},
    settings::Settings,
    store::{self, Store, keys::KeyRow},
    telemetry::{FailureKind, Metrics},
};
use delino_protocol::delitools::v1::{Capability, KeyKind};
use std::sync::Arc;
use tokio::sync::watch;
use uuid::Uuid;

/// How a paid execution was authenticated. Stored on request records for attribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialKind {
    UserToken,
    PersonalKey,
    ServiceKey,
}
impl CredentialKind {
    /// Database value of `requests.credential_kind`.
    pub fn to_db(self) -> i16 {
        match self {
            Self::UserToken => 1,
            Self::PersonalKey => 2,
            Self::ServiceKey => 3,
        }
    }
}

/// The authorized caller of one paid execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionPrincipal {
    pub tenant_id: Uuid,
    pub capability: Capability,
    pub credential: CredentialKind,
    pub api_key_id: Option<Uuid>,
    /// The acting user for user tokens and personal keys; the issuing user for service keys is
    /// attribution only and is not recorded here.
    pub user_id: Option<Uuid>,
}
impl ExecutionPrincipal {
    fn consumer(&self) -> Consumer {
        match self.user_id {
            Some(user) if self.credential != CredentialKind::ServiceKey => Consumer::User(user),
            _ => Consumer::TenantService,
        }
    }
}

/// A verified Logto user acting on key management.
#[derive(Clone, Debug)]
pub struct UserContext {
    pub user_id: Uuid,
}

/// A verified platform operator.
#[derive(Clone, Debug)]
pub struct OperatorContext {
    /// `operator:<subject hash>` for audit records.
    pub actor: String,
}

pub struct Platform {
    pub store: Store,
    pub config: Arc<Config>,
    pub verifier: Verifier,
    pub authority: Arc<dyn TenantAuthority>,
    pub metrics: Arc<Metrics>,
    pub admission: Admission,
    settings: watch::Sender<Arc<Settings>>,
    /// Orders settings commits with their live publication; see [`Platform::settings_update_guard`].
    settings_updates: tokio::sync::Mutex<()>,
    readiness: std::sync::Mutex<Vec<Arc<dyn ReadinessCheck>>>,
}

impl Platform {
    pub fn new(
        store: Store,
        config: Arc<Config>,
        verifier: Verifier,
        authority: Arc<dyn TenantAuthority>,
        settings: Settings,
    ) -> Arc<Self> {
        let (settings, _) = watch::channel(Arc::new(settings));
        Arc::new(Self {
            store,
            config,
            verifier,
            authority,
            metrics: Arc::new(Metrics::new()),
            admission: Admission::new(),
            settings,
            settings_updates: tokio::sync::Mutex::new(()),
            readiness: std::sync::Mutex::new(vec![
                Arc::new(SchemaReadiness),
                Arc::new(CatalogPriceReadiness),
            ]),
        })
    }

    /// Current operator settings snapshot.
    pub fn settings(&self) -> Arc<Settings> {
        self.settings.borrow().clone()
    }
    /// Serialize a settings commit with the publication of its snapshot. PostgreSQL orders the
    /// transactions, but a request resumes after its own commit at an arbitrary time. Without this
    /// guard an earlier transaction could publish after a later one and replace the newest live
    /// settings with a stale snapshot. One process serves one database (instance lock), so a
    /// process-wide guard gives the same order as the stored version.
    pub async fn settings_update_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.settings_updates.lock().await
    }
    /// Publish settings that were just stored. Hold [`Platform::settings_update_guard`] from
    /// before the commit until this call returns.
    pub fn apply_settings(&self, settings: Settings) {
        self.settings.send_replace(Arc::new(settings));
    }
    pub fn subscribe_settings(&self) -> watch::Receiver<Arc<Settings>> {
        self.settings.subscribe()
    }

    /// Register an additional readiness requirement.
    pub fn add_readiness_check(&self, check: Arc<dyn ReadinessCheck>) {
        self.readiness
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(check);
    }
    /// Evaluate every readiness check. Failing check names are logged, not returned.
    pub async fn ready(&self) -> bool {
        let checks: Vec<_> = self
            .readiness
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut ready = true;
        for check in checks {
            if !check.ready(&self.store.pool).await {
                tracing::warn!(check = check.name(), "readiness check failing");
                ready = false;
            }
        }
        ready
    }

    fn authority_failure(&self) -> Fault {
        self.metrics.failure(FailureKind::AuthorityUnavailable);
        tracing::warn!(
            category = "authority_unavailable",
            "tenant authority could not answer; failing closed"
        );
        Fault::dependency_unavailable()
    }

    /// Resolve a verified user bearer token to a platform user.
    async fn resolve_user(&self, token: &str) -> Result<Uuid> {
        let identity = self.verifier.verify(token).await.inspect_err(|_| {
            self.metrics.failure(FailureKind::Authentication);
        })?;
        if identity.machine {
            self.metrics.failure(FailureKind::Authorization);
            return Err(Fault::forbidden());
        }
        match self.authority.resolve_user(token).await {
            Ok(Some(user)) => Ok(user),
            Ok(None) => {
                self.metrics.failure(FailureKind::Authorization);
                Err(Fault::forbidden())
            }
            Err(_) => Err(self.authority_failure()),
        }
    }

    /// Authenticate a Logto user for key management. API keys cannot manage keys.
    pub async fn authenticate_user(&self, authorization: &str) -> Result<UserContext> {
        match auth::parse_credential(authorization).inspect_err(|_| {
            self.metrics.failure(FailureKind::Authentication);
        })? {
            Credential::ApiKey(..) => {
                self.metrics.failure(FailureKind::Authorization);
                Err(Fault::forbidden())
            }
            Credential::Jwt(token) => Ok(UserContext {
                user_id: self.resolve_user(token).await?,
            }),
        }
    }

    /// Authenticate a platform operator from the signed role claim. Tenant Owner/Admin and API
    /// keys never imply platform authority.
    pub async fn authenticate_operator(&self, authorization: &str) -> Result<OperatorContext> {
        match auth::parse_credential(authorization).inspect_err(|_| {
            self.metrics.failure(FailureKind::Authentication);
        })? {
            Credential::ApiKey(..) => {
                self.metrics.failure(FailureKind::Authorization);
                Err(Fault::forbidden())
            }
            Credential::Jwt(token) => {
                let identity = self.verifier.verify(token).await.inspect_err(|_| {
                    self.metrics.failure(FailureKind::Authentication);
                })?;
                if !identity.operator {
                    self.metrics.failure(FailureKind::Authorization);
                    return Err(Fault::forbidden());
                }
                Ok(OperatorContext {
                    actor: format!("operator:{}", identity.subject_hash),
                })
            }
        }
    }

    /// Check a management scope against the current tenant authority.
    pub async fn authorize_management(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        scope: ManagementScope,
    ) -> Result<()> {
        match self
            .authority
            .authorize_management(tenant_id, user_id, scope)
            .await
        {
            Ok(Decision::Allowed) => Ok(()),
            Ok(_) => {
                self.metrics.failure(FailureKind::Authorization);
                Err(Fault::forbidden())
            }
            Err(_) => Err(self.authority_failure()),
        }
    }

    /// Authenticate and authorize one new paid execution for `tenant_id` and `capability`.
    ///
    /// * A Logto user token acts as the user, who must currently have tenant access.
    /// * A personal key additionally requires its capability and acts as its owning user, who must
    ///   still have access, so permissions are the intersection of key capabilities and the user's
    ///   current permissions.
    /// * A service key belongs to the tenant: it requires its capability and tenant-level
    ///   authorization, independent of the issuing member.
    ///
    /// Billing blocks apply to every credential. A check that cannot complete fails closed.
    pub async fn authenticate_execution(
        &self,
        authorization: &str,
        tenant_id: Uuid,
        capability: Capability,
    ) -> Result<ExecutionPrincipal> {
        let credential = auth::parse_credential(authorization).inspect_err(|_| {
            self.metrics.failure(FailureKind::Authentication);
        })?;
        let principal = match credential {
            Credential::Jwt(token) => ExecutionPrincipal {
                tenant_id,
                capability,
                credential: CredentialKind::UserToken,
                api_key_id: None,
                user_id: Some(self.resolve_user(token).await?),
            },
            Credential::ApiKey(id, presented) => {
                let key = self.verified_key(id, &presented).await?;
                if key.tenant_id != tenant_id {
                    // Tenant isolation: a key never acts for another tenant.
                    self.metrics.failure(FailureKind::Authorization);
                    return Err(Fault::forbidden());
                }
                if !key.capabilities.contains(capability) {
                    self.metrics.failure(FailureKind::Authorization);
                    return Err(Fault::forbidden());
                }
                ExecutionPrincipal {
                    tenant_id,
                    capability,
                    credential: if key.kind == KeyKind::KEY_KIND_SERVICE {
                        CredentialKind::ServiceKey
                    } else {
                        CredentialKind::PersonalKey
                    },
                    api_key_id: Some(key.id),
                    user_id: (key.kind == KeyKind::KEY_KIND_PERSONAL).then_some(key.user_id),
                }
            }
        };
        self.authorize_principal(&principal).await?;
        Ok(principal)
    }

    /// Re-check a principal immediately before a duplicate disclosure or an AI attempt. A revoked
    /// or expired key and a removed user permission reject new attempts; calls already running
    /// are not interrupted by this check.
    pub async fn reauthorize(&self, principal: &ExecutionPrincipal) -> Result<()> {
        if let Some(id) = principal.api_key_id {
            let key = store::keys::get(&self.store.pool, id)
                .await?
                .ok_or_else(Fault::unauthenticated)?;
            if !keys::is_usable(key.revoked_at, key.expires_at, store::now()) {
                self.metrics.failure(FailureKind::Authentication);
                return Err(Fault::unauthenticated());
            }
        }
        self.authorize_principal(principal).await
    }

    async fn authorize_principal(&self, principal: &ExecutionPrincipal) -> Result<()> {
        let check = ExecutionCheck {
            tenant_id: principal.tenant_id,
            capability: principal.capability,
            consumer: principal.consumer(),
        };
        match self.authority.authorize_execution(check).await {
            Ok(Decision::Allowed) => Ok(()),
            Ok(Decision::Forbidden) => {
                self.metrics.failure(FailureKind::Authorization);
                Err(Fault::forbidden())
            }
            Ok(Decision::BillingBlocked) => {
                self.metrics.failure(FailureKind::Authorization);
                Err(Fault::billing_blocked())
            }
            Err(_) => Err(self.authority_failure()),
        }
    }

    /// Look up a key and verify its secret in constant time, then its revocation and expiry.
    /// Unknown, mismatching, revoked and expired keys are indistinguishable to the caller.
    async fn verified_key(&self, id: Uuid, presented: &[u8; 32]) -> Result<KeyRow> {
        let row = store::keys::get(&self.store.pool, id).await?;
        // Compare against a dummy hash for unknown keys so timing does not reveal existence.
        let stored: &[u8] = row
            .as_ref()
            .map_or(&[0_u8; 32], |r| r.secret_hash.as_slice());
        let matches = keys::verify(stored, presented);
        match row {
            Some(key)
                if matches && keys::is_usable(key.revoked_at, key.expires_at, store::now()) =>
            {
                Ok(key)
            }
            _ => {
                self.metrics.failure(FailureKind::Authentication);
                Err(Fault::unauthenticated())
            }
        }
    }
}
