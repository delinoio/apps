// SPDX-License-Identifier: FSL-1.1-ALv2
//! Per-request admission, authentication, rate limiting, logging and error mapping.
use crate::{
    ApiError, CORRELATION_HEADER, MAX_BODY_BYTES, Result, Server,
    auth::{Bearer, bearer},
    rate,
};
use buffa::Enumeration;
use chrono::{DateTime, Utc};
use connectrpc::{RequestContext, Response, ServiceResult};
use delino_protocol::anydash::v1::{
    ActivityKind, ClientKind, DeviceCredentialState, ErrorReason, RateLimitScope,
};
use std::{sync::Mutex, time::Instant};
use uuid::Uuid;

/// Device, owner, credential state, deletion time, access expiry and personal tenant.
type CredentialRow = (
    Uuid,
    Uuid,
    i16,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
    Option<Uuid>,
);

pub const CLIENT_KIND_HEADER: &str = "x-anydash-client-kind";
pub const CLIENT_VERSION_HEADER: &str = "x-anydash-client-version";

/// Request facts recorded by the HTTP policy before the Connect handler runs.
#[derive(Clone, Debug)]
pub struct RequestMeta {
    pub correlation: String,
    pub client_ip: String,
    pub body_bytes: usize,
}

/// Audit context collected while a request runs, used for permission-denial events.
#[derive(Clone, Debug, Default)]
pub struct Actor {
    pub user: Option<Uuid>,
    pub device: Option<Uuid>,
    pub tenant: Option<Uuid>,
    pub dashboard: Option<Uuid>,
    pub widget: Option<Uuid>,
}

pub struct Call {
    pub correlation: String,
    pub service: &'static str,
    pub method: &'static str,
    pub client_kind: ClientKind,
    version_header: Option<String>,
    pub client_version: Option<semver::Version>,
    pub authorization: Option<String>,
    pub client_ip: String,
    pub body_bytes: usize,
    pub started: Instant,
    pub now: DateTime<Utc>,
    pub actor: Mutex<Actor>,
}

/// A user authenticated with a Logto token and connected to an AnyDash account.
#[derive(Clone, Debug)]
pub struct User {
    pub subject: String,
    pub user_id: Uuid,
    pub personal_tenant: Uuid,
    pub time_zone: String,
    pub preferred_device: Option<Uuid>,
}

/// A collector authenticated with an active device access token.
#[derive(Clone, Debug)]
pub struct Device {
    pub device_id: Uuid,
    pub user_id: Uuid,
    pub personal_tenant: Uuid,
}

#[derive(Clone, Debug)]
pub enum Principal {
    User(User),
    Device(Device),
}
impl Principal {
    pub fn user_id(&self) -> Uuid {
        match self {
            Self::User(u) => u.user_id,
            Self::Device(d) => d.user_id,
        }
    }
    pub fn personal_tenant(&self) -> Uuid {
        match self {
            Self::User(u) => u.personal_tenant,
            Self::Device(d) => d.personal_tenant,
        }
    }
}

impl Call {
    pub fn new(ctx: &RequestContext, service: &'static str, method: &'static str) -> Self {
        let header = |name: &str| {
            ctx.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let meta = ctx.extensions().get::<RequestMeta>().cloned();
        let kind_header = header(CLIENT_KIND_HEADER);
        Self {
            correlation: meta
                .as_ref()
                .map(|m| m.correlation.clone())
                .unwrap_or_else(|| crate::new_id().to_string()),
            service,
            method,
            client_kind: kind_header
                .as_deref()
                .and_then(ClientKind::from_proto_name)
                .unwrap_or(ClientKind::Unspecified),
            version_header: header(CLIENT_VERSION_HEADER),
            client_version: None,
            authorization: header("authorization"),
            client_ip: meta
                .as_ref()
                .map(|m| m.client_ip.clone())
                .unwrap_or_default(),
            body_bytes: meta.map(|m| m.body_bytes).unwrap_or(0),
            started: Instant::now(),
            now: crate::time::now(),
            actor: Mutex::new(Actor::default()),
        }
    }

    pub fn span(&self) -> tracing::Span {
        tracing::info_span!(
            "rpc",
            correlation_id = %self.correlation,
            service = self.service,
            method = self.method,
            client_kind = self.client_kind.proto_name(),
            client_version = self.version_header.as_deref().unwrap_or(""),
        )
    }

    pub fn note(&self, f: impl FnOnce(&mut Actor)) {
        let mut actor = self.actor.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut actor);
    }

