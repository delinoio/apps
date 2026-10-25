// SPDX-License-Identifier: FSL-1.1-ALv2
pub mod accounting;
pub mod api_keys;
pub mod auth;
pub mod catalog;
pub mod config;
pub mod events;
pub mod features;
mod fingerprint;
pub mod http;
pub mod metering;
pub mod postgres;
pub mod provider;
pub mod rating;
pub mod rpc;
pub mod telemetry;
pub mod worker;
use buffa::Message;
use connectrpc::{ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult};
use delino_protocol::delcore::v1::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use sqlx::{PgPool, Row};
use std::sync::Arc;
use tracing::Instrument;

pub type Result<T> = std::result::Result<T, Fault>;
#[derive(Debug)]
pub struct Fault {
    pub code: ErrorCode,
    pub reason: i32,
    pub message: String,
    pub permanent: bool,
    pub cause: Option<anyhow::Error>,
    pub limit: Option<telemetry::LimitKind>,
}
impl Fault {
    pub fn new(code: ErrorCode, reason: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            reason,
            message: message.into(),
            permanent: false,
            cause: None,
            limit: None,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, 1, message)
    }
    pub fn unauthenticated() -> Self {
        Self::new(ErrorCode::Unauthenticated, 2, "authentication failed")
    }
    pub fn denied() -> Self {
        Self::new(ErrorCode::PermissionDenied, 3, "operation is not permitted")
    }
    pub fn conflict() -> Self {
        Self::new(
            ErrorCode::AlreadyExists,
            4,
            "request identity conflicts with previous input",
        )
    }
    pub fn precondition(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::FailedPrecondition, 5, message)
    }
    pub fn unavailable() -> Self {
        Self::new(ErrorCode::Internal, 6, "operation could not be completed")
    }
    pub fn missing() -> Self {
        Self::new(ErrorCode::NotFound, 6, "resource not found")
    }
    fn log_rejection(&self, correlation: &str, op: Op) {
        let sqlstate = self.cause.as_ref().and_then(|cause| {
            cause
                .chain()
                .find_map(|cause| match cause.downcast_ref::<sqlx::Error>() {
                    Some(sqlx::Error::Database(db)) => db.code().map(|code| code.into_owned()),
                    _ => None,
                })
        });
        // Database messages can contain customer values; retain only SQLSTATE.
        tracing::warn!(
            correlation_id = correlation,
            procedure = op.name(),
            category = self.code.as_str(),
            reason = self.reason,
            sqlstate = sqlstate.as_deref(),
            "rpc rejected"
        );
    }
    pub fn with_correlation(self, correlation: &str) -> connectrpc::ConnectError {
        let detail = ErrorDetail {
            reason: self.reason.into(),
            correlation_id: correlation.to_owned(),
            ..Default::default()
        };
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("X-Delcore-Correlation-Id", correlation.parse().unwrap());
        self.connect()
            .with_headers(headers)
            .with_detail(connectrpc::ErrorDetail::from_message(
                "delcore.v1.ErrorDetail",
                &detail,
            ))
    }
    pub fn connect(self) -> connectrpc::ConnectError {
        connectrpc::ConnectError::new(self.code, self.message)
    }
}
impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Fault {}
impl From<sqlx::Error> for Fault {
    fn from(error: sqlx::Error) -> Self {
        let mut fault = match &error {
            sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => Self::conflict(),
            _ => Self::unavailable(),
        };
        fault.cause = Some(error.into());
        fault
    }
}
impl From<serde_json::Error> for Fault {
    fn from(error: serde_json::Error) -> Self {
        let mut fault = Self::unavailable();
        fault.cause = Some(error.into());
        fault
    }
}
impl From<reqwest::Error> for Fault {
    fn from(error: reqwest::Error) -> Self {
        let mut fault = Self::unavailable();
        fault.cause = Some(error.into());
        fault
    }
}
impl From<anyhow::Error> for Fault {
    fn from(error: anyhow::Error) -> Self {
        let mut fault = Self::unavailable();
        fault.cause = Some(error);
        fault
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    DeleteGroup,
    PutMeter,
    PutFeature,
    PutAddOn,
    ListBillingHistory,
    GetCurrentUser,
    CreateTenant,
    GetTenant,
    ListMembers,
    SetMemberRoles,
    RemoveMember,
    CreateInvitation,
    AcceptInvitation,
    RevokeInvitation,
    TransferOwnership,
    AcceptOwnership,
    SetGroup,
    ListGroups,
    GetCatalog,
    PutService,
    PutPlan,
    CreatePrice,
    Authorize,
    CreateApiKey,
    ListApiKeys,
    RevokeApiKey,
    AuthorizeApiKey,
    ListMeterAggregates,
    GetMeteringStatus,
    CheckTenantLifecycle,
    GetTenantStates,
    IngestEvent,
    CorrectEvent,
    GetEvent,
    ListLedger,
    RequestTopUp,
    SetAutoTopUp,
    ChangeSubscription,
    CancelSubscription,
    DeleteTenant,
    Withdraw,
    SetTenantBlocked,
    ListJobs,
    RetryJob,
    ListAudit,
}
impl Op {
    pub fn name(self) -> &'static str {
        match self {
            Self::DeleteGroup => "DeleteGroup",
            Self::PutMeter => "PutMeter",
            Self::PutFeature => "PutFeature",
            Self::PutAddOn => "PutAddOn",
            Self::ListBillingHistory => "ListBillingHistory",
            Self::GetCurrentUser => "GetCurrentUser",
            Self::CreateTenant => "CreateTenant",
            Self::GetTenant => "GetTenant",
            Self::ListMembers => "ListMembers",
            Self::SetMemberRoles => "SetMemberRoles",
            Self::RemoveMember => "RemoveMember",
            Self::CreateInvitation => "CreateInvitation",
            Self::AcceptInvitation => "AcceptInvitation",
            Self::RevokeInvitation => "RevokeInvitation",
            Self::TransferOwnership => "TransferOwnership",
            Self::AcceptOwnership => "AcceptOwnership",
            Self::SetGroup => "SetGroup",
            Self::ListGroups => "ListGroups",
            Self::GetCatalog => "GetCatalog",
            Self::PutService => "PutService",
            Self::PutPlan => "PutPlan",
            Self::CreatePrice => "CreatePrice",
            Self::Authorize => "Authorize",
            Self::CreateApiKey => "CreateApiKey",
            Self::ListApiKeys => "ListApiKeys",
            Self::RevokeApiKey => "RevokeApiKey",
            Self::AuthorizeApiKey => "AuthorizeApiKey",
            Self::ListMeterAggregates => "ListMeterAggregates",
            Self::GetMeteringStatus => "GetMeteringStatus",
            Self::CheckTenantLifecycle => "CheckTenantLifecycle",
            Self::GetTenantStates => "GetTenantStates",
            Self::IngestEvent => "IngestEvent",
            Self::CorrectEvent => "CorrectEvent",
            Self::GetEvent => "GetEvent",
            Self::ListLedger => "ListLedger",
            Self::RequestTopUp => "RequestTopUp",
            Self::SetAutoTopUp => "SetAutoTopUp",
            Self::ChangeSubscription => "ChangeSubscription",
            Self::CancelSubscription => "CancelSubscription",
            Self::DeleteTenant => "DeleteTenant",
            Self::Withdraw => "Withdraw",
            Self::SetTenantBlocked => "SetTenantBlocked",
            Self::ListJobs => "ListJobs",
            Self::RetryJob => "RetryJob",
            Self::ListAudit => "ListAudit",
        }
    }
    pub fn mutation(self) -> bool {
        matches!(
            self,
            Self::DeleteGroup
                | Self::PutMeter
                | Self::PutFeature
                | Self::PutAddOn
                | Self::CreateTenant
                | Self::SetMemberRoles
                | Self::RemoveMember
                | Self::CreateInvitation
                | Self::AcceptInvitation
                | Self::RevokeInvitation
                | Self::TransferOwnership
                | Self::AcceptOwnership
                | Self::SetGroup
                | Self::PutService
                | Self::PutPlan
                | Self::CreatePrice
                | Self::CreateApiKey
                | Self::RevokeApiKey
                | Self::IngestEvent
                | Self::CorrectEvent
                | Self::RequestTopUp
                | Self::SetAutoTopUp
                | Self::ChangeSubscription
                | Self::CancelSubscription
                | Self::DeleteTenant
                | Self::Withdraw
                | Self::SetTenantBlocked
                | Self::RetryJob
        )
    }
}
fn event_span(op: Op, correlation: &str) -> tracing::Span {
    macro_rules! span {
        ($name:literal) => {
            tracing::info_span!(
                $name,
                "delcore.correlation_id" = correlation,
                "delcore.outcome" = tracing::field::Empty,
                "delcore.error_reason" = tracing::field::Empty,
                "delcore.event.revision" = tracing::field::Empty,
                "delcore.event.matched_meters" = tracing::field::Empty,
                "delcore.correction.kind" = tracing::field::Empty,
            )
        };
    }
    match op {
        Op::IngestEvent => span!("delcore.event.ingest"),
        Op::CorrectEvent => span!("delcore.event.correct"),
        _ => tracing::Span::none(),
    }
}

