// SPDX-License-Identifier: FSL-1.1-ALv2
//! DeliTools API server: paid, synchronous translation and structured extraction over Connect.
//!
//! The crate owns Vercel AI Gateway execution, the model allowlist and its meter mapping,
//! provider usage interpretation, and a durable usage outbox that reports measured token usage
//! to Delcore with `IngestEvent`. Delcore remains generic: it knows services, capabilities,
//! meters, prices and events, never models or Gateway behavior.
//!
//! Request flow (see [`Server`]): authenticate and authorize (`auth`), admit by tenant rate and
//! concurrency (`limits`), validate (`translate` / `extract`), verify model prices (`pricing`),
//! then execute with retries (`exec`) while recording confirmed usage (`outbox`).
//!
//! Privacy: submitted text, schemas, outputs and credentials are never persisted or logged.
//! PostgreSQL holds only the usage outbox and the schema version table.

pub mod auth;
pub mod config;
pub mod delcore;
pub mod error;
pub mod exec;
pub mod extract;
pub mod gateway;
pub mod http;
pub mod limits;
pub mod models;
pub mod outbox;
pub mod pricing;
pub mod retry;
pub mod store;
pub mod telemetry;
pub mod translate;
pub mod usage;

use auth::Authenticator;
use buffa::Message;
use chrono::Utc;
use config::Config;
use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use delcore::DelcoreClient;
use delino_protocol::delitools::v1::*;
use error::ApiError;
use exec::{Job, Output, Task};
use gateway::Gateway;
use limits::TenantGate;
use outbox::{Delivery, Outbox};
use pricing::PriceCheck;
use server_common::authority::Authority;
use sqlx::PgPool;
use std::sync::Arc;
use telemetry::{Metrics, Procedure};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use uuid::Uuid;

/// Response header carrying the request ID on every reply.
pub const REQUEST_ID_HEADER: &str = "X-Delitools-Request-Id";

/// Shared state of the running server.
pub struct Inner {
    pub config: Config,
    pub metrics: Arc<Metrics>,
    pub db: PgPool,
    pub outbox: Outbox,
    pub delcore: Arc<DelcoreClient>,
    pub(crate) auth: Authenticator,
    pub(crate) prices: PriceCheck,
    pub(crate) gateway: Gateway,
    pub(crate) gate: TenantGate,
    /// Service-wide fence. Cancelled once this process no longer owns the instance lock; every
    /// job holds a child token, so a job admitted while the fence closes is already cancelled.
    pub(crate) fence: CancellationToken,
}

/// The DeliTools Connect service. Cloning shares the same state.
#[derive(Clone)]
pub struct Server {
    inner: Arc<Inner>,
}

impl std::ops::Deref for Server {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.inner
    }
}