    fn actor(&self) -> Actor {
        self.actor.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn bearer(&self) -> Bearer<'_> {
        bearer(self.authorization.as_deref())
    }
}

/// Validates the client headers, the minimum client version and the body size.
/// Runs before authentication on every API request.
pub fn admit(call: &mut Call, config: &crate::config::Config) -> Result<()> {
    if call.client_kind == ClientKind::Unspecified {
        return Err(ApiError::invalid(
            "X-AnyDash-Client-Kind",
            "missing or unknown client kind",
        ));
    }
    let version = call
        .version_header
        .as_deref()
        .and_then(|v| semver::Version::parse(v).ok())
        .filter(|v| v.build.is_empty())
        .ok_or_else(|| {
            ApiError::invalid(
                "X-AnyDash-Client-Version",
                "missing or invalid client version",
            )
        })?;
    if let Some(minimum) = config.min_versions.get(&call.client_kind.to_i32())
        && version < *minimum
    {
        return Err(ApiError::update_required(
            &minimum.to_string(),
            call.client_kind,
        ));
    }
    call.client_version = Some(version);
    if call.body_bytes > MAX_BODY_BYTES {
        return Err(ApiError::payload_too_large());
    }
    Ok(())
}

impl Server {
    /// Validates a Logto user token for the AnyDash resource and returns its subject. A device
    /// token or a missing credential is the wrong credential kind.
    pub async fn subject(&self, call: &Call) -> Result<String> {
        match call.bearer() {
            Bearer::User(token) => Ok(self
                .verifier
                .verify(token, &self.config.resource)
                .await?
                .subject),
            _ => Err(ApiError::unauthenticated()),
        }
    }

    /// Authenticates a user token, resolves the AnyDash account and applies the user rate limit.
    pub async fn user(&self, call: &Call) -> Result<User> {
        let subject = self.subject(call).await?;
        let row: Option<(Uuid, Uuid, String, Option<Uuid>)> = sqlx::query_as(
            "SELECT user_id,personal_tenant_id,time_zone,preferred_device_id FROM accounts WHERE logto_subject=$1",
        )
        .bind(&subject)
        .fetch_optional(&self.db)
        .await?;
        let Some((user_id, personal_tenant, time_zone, preferred_device)) = row else {
            rate::check(
                &self.db,
                RateLimitScope::UserToken,
                &format!("subject:{}", crate::sha256(subject.as_bytes())),
                rate::USER_TOKEN_LIMIT,
            )
            .await?;
            return Err(ApiError::precondition(
                ErrorReason::AccountNotConnected,
                "connect the AnyDash account first",
            ));
        };
        call.note(|a| a.user = Some(user_id));
        rate::check(
            &self.db,
            RateLimitScope::UserToken,
            &user_id.to_string(),
            rate::USER_TOKEN_LIMIT,
        )
        .await?;
        Ok(User {
            subject,
            user_id,
            personal_tenant,
            time_zone,
            preferred_device,
        })
    }

    /// A connected user whose personal space is `ACTIVE`.
    pub async fn active_user(&self, call: &Call) -> Result<User> {
        let user = self.user(call).await?;
        self.delcore
            .require_active(&user.personal_tenant.to_string())
            .await?;
        Ok(user)
    }

    /// Authenticates a device access token, applies the device rate limit and requires the
    /// owner's personal space to be `ACTIVE`.
    pub async fn device(&self, call: &Call) -> Result<Device> {
        let Bearer::Device(token) = call.bearer() else {
            return Err(ApiError::unauthenticated());
        };
        let device = self.device_by_token(call, token).await?;
        rate::check(
            &self.db,
            RateLimitScope::DeviceToken,
            &device.device_id.to_string(),
            rate::DEVICE_TOKEN_LIMIT,
        )
        .await?;
        self.delcore
            .require_active(&device.personal_tenant.to_string())
            .await?;
        Ok(device)
    }