pub struct Server {
    pub db: PgPool,
    pub auth: auth::Verifier,
    pub config: config::Config,
    pub metrics: Arc<telemetry::Metrics>,
}
impl Server {
    pub fn new(db: PgPool, auth: auth::Verifier, config: config::Config) -> Self {
        Self {
            db,
            auth,
            config,
            metrics: Arc::new(telemetry::Metrics::default()),
        }
    }
    async fn handle<Q, R>(&self, ctx: RequestContext, op: Op, q: Q) -> ServiceResult<R>
    where
        Q: Message + Serialize + DeserializeOwned,
        R: Message + Serialize + DeserializeOwned,
    {
        let correlation = ctx
            .headers()
            .get("X-Delcore-Correlation-Id")
            .and_then(|value| value.to_str().ok())
            .filter(|value| server_common::canonical_id(value))
            .map(str::to_owned)
            .unwrap_or_else(server_common::id);
        let authorization = ctx
            .headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        let input = serde_json::to_value(&q).unwrap_or_default();
        let event_span = event_span(op, &correlation);
        let result = tokio::time::timeout(
            self.config.timeout(),
            self.execute::<Q, R>(authorization, op, q, &correlation)
                .instrument(event_span.clone()),
        )
        .await
        .unwrap_or_else(|_| {
            Err(Fault::new(
                ErrorCode::DeadlineExceeded,
                6,
                "request deadline exceeded",
            ))
        });
        if matches!(op, Op::IngestEvent | Op::CorrectEvent) {
            let outcome = match &result {
                Ok(v) if v["_replayed"].as_bool().unwrap_or(false) => telemetry::Outcome::Replayed,
                Ok(_) => telemetry::Outcome::Accepted,
                Err(_) => telemetry::Outcome::Rejected,
            };
            let reason = result.as_ref().err().map(|f| match f.reason {
                1 => "invalid_input",
                2 => "unauthenticated",
                3 => "forbidden",
                4 => "conflict",
                5 => "precondition",
                6 => "dependency",
                7 => "stale_revision",
                8 => "no_matching_meter",
                9 => "invalid_attribute",
                10 => "cardinality_exceeded",
                11 => "event_identity_conflict",
                12 => "event_not_found",
                13 => "event_cancelled",
                14 => "reporting_window_closed",
                _ => "other",
            });
            event_span.record("delcore.outcome", outcome.as_str());
            event_span.record("delcore.error_reason", telemetry::reason(reason));
            if let Ok(value) = &result
                && let Some(revision) = value["revision"].as_i64()
            {
                event_span.record("delcore.event.revision", revision);
            }
            if op == Op::IngestEvent {
                self.metrics.event_ingested(outcome, reason);
            } else {
                let kind = match input["kind"].as_str() {
                    Some("EVENT_CORRECTION_KIND_REPLACE") => telemetry::CorrectionKind::Replace,
                    Some("EVENT_CORRECTION_KIND_CANCEL") => telemetry::CorrectionKind::Cancel,
                    _ => match input["kind"].as_i64() {
                        Some(1) => telemetry::CorrectionKind::Replace,
                        Some(2) => telemetry::CorrectionKind::Cancel,
                        _ => telemetry::CorrectionKind::Invalid,
                    },
                };
                event_span.record("delcore.correction.kind", kind.as_str());
                self.metrics.event_corrected(kind, outcome, reason);
            }
            if let Some(limit) = result.as_ref().err().and_then(|f| f.limit) {
                self.metrics.cardinality_rejected(limit);
            }
        }
        if let Some(operation) = telemetry::ApiKeyOperation::of(op) {
            let outcome = match &result {
                Ok(v) if op == Op::AuthorizeApiKey => {
                    if v["allowed"].as_bool().unwrap_or(false) {
                        telemetry::ApiKeyOutcome::Allowed
                    } else {
                        telemetry::ApiKeyOutcome::Denied
                    }
                }
                Ok(_) => telemetry::ApiKeyOutcome::Succeeded,
                Err(fault) if fault.reason == 2 => telemetry::ApiKeyOutcome::Unauthenticated,
                Err(_) => telemetry::ApiKeyOutcome::Rejected,
            };
            self.metrics.api_key_operation(operation, outcome);
        }
        match result {
            Ok(mut value) => {
                value.as_object_mut().unwrap().remove("_replayed");
                value["metadata"] = json!({"correlationId":correlation});
                let body =
                    serde_json::from_value(value).map_err(|_| Fault::unavailable().connect())?;
                Ok(Response::new(body).with_header("X-Delcore-Correlation-Id", correlation))
            }
            Err(fault) => {
                fault.log_rejection(&correlation, op);
                Err(fault.with_correlation(&correlation))
            }
        }
    }
    async fn execute<Q, R>(
        &self,
        authorization: &str,
        op: Op,
        request: Q,
        correlation: &str,
    ) -> Result<Value>
    where
        Q: Message + Serialize + DeserializeOwned,
        R: Message + Serialize + DeserializeOwned,
    {
        let identity = self
            .auth
            .verify(authorization)
            .await
            .map_err(|_| Fault::unauthenticated())?;
        let q = serde_json::to_value(&request)?;
        let key = server_common::string(&q, "requestKey");
        if op.mutation() && !rpc::canonical(key) {
            return Err(Fault::invalid("request_key must be a UUID v7"));
        }
        let raw = request.encode_to_vec();
        let fingerprint = fingerprint::fingerprint(op, &raw)?;
        let mut tx = self.db.begin().await?;
        // Single-instance v1 serializes state transitions, matching worker transactions.
        // Remove only together with per-aggregate locks and concurrency regression tests.
        sqlx::query("SELECT pg_advisory_xact_lock(41820003)")
            .execute(&mut *tx)
            .await?;
        // Read the clock only while holding the lock. A request that waited here must not be
        // validated against a timestamp from before the transactions it waited for. In particular,
        // a deleting tenant's cancellation check runs under this lock and relies on a later
        // reporting request seeing the ended grace period.
        let now = server_common::now();
        let mut call = rpc::Call {
            tx: &mut tx,
            identity,
            user: String::new(),
            service: String::new(),
            actor: String::new(),
            correlation: correlation.into(),
            now,
            config: &self.config,
        };
        if !call.identity.client_id.is_empty() {
            call.service =
                sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM services WHERE client_id=$1")
                    .bind(&call.identity.client_id)
                    .fetch_optional(&mut *call.tx)
                    .await?
                    .ok_or_else(Fault::denied)?
                    .to_string();
            call.actor = format!("service:{}", call.service);
        } else {
            let row=sqlx::query("INSERT INTO users(id,issuer,subject_hash,email) VALUES($1,$2,$3,$4) ON CONFLICT(issuer,subject_hash) DO UPDATE SET email=EXCLUDED.email RETURNING id,withdrawing_at").bind(uuid::Uuid::now_v7()).bind(&self.config.issuer).bind(&call.identity.subject_hash).bind(&call.identity.email).fetch_one(&mut *call.tx).await?;
            call.user = row.get::<uuid::Uuid, _>("id").to_string();
            if row
                .get::<Option<chrono::DateTime<chrono::Utc>>, _>("withdrawing_at")
                .is_some()
                && !matches!(
                    op,
                    Op::GetTenant
                        | Op::ListLedger
                        | Op::ListBillingHistory
                        | Op::RequestTopUp
                        | Op::CancelSubscription
                        | Op::Withdraw
                )
            {
                return Err(Fault::precondition("withdrawal is pending"));
            }
            call.actor = format!("user:{}", call.user);
            let (personal,configured):(bool,bool)=sqlx::query_as("SELECT EXISTS(SELECT 1 FROM tenants WHERE personal AND owner_id=$1 AND deleted_at IS NULL),EXISTS(SELECT 1 FROM plans WHERE is_default AND free)").bind(rpc::uuid(&call.user)?).fetch_one(&mut *call.tx).await?;
            if !personal && configured {
                call.create_tenant("Personal", true).await?;
            }
            if !configured && !call.identity.operator {
                return Err(Fault::precondition("default free plan is not configured"));
            }
        }
        if op.mutation() {
            // Enforce expiry even when maintenance has not run. A replay never
            // refreshes created_at, and current authorization still applies.
            sqlx::query(
                "DELETE FROM requests WHERE actor_id=$1 AND request_key=$2 AND created_at<=$3",
            )
            .bind(&call.actor)
            .bind(key)
            .bind(now - chrono::Duration::seconds(config::TRANSIENT_RETENTION_SECONDS))
            .execute(&mut *call.tx)
            .await?;
        }
        if op.mutation()
            && let Some(row)=sqlx::query("SELECT fingerprint,response,EXISTS(SELECT 1 FROM tenants t WHERE t.id=requests.tenant_id AND t.deleted_at IS NOT NULL) AS removed FROM requests WHERE actor_id=$1 AND request_key=$2").bind(&call.actor).bind(key).fetch_optional(&mut *call.tx).await?{
    if row.get::<String,_>("fingerprint")!=fingerprint{return Err(Fault::conflict());}
    if row.get::<bool,_>("removed"){return Err(Fault::precondition("cached response was removed under retention policy"));}
    call.authorize_replay(op,&q).await?;
    let bytes:Vec<u8>=row.get("response");let response=R::decode_from_slice(&bytes).map_err(|_|Fault::unavailable())?;
    let mut value=serde_json::to_value(response)?;value["_replayed"]=json!(true);tx.commit().await?;return Ok(value);
        }
        let mut response = call.dispatch(op, &q).await?;
        let replayed = response.as_object_mut().unwrap().remove("_replayed");
        let retry_kind = response.as_object_mut().unwrap().remove("_retryKind");
        if op.mutation() {
            // A one-time secret must never reach the replay record: a replayed
            // response carries the key metadata and an empty secret.
            let mut stored = response.clone();
            if op == Op::CreateApiKey {
                stored["secret"] = json!("");
            }
            let message: R = serde_json::from_value(stored)?;
            let tenant = server_common::string(&q, "tenantId");
            let tenant = if rpc::canonical(tenant) {
                Some(rpc::uuid(tenant)?)
            } else if op == Op::CreateTenant {
                Some(rpc::uuid(server_common::string(&response["tenant"], "id"))?)
            } else {
                None
            };
            sqlx::query("INSERT INTO requests(actor_id,request_key,fingerprint,response,created_at,tenant_id) VALUES($1,$2,$3,$4,$5,$6)").bind(&call.actor).bind(key).bind(fingerprint).bind(message.encode_to_vec()).bind(now).bind(tenant).execute(&mut *call.tx).await?;
        }
        tx.commit().await?;
        if let Some(kind) = retry_kind.and_then(|v| v.as_str().map(str::to_owned)) {
            self.metrics.operator_retry(&kind);
        }
        if let Some(replayed) = replayed {
            response["_replayed"] = replayed;
        }
        Ok(response)
    }
}
#[allow(refining_impl_trait)]
impl DelcoreService for Server {
    async fn delete_group(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteGroupRequest>,
    ) -> ServiceResult<DeleteGroupResponse> {
        self.handle(ctx, Op::DeleteGroup, q.to_owned_message())
            .await
    }
    async fn put_meter(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, PutMeterRequest>,
    ) -> ServiceResult<PutMeterResponse> {
        self.handle(ctx, Op::PutMeter, q.to_owned_message()).await
    }
    async fn put_feature(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, PutFeatureRequest>,
    ) -> ServiceResult<PutFeatureResponse> {
        self.handle(ctx, Op::PutFeature, q.to_owned_message()).await
    }
    async fn put_add_on(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, PutAddOnRequest>,
    ) -> ServiceResult<PutAddOnResponse> {
        self.handle(ctx, Op::PutAddOn, q.to_owned_message()).await
    }
    async fn list_billing_history(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListBillingHistoryRequest>,
    ) -> ServiceResult<ListBillingHistoryResponse> {
        self.handle(ctx, Op::ListBillingHistory, q.to_owned_message())
            .await
    }
    async fn get_current_user(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetCurrentUserRequest>,
    ) -> ServiceResult<GetCurrentUserResponse> {
        self.handle(ctx, Op::GetCurrentUser, q.to_owned_message())
            .await
    }
    async fn create_tenant(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreateTenantRequest>,
    ) -> ServiceResult<CreateTenantResponse> {
        self.handle(ctx, Op::CreateTenant, q.to_owned_message())
            .await
    }
    async fn get_tenant(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetTenantRequest>,
    ) -> ServiceResult<GetTenantResponse> {
        self.handle(ctx, Op::GetTenant, q.to_owned_message()).await
    }
    async fn list_members(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListMembersRequest>,
    ) -> ServiceResult<ListMembersResponse> {
        self.handle(ctx, Op::ListMembers, q.to_owned_message())
            .await
    }
    async fn set_member_roles(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SetMemberRolesRequest>,
    ) -> ServiceResult<SetMemberRolesResponse> {
        self.handle(ctx, Op::SetMemberRoles, q.to_owned_message())
            .await
    }
    async fn remove_member(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, RemoveMemberRequest>,
    ) -> ServiceResult<RemoveMemberResponse> {
        self.handle(ctx, Op::RemoveMember, q.to_owned_message())
            .await
    }
    async fn create_invitation(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreateInvitationRequest>,
    ) -> ServiceResult<CreateInvitationResponse> {
        self.handle(ctx, Op::CreateInvitation, q.to_owned_message())
            .await
    }
    async fn accept_invitation(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, AcceptInvitationRequest>,
    ) -> ServiceResult<AcceptInvitationResponse> {
        self.handle(ctx, Op::AcceptInvitation, q.to_owned_message())
            .await
    }
    async fn revoke_invitation(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, RevokeInvitationRequest>,
    ) -> ServiceResult<RevokeInvitationResponse> {
        self.handle(ctx, Op::RevokeInvitation, q.to_owned_message())
            .await
    }
    async fn transfer_ownership(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, TransferOwnershipRequest>,
    ) -> ServiceResult<TransferOwnershipResponse> {
        self.handle(ctx, Op::TransferOwnership, q.to_owned_message())
            .await
    }
    async fn accept_ownership(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, AcceptOwnershipRequest>,
    ) -> ServiceResult<AcceptOwnershipResponse> {
        self.handle(ctx, Op::AcceptOwnership, q.to_owned_message())
            .await
    }
    async fn set_group(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SetGroupRequest>,
    ) -> ServiceResult<SetGroupResponse> {
        self.handle(ctx, Op::SetGroup, q.to_owned_message()).await
    }
    async fn list_groups(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListGroupsRequest>,
    ) -> ServiceResult<ListGroupsResponse> {
        self.handle(ctx, Op::ListGroups, q.to_owned_message()).await
    }
    async fn get_catalog(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetCatalogRequest>,
    ) -> ServiceResult<GetCatalogResponse> {
        self.handle(ctx, Op::GetCatalog, q.to_owned_message()).await
    }
    async fn put_service(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, PutServiceRequest>,
    ) -> ServiceResult<PutServiceResponse> {
        self.handle(ctx, Op::PutService, q.to_owned_message()).await
    }
    async fn put_plan(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, PutPlanRequest>,
    ) -> ServiceResult<PutPlanResponse> {
        self.handle(ctx, Op::PutPlan, q.to_owned_message()).await
    }
    async fn create_price(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreatePriceRequest>,
    ) -> ServiceResult<CreatePriceResponse> {
        self.handle(ctx, Op::CreatePrice, q.to_owned_message())
            .await
    }
    async fn authorize(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, AuthorizeRequest>,
    ) -> ServiceResult<AuthorizeResponse> {
        self.handle(ctx, Op::Authorize, q.to_owned_message()).await
    }
    async fn create_api_key(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreateApiKeyRequest>,
    ) -> ServiceResult<CreateApiKeyResponse> {
        self.handle(ctx, Op::CreateApiKey, q.to_owned_message())
            .await
    }
    async fn list_api_keys(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListApiKeysRequest>,
    ) -> ServiceResult<ListApiKeysResponse> {
        self.handle(ctx, Op::ListApiKeys, q.to_owned_message())
            .await
    }
    async fn revoke_api_key(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, RevokeApiKeyRequest>,
    ) -> ServiceResult<RevokeApiKeyResponse> {
        self.handle(ctx, Op::RevokeApiKey, q.to_owned_message())
            .await
    }
    async fn authorize_api_key(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, AuthorizeApiKeyRequest>,
    ) -> ServiceResult<AuthorizeApiKeyResponse> {
        self.handle(ctx, Op::AuthorizeApiKey, q.to_owned_message())
            .await
    }
    async fn list_meter_aggregates(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListMeterAggregatesRequest>,
    ) -> ServiceResult<ListMeterAggregatesResponse> {
        self.handle(ctx, Op::ListMeterAggregates, q.to_owned_message())
            .await
    }
    async fn get_metering_status(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetMeteringStatusRequest>,
    ) -> ServiceResult<GetMeteringStatusResponse> {
        self.handle(ctx, Op::GetMeteringStatus, q.to_owned_message())
            .await
    }
    async fn check_tenant_lifecycle(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CheckTenantLifecycleRequest>,
    ) -> ServiceResult<CheckTenantLifecycleResponse> {
        self.handle(ctx, Op::CheckTenantLifecycle, q.to_owned_message())
            .await
    }
    async fn get_tenant_states(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetTenantStatesRequest>,
    ) -> ServiceResult<GetTenantStatesResponse> {
        self.handle(ctx, Op::GetTenantStates, q.to_owned_message())
            .await
    }
    async fn ingest_event(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, IngestEventRequest>,
    ) -> ServiceResult<IngestEventResponse> {
        self.handle(ctx, Op::IngestEvent, q.to_owned_message())
            .await
    }
    async fn correct_event(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CorrectEventRequest>,
    ) -> ServiceResult<CorrectEventResponse> {
        self.handle(ctx, Op::CorrectEvent, q.to_owned_message())
            .await
    }
    async fn get_event(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetEventRequest>,
    ) -> ServiceResult<GetEventResponse> {
        self.handle(ctx, Op::GetEvent, q.to_owned_message()).await
    }
    async fn list_ledger(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListLedgerRequest>,
    ) -> ServiceResult<ListLedgerResponse> {
        self.handle(ctx, Op::ListLedger, q.to_owned_message()).await
    }
    async fn request_top_up(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, RequestTopUpRequest>,
    ) -> ServiceResult<RequestTopUpResponse> {
        self.handle(ctx, Op::RequestTopUp, q.to_owned_message())
            .await
    }
    async fn set_auto_top_up(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SetAutoTopUpRequest>,
    ) -> ServiceResult<SetAutoTopUpResponse> {
        self.handle(ctx, Op::SetAutoTopUp, q.to_owned_message())
            .await
    }
    async fn change_subscription(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ChangeSubscriptionRequest>,
    ) -> ServiceResult<ChangeSubscriptionResponse> {
        self.handle(ctx, Op::ChangeSubscription, q.to_owned_message())
            .await
    }
    async fn cancel_subscription(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CancelSubscriptionRequest>,
    ) -> ServiceResult<CancelSubscriptionResponse> {
        self.handle(ctx, Op::CancelSubscription, q.to_owned_message())
            .await
    }
    async fn delete_tenant(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteTenantRequest>,
    ) -> ServiceResult<DeleteTenantResponse> {
        self.handle(ctx, Op::DeleteTenant, q.to_owned_message())
            .await
    }
    async fn withdraw(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, WithdrawRequest>,
    ) -> ServiceResult<WithdrawResponse> {
        self.handle(ctx, Op::Withdraw, q.to_owned_message()).await
    }
    async fn set_tenant_blocked(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SetTenantBlockedRequest>,
    ) -> ServiceResult<SetTenantBlockedResponse> {
        self.handle(ctx, Op::SetTenantBlocked, q.to_owned_message())
            .await
    }
    async fn list_jobs(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListJobsRequest>,
    ) -> ServiceResult<ListJobsResponse> {
        self.handle(ctx, Op::ListJobs, q.to_owned_message()).await
    }
    async fn retry_job(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, RetryJobRequest>,
    ) -> ServiceResult<RetryJobResponse> {
        self.handle(ctx, Op::RetryJob, q.to_owned_message()).await
    }
    async fn list_audit(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListAuditRequest>,
    ) -> ServiceResult<ListAuditResponse> {
        self.handle(ctx, Op::ListAudit, q.to_owned_message()).await
    }
}

#[cfg(test)]
mod trace_tests;