/// Cancels the execution when the handler future is dropped, for example on client disconnect.
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl Server {
    /// Builds the server from validated configuration and a migrated database.
    pub fn new(db: PgPool, config: Config) -> anyhow::Result<Self> {
        let delcore = Arc::new(DelcoreClient::new(
            &config.delcore,
            config.dependency_timeout,
        )?);
        let authority = Authority::new(
            reqwest::Client::builder()
                .timeout(config.dependency_timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            config.delcore.url.clone(),
            config.delcore.token_url.clone(),
            config.delcore.resource.clone(),
            config.delcore.client_id.clone(),
            config.delcore.client_secret.expose().to_owned(),
        );
        Ok(Self {
            inner: Arc::new(Inner {
                metrics: Arc::new(Metrics::new()),
                outbox: Outbox::new(db.clone()),
                auth: Authenticator {
                    authority,
                    delcore: delcore.clone(),
                },
                prices: PriceCheck::new(delcore.clone(), config.price_cache_ttl),
                gateway: Gateway::new(&config.gateway)?,
                gate: TenantGate::new(&config.limits),
                fence: CancellationToken::new(),
                delcore,
                db,
                config,
            }),
        })
    }

    /// Fences the service after the instance lock is lost: no new provider attempt, retry or
    /// usage delivery starts, and unconfirmed provider calls are canceled at once. Usage that a
    /// provider already confirmed is still stored (bounded by `DEPENDENCY_TIMEOUT`). Idempotent.
    pub fn fence(&self) {
        self.fence.cancel();
    }

    /// The usage delivery worker bound to this server's state.
    pub fn delivery(&self) -> Arc<Delivery> {
        Arc::new(Delivery {
            fence: self.fence.clone(),
            outbox: self.outbox.clone(),
            delcore: self.delcore.clone(),
            settings: self.config.outbox.clone(),
            metrics: self.metrics.clone(),
            clock: outbox::system_clock(),
        })
    }

    /// Handles one RPC. Every outcome carries the request ID.
    async fn handle<R>(
        &self,
        ctx: RequestContext,
        procedure: Procedure,
        tenant_id: String,
        prepare: impl FnOnce(&Config) -> Result<(Task, models::ModelEntry), ApiError>,
        finish: impl FnOnce(String, Output) -> R,
    ) -> ServiceResult<R>
    where
        R: Message + serde::Serialize + 'static,
    {
        let request_id = Uuid::now_v7();
        let started = std::time::Instant::now();
        let span = tracing::info_span!("delitools.request", request_id = %request_id, procedure = procedure.name());
        let authorization = ctx
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let result = self
            .admit_and_run(&authorization, procedure, &tenant_id, request_id, prepare)
            .instrument(span.clone())
            .await;
        let outcome = result.as_ref().err().map_or("ok", ApiError::label);
        self.metrics
            .requests
            .with_label_values(&[procedure.name(), outcome])
            .inc();
        self.metrics
            .duration
            .with_label_values(&[procedure.name()])
            .observe(started.elapsed().as_secs_f64());
        if let Err(error) = &result {
            use delino_protocol::delitools::v1::ErrorReason as R;
            match error.reason {
                R::ERROR_REASON_INVALID_INPUT
                | R::ERROR_REASON_UNAUTHENTICATED
                | R::ERROR_REASON_PERMISSION_DENIED
                | R::ERROR_REASON_BILLING_BLOCKED
                | R::ERROR_REASON_LIMIT_EXCEEDED
                | R::ERROR_REASON_RATE_LIMITED
                | R::ERROR_REASON_MODEL_UNAVAILABLE
                | R::ERROR_REASON_UNSUPPORTED_SCHEMA => {
                    self.metrics
                        .denied
                        .with_label_values(&[error.label()])
                        .inc();
                }
                _ => self
                    .metrics
                    .failures
                    .with_label_values(&[procedure.name(), error.label()])
                    .inc(),
            }
        }
        span.in_scope(|| {
            tracing::info!(
                category = "request",
                outcome,
                duration_ms = started.elapsed().as_millis() as u64,
                "request completed"
            )
        });
        let id = request_id.to_string();
        match result {
            Ok(output) => {
                Ok(Response::new(finish(id.clone(), output)).with_header(REQUEST_ID_HEADER, id))
            }
            Err(error) => Err(error.into_connect(&id)),
        }
    }

    async fn admit_and_run(
        &self,
        authorization: &str,
        procedure: Procedure,
        tenant_id: &str,
        request_id: Uuid,
        prepare: impl FnOnce(&Config) -> Result<(Task, models::ModelEntry), ApiError>,
    ) -> Result<Output, ApiError> {
        // A configuration that bypassed validation must fail the request, never panic.
        let Some(deadline) = self
            .config
            .limits
            .deadline_from(tokio::time::Instant::now())
        else {
            tracing::error!(
                category = "deadline",
                "the execution timeout exceeds the monotonic clock range"
            );
            return Err(ApiError::internal());
        };
        if self.fence.is_cancelled() {
            return Err(ApiError::canceled());
        }
        let fence = self.fence.clone();
        let admitted = tokio::time::timeout_at(deadline, async {
            tokio::select! {
                biased;
                _ = fence.cancelled() => Err(ApiError::canceled()),
                admitted = async {
                    let principal = self
                        .auth
                        .authenticate(authorization, tenant_id, procedure.capability())
                        .await?;
                    let permit = self.gate.admit(&principal.tenant_id)?;
                    let (task, model) = prepare(&self.config)?;
                    self.prices.verify(&model).await?;
                    Ok::<_, ApiError>((principal, permit, task, model))
                } => admitted,
            }
        })
        .await
        .map_err(|_| ApiError::timeout())?;
        let (principal, permit, task, model) = admitted?;
        tracing::info!(category = "admitted", tenant_id = %principal.tenant_id, credential = principal.kind(), "request admitted");
        // A child of the fence: a job admitted while the fence closes starts out canceled.
        let cancel = self.fence.child_token();
        let _guard = CancelOnDrop(cancel.clone());
        let job = Job {
            request_id,
            principal,
            task,
            model,
            started_at: server_common::now(),
            deadline,
            cancel,
            permit,
        };
        // The task outlives a dropped handler so confirmed usage is stored after cancellation.
        let task = tokio::spawn(self.inner.clone().execute(job).in_current_span());
        match task.await {
            Ok(result) => result,
            Err(_) => {
                tracing::error!(category = "execution_task", "the execution task failed");
                Err(ApiError::internal())
            }
        }
    }
}

// The generated trait also permits borrowed responses; this implementation returns owned messages.
#[allow(refining_impl_trait)]
impl DelitoolsService for Server {
    async fn translate(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, TranslateRequest>,
    ) -> ServiceResult<TranslateResponse> {
        let q = q.to_owned_message();
        let tenant = q.tenant_id.clone();
        self.handle(
            ctx,
            Procedure::Translate,
            tenant,
            move |config| {
                let translation = translate::validate(
                    &q.texts,
                    &q.target_language,
                    &q.source_language,
                    &q.model,
                    &config.limits,
                    &config.models,
                )?;
                let model = translation.model.clone();
                Ok((Task::Translate(translation), model))
            },
            |request_id, output| match output {
                Output::Translations(translations) => TranslateResponse {
                    request_id,
                    translations,
                    ..Default::default()
                },
                Output::Json(_) => unreachable!("translation yields translations"),
            },
        )
        .await
    }

    async fn extract(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ExtractRequest>,
    ) -> ServiceResult<ExtractResponse> {
        let q = q.to_owned_message();
        let tenant = q.tenant_id.clone();
        self.handle(
            ctx,
            Procedure::Extract,
            tenant,
            move |config| {
                let extraction = extract::validate(&q.text, &q.json_schema, &config.limits)?;
                let model = config
                    .models
                    .get(models::EXTRACTION_MODEL)
                    .cloned()
                    .ok_or_else(ApiError::model_not_allowed)?;
                Ok((Task::Extract(extraction), model))
            },
            |request_id, output| match output {
                Output::Json(json) => ExtractResponse {
                    request_id,
                    json,
                    ..Default::default()
                },
                Output::Translations(_) => unreachable!("extraction yields JSON"),
            },
        )
        .await
    }
}

impl Server {
    /// Execution slots currently held by a tenant. Exposed for tests and diagnostics.
    pub fn in_flight(&self, tenant: &str) -> usize {
        self.gate.in_flight(tenant)
    }
}

/// Current time truncated to microseconds, matching PostgreSQL precision.
pub fn now() -> chrono::DateTime<Utc> {
    server_common::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::Procedure;

    /// A request-time deadline that does not fit the clock must produce the internal error,
    /// even when the transport layer is bypassed and the configuration skipped validation.
    #[tokio::test]
    async fn an_unrepresentable_deadline_is_an_internal_error_not_a_panic() {
        let dir = config::tests::tempdir();
        let env = config::tests::base_env(&dir);
        let mut config = Config::from_lookup(&|key| env.get(key).cloned()).unwrap();
        config.limits.execution_timeout = std::time::Duration::MAX;
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://nobody:nobody@127.0.0.1:1/none")
            .unwrap();
        let server = Server::new(db, config).unwrap();
        let result = server
            .admit_and_run("", Procedure::Translate, "tenant", Uuid::now_v7(), |_| {
                unreachable!("the request must fail before preparation")
            })
            .await;
        let Err(error) = result else {
            panic!("the request must fail");
        };
        assert_eq!(error.code, connectrpc::ErrorCode::Internal);
    }
}