    async fn device_by_token(&self, call: &Call, token: &str) -> Result<Device> {
        let row: Option<CredentialRow> = sqlx::query_as(
            "SELECT d.id,d.user_id,d.credential_state,d.deleted_at,c.access_expires_at,a.personal_tenant_id \
             FROM device_credentials c JOIN devices d ON d.id=c.device_id \
             LEFT JOIN accounts a ON a.user_id=d.user_id WHERE c.access_token_sha256=$1",
        )
        .bind(crate::sha256(token.as_bytes()))
        .fetch_optional(&self.db)
        .await?;
        let Some((device_id, user_id, state, deleted, expires, tenant)) = row else {
            return Err(ApiError::unauthenticated());
        };
        call.note(|a| {
            a.user = Some(user_id);
            a.device = Some(device_id);
        });
        if deleted.is_some()
            || i32::from(state) != DeviceCredentialState::Active.to_i32()
            || tenant.is_none()
        {
            return Err(ApiError::precondition(
                ErrorReason::DeviceCredentialsInactive,
                "device credentials are not active",
            ));
        }
        if expires <= call.now {
            return Err(ApiError::unauthenticated());
        }
        Ok(Device {
            device_id,
            user_id,
            personal_tenant: tenant.unwrap_or_default(),
        })
    }

    /// Accepts a user token or a device token, with the matching space check.
    pub async fn principal(&self, call: &Call) -> Result<Principal> {
        match call.bearer() {
            Bearer::Device(_) => Ok(Principal::Device(self.device(call).await?)),
            Bearer::User(_) => Ok(Principal::User(self.active_user(call).await?)),
            _ => Err(ApiError::unauthenticated()),
        }
    }

    /// Logs, measures and maps one RPC result. Permission denials write an audit event.
    pub async fn finish<T>(&self, call: &Call, result: Result<T>) -> ServiceResult<T> {
        let error = result.as_ref().err();
        let outcome = crate::metrics::outcome(error);
        let elapsed = call.started.elapsed();
        self.metrics
            .requests
            .with_label_values(&[call.service, call.method, outcome])
            .inc();
        self.metrics
            .duration
            .with_label_values(&[call.service, call.method])
            .observe(elapsed.as_secs_f64());
        if let Some(e) = error
            && matches!(
                e.reason,
                ErrorReason::PermissionDenied | ErrorReason::DataOwnerRequired
            )
        {
            let actor = self.actor_for_audit(call);
            if let Err(audit) = crate::audit::record_pool(
                &self.db,
                call,
                crate::audit::Event {
                    kind: ActivityKind::PermissionDenied,
                    actor_user: actor.user,
                    actor_device: actor.device,
                    subject_user: actor.user,
                    tenant: actor.tenant,
                    dashboard: actor.dashboard,
                    widget: actor.widget,
                    ..Default::default()
                },
            )
            .await
            {
                tracing::warn!(
                    reason = audit.reason_name(),
                    "permission denial audit failed"
                );
            }
        }
        tracing::info!(
            outcome,
            reason = error.map(|e| e.reason_name()).unwrap_or(""),
            duration_ms = elapsed.as_millis() as u64,
            "AnyDash request completed"
        );
        match result {
            Ok(body) => {
                Ok(Response::new(body).with_header(CORRELATION_HEADER, call.correlation.as_str()))
            }
            Err(e) => Err(e.connect(&call.correlation)),
        }
    }

    fn actor_for_audit(&self, call: &Call) -> Actor {
        call.actor()
    }
}

/// Implements a generated Connect service trait by delegating each method to a handler
/// `async fn(&Server, &Call, Request) -> Result<Response>`.
#[macro_export]
macro_rules! service {
    ($trait:path, $service:literal, { $($method:ident => $name:literal, $req:ty, $resp:ty, $handler:path;)* }) => {
        #[allow(refining_impl_trait)]
        impl $trait for $crate::Server {
            $(
            async fn $method(
                &self,
                ctx: ::connectrpc::RequestContext,
                q: ::connectrpc::ServiceRequest<'_, $req>,
            ) -> ::connectrpc::ServiceResult<$resp> {
                use ::tracing::Instrument;
                let mut call = $crate::call::Call::new(&ctx, $service, $name);
                let request = q.to_owned_message();
                let span = call.span();
                async move {
                    let result = match $crate::call::admit(&mut call, &self.config) {
                        Ok(()) => $handler(self, &call, request).await,
                        Err(e) => Err(e),
                    };
                    self.finish(&call, result).await
                }
                .instrument(span)
                .await
            }
            )*
        }
    };
}
