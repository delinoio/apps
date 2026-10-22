// SPDX-License-Identifier: FSL-1.1-ALv2
mod auth;
pub mod config;
mod fingerprint;
pub mod http;
pub mod maintenance;
mod operations;
mod size;
pub mod store;
mod validation;
use buffa::Message;
use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use delino_protocol::devbase::v1::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use server_common::{Fault, Result, authority::Authority, limit::Limiter};

use sqlx::{PgPool, Row};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    CreateProject,
    GetProject,
    ListProjects,
    UpdateProject,
    DeleteProject,
    IssueApiKey,
    GetApiKey,
    ListApiKeys,
    RevokeApiKey,
    CreateEndUser,
    GetEndUser,
    LookupEndUser,
    ListEndUsers,
    UpdateEndUser,
    DeleteEndUser,
}
impl Op {
    pub fn name(self) -> &'static str {
        match self {
            Self::CreateProject => "CreateProject",
            Self::GetProject => "GetProject",
            Self::ListProjects => "ListProjects",
            Self::UpdateProject => "UpdateProject",
            Self::DeleteProject => "DeleteProject",
            Self::IssueApiKey => "IssueApiKey",
            Self::GetApiKey => "GetApiKey",
            Self::ListApiKeys => "ListApiKeys",
            Self::RevokeApiKey => "RevokeApiKey",
            Self::CreateEndUser => "CreateEndUser",
            Self::GetEndUser => "GetEndUser",
            Self::LookupEndUser => "LookupEndUser",
            Self::ListEndUsers => "ListEndUsers",
            Self::UpdateEndUser => "UpdateEndUser",
            Self::DeleteEndUser => "DeleteEndUser",
        }
    }
    /// Resolves an exact DevBase RPC path to its operation. Any other path, including near
    /// misses, returns `None` so request-controlled text never becomes a metric label.
    pub fn from_procedure_path(path: &str) -> Option<Self> {
        let op = match path.strip_prefix("/devbase.v1.DevBaseService/")? {
            "CreateProject" => Self::CreateProject,
            "GetProject" => Self::GetProject,
            "ListProjects" => Self::ListProjects,
            "UpdateProject" => Self::UpdateProject,
            "DeleteProject" => Self::DeleteProject,
            "IssueApiKey" => Self::IssueApiKey,
            "GetApiKey" => Self::GetApiKey,
            "ListApiKeys" => Self::ListApiKeys,
            "RevokeApiKey" => Self::RevokeApiKey,
            "CreateEndUser" => Self::CreateEndUser,
            "GetEndUser" => Self::GetEndUser,
            "LookupEndUser" => Self::LookupEndUser,
            "ListEndUsers" => Self::ListEndUsers,
            "UpdateEndUser" => Self::UpdateEndUser,
            "DeleteEndUser" => Self::DeleteEndUser,
            _ => return None,
        };
        Some(op)
    }
    pub fn mutation(self) -> bool {
        matches!(
            self,
            Self::CreateProject
                | Self::UpdateProject
                | Self::DeleteProject
                | Self::IssueApiKey
                | Self::RevokeApiKey
                | Self::CreateEndUser
                | Self::UpdateEndUser
                | Self::DeleteEndUser
        )
    }
    pub fn management(self) -> bool {
        !matches!(
            self,
            Self::CreateEndUser
                | Self::GetEndUser
                | Self::LookupEndUser
                | Self::ListEndUsers
                | Self::UpdateEndUser
                | Self::DeleteEndUser
        )
    }
    pub fn tenant_target(self) -> bool {
        matches!(self, Self::CreateProject | Self::ListProjects)
    }
    pub fn degraded(self) -> bool {
        matches!(
            self,
            Self::GetProject
                | Self::ListProjects
                | Self::DeleteProject
                | Self::GetApiKey
                | Self::ListApiKeys
                | Self::RevokeApiKey
                | Self::DeleteEndUser
        )
    }
}
pub struct Server {
    pub metrics: std::sync::Arc<server_common::telemetry::Metrics>,
    pub db: PgPool,
    pub authority: Authority,
    pub config: config::Config,
    users: Limiter,
    management: Limiter,
}
impl Server {
    pub fn new(db: PgPool, authority: Authority, config: config::Config) -> Self {
        Self {
            metrics: std::sync::Arc::new(server_common::telemetry::Metrics::new(
                server_common::telemetry::Service::DevBase,
            )),
            users: Limiter::new(config.user_rate, config.user_burst),
            management: Limiter::new(config.management_rate, config.management_burst),
            db,
            authority,
            config,
        }
    }
    async fn handle<Q, R>(&self, ctx: RequestContext, op: Op, q: Q) -> ServiceResult<R>
    where
        Q: Message + Serialize + DeserializeOwned,
        R: Message + Serialize + DeserializeOwned,
    {
        let trace = server_common::id();
        let start = std::time::Instant::now();
        let authorization = ctx
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        // Captured by the first-position interceptor from the received message bytes. A missing
        // value (for example a handler reached without the interceptor) fails closed in `execute`.
        let received = ctx
            .extensions()
            .get::<size::ReceivedMessageSize>()
            .map(|size| size.0);
        let result = tokio::time::timeout(
            self.config.request_timeout,
            self.execute::<Q, R>(authorization, op, q, received, &trace),
        )
        .await
        .unwrap_or_else(|_| Err(Fault::unavailable()));
        self.metrics
            .requests
            .with_label_values(&[
                op.name(),
                result
                    .as_ref()
                    .err()
                    .map(|f| f.code.as_str())
                    .unwrap_or("ok"),
            ])
            .inc();
        self.metrics
            .duration
            .with_label_values(&[op.name()])
            .observe(start.elapsed().as_secs_f64());
        tracing::info!(trace_id=%trace,procedure=op.name(),duration_ms=start.elapsed().as_millis(),success=result.is_ok(),"request completed");
        match result {
            Ok(mut value) => {
                value["metadata"] = json!({"traceId":trace});
                let body =
                    serde_json::from_value(value).map_err(|_| Fault::unavailable().connect())?;
                Ok(Response::new(body).with_header("X-Devbase-Trace-Id", trace))
            }
            Err(f) => {
                let mut detail = ErrorDetail {
                    reason: f.reason.into(),
                    trace_id: trace.clone(),
                    ..Default::default()
                };
                if let Some(retry) = f.retry {
                    detail.retry_after = Some(buffa_types::google::protobuf::Duration {
                        // Startup validation keeps retry delays within the protobuf Duration range, far
                        // below i64::MAX; the checked conversion only guards against future changes.
                        seconds: i64::try_from(retry.as_secs()).unwrap_or(i64::MAX),
                        nanos: retry.subsec_nanos() as i32,
                        ..Default::default()
                    })
                    .into();
                }
                let mut headers = axum::http::HeaderMap::new();
                headers.insert("X-Devbase-Trace-Id", trace.parse().unwrap());
                if let Some(retry) = f.retry {
                    headers.insert(
                        "Retry-After",
                        retry
                            .as_secs_f64()
                            .ceil()
                            .max(1.0)
                            .to_string()
                            .parse()
                            .unwrap(),
                    );
                }
                Err(f.connect().with_headers(headers).with_detail(
                    connectrpc::ErrorDetail::from_message("devbase.v1.ErrorDetail", &detail),
                ))
            }
        }
    }
    async fn execute<Q, R>(
        &self,
        authorization: &str,
        op: Op,
        request: Q,
        received: Option<usize>,
        trace: &str,
    ) -> Result<Value>
    where
        Q: Message + Serialize + DeserializeOwned,
        R: Message + Serialize + DeserializeOwned,
    {
        let q = serde_json::to_value(&request)?;
        let who = self.authenticate(authorization, op, &q).await?;
        let limiter = if op.management() {
            &self.management
        } else {
            &self.users
        };
        let rate_key = if op.management() {
            &who.tenant
        } else {
            &who.project
        };
        if let Err(retry) = limiter.allow(rate_key) {
            self.metrics
                .limits
                .with_label_values(&[if op.management() {
                    "management_api"
                } else {
                    "user_api"
                }])
                .inc();
            let mut f = Fault::new(
                connectrpc::ErrorCode::ResourceExhausted,
                7,
                "rate limit exceeded; retry later",
            );
            f.retry = Some(retry);
            return Err(f);
        }
        // The limit applies to the received (decompressed) message bytes, not to the decoded
        // message, so legal JSON padding and escape expansion count toward it.
        let Some(received) = received else {
            tracing::error!(trace_id=%trace,procedure=op.name(),"received message size was not captured");
            return Err(Fault::unavailable());
        };
        if received > size::MAX_MESSAGE_BYTES {
            tracing::warn!(trace_id=%trace,procedure=op.name(),received_bytes=received,limit_bytes=size::MAX_MESSAGE_BYTES,"request message exceeds size limit");
            return Err(Fault::invalid("request exceeds 64 KiB"));
        }
        validation::validate(op, &q)?;
        if op == Op::CreateProject
            && self
                .authority
                .states(std::slice::from_ref(&who.tenant))
                .await?[0]
                != server_common::authority::TenantState::Active
        {
            return Err(Fault::denied());
        }
        if !op.mutation() {
            let mut db = self.db.acquire().await?;
            return self
                .operation(&mut db, op, &q, &who, trace)
                .await
                .map(|v| v.body);
        }
        let key = server_common::string(&q, "idempotencyKey");
        let fingerprint = fingerprint::fingerprint(op, &request.encode_to_vec())?;
        let mut tx = self.db.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("{}|{key}", who.actor))
            .execute(&mut *tx)
            .await?;
        if let Some(key) = who.key {
            let parent: Option<uuid::Uuid> =
                sqlx::query_scalar("SELECT id FROM projects WHERE id=$1 FOR KEY SHARE")
                    .bind(server_common::uuid(&who.project)?)
                    .fetch_optional(&mut *tx)
                    .await?;
            if parent.is_none() {
                return Err(Fault::unauthenticated());
            }
            auth::usable(&mut tx, key, true).await?;
        }
        let now = server_common::now();
        sqlx::query(
            "DELETE FROM replays WHERE principal=$1 AND idempotency_key=$2 AND expires_at<=$3",
        )
        .bind(&who.actor)
        .bind(key)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if let Some(row)=sqlx::query("SELECT operation,tenant_id,project_id,fingerprint,response,scrubbed FROM replays WHERE principal=$1 AND idempotency_key=$2").bind(&who.actor).bind(key).fetch_optional(&mut *tx).await? {
            if row.get::<String,_>("operation")!=op.name()||row.get::<uuid::Uuid,_>("tenant_id").to_string()!=who.tenant||(!who.project.is_empty()&&row.get::<Option<uuid::Uuid>,_>("project_id").is_some_and(|id|id.to_string()!=who.project)){return Err(Fault::conflict())}
            if row.get::<bool,_>("scrubbed"){return Err(Fault::missing())}
            if row.get::<Vec<u8>,_>("fingerprint")!=fingerprint{return Err(Fault::conflict())}
            let bytes:Vec<u8>=row.get("response");let result=R::decode_from_slice(&bytes).map_err(|_|Fault::unavailable())?;return Ok(serde_json::to_value(result)?)
        }
        let result = self.operation(&mut tx, op, &q, &who, trace).await?;
        let mut snapshot = result.body.clone();
        snapshot.as_object_mut().unwrap().remove("secret");
        let snapshot: R = serde_json::from_value(snapshot)?;
        sqlx::query("INSERT INTO replays(principal,idempotency_key,operation,tenant_id,project_id,target_user_id,fingerprint,response,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(&who.actor).bind(key).bind(op.name()).bind(server_common::uuid(&who.tenant)?).bind(result.project.or_else(||server_common::uuid(&who.project).ok())).bind(result.user).bind(fingerprint).bind(snapshot.encode_to_vec()).bind(now).bind(now+chrono::Duration::hours(24)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(result.body)
    }
}
// The generated trait also permits borrowed responses; this implementation returns owned messages.
#[allow(refining_impl_trait)]
impl DevBaseService for Server {
    async fn create_project(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreateProjectRequest>,
    ) -> ServiceResult<CreateProjectResponse> {
        self.handle(ctx, Op::CreateProject, q.to_owned_message())
            .await
    }
    async fn get_project(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetProjectRequest>,
    ) -> ServiceResult<GetProjectResponse> {
        self.handle(ctx, Op::GetProject, q.to_owned_message()).await
    }
    async fn list_projects(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListProjectsRequest>,
    ) -> ServiceResult<ListProjectsResponse> {
        self.handle(ctx, Op::ListProjects, q.to_owned_message())
            .await
    }
    async fn update_project(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, UpdateProjectRequest>,
    ) -> ServiceResult<UpdateProjectResponse> {
        self.handle(ctx, Op::UpdateProject, q.to_owned_message())
            .await
    }
    async fn delete_project(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteProjectRequest>,
    ) -> ServiceResult<DeleteProjectResponse> {
        self.handle(ctx, Op::DeleteProject, q.to_owned_message())
            .await
    }
    async fn issue_api_key(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, IssueApiKeyRequest>,
    ) -> ServiceResult<IssueApiKeyResponse> {
        self.handle(ctx, Op::IssueApiKey, q.to_owned_message())
            .await
    }
    async fn get_api_key(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetApiKeyRequest>,
    ) -> ServiceResult<GetApiKeyResponse> {
        self.handle(ctx, Op::GetApiKey, q.to_owned_message()).await
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
    async fn create_end_user(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreateEndUserRequest>,
    ) -> ServiceResult<CreateEndUserResponse> {
        self.handle(ctx, Op::CreateEndUser, q.to_owned_message())
            .await
    }
    async fn get_end_user(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetEndUserRequest>,
    ) -> ServiceResult<GetEndUserResponse> {
        self.handle(ctx, Op::GetEndUser, q.to_owned_message()).await
    }
    async fn lookup_end_user(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, LookupEndUserRequest>,
    ) -> ServiceResult<LookupEndUserResponse> {
        self.handle(ctx, Op::LookupEndUser, q.to_owned_message())
            .await
    }
    async fn list_end_users(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListEndUsersRequest>,
    ) -> ServiceResult<ListEndUsersResponse> {
        self.handle(ctx, Op::ListEndUsers, q.to_owned_message())
            .await
    }
    async fn update_end_user(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, UpdateEndUserRequest>,
    ) -> ServiceResult<UpdateEndUserResponse> {
        self.handle(ctx, Op::UpdateEndUser, q.to_owned_message())
            .await
    }
    async fn delete_end_user(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteEndUserRequest>,
    ) -> ServiceResult<DeleteEndUserResponse> {
        self.handle(ctx, Op::DeleteEndUser, q.to_owned_message())
            .await
    }
}
