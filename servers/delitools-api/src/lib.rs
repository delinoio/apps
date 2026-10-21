// SPDX-License-Identifier: FSL-1.1-ALv2
//! DeliTools API service foundation: configuration, storage, authentication, API keys, operator
//! settings and inspection, background worker framework, observability and the extension points
//! for the paid execution pipeline.
pub mod admission;
pub mod auth;
pub mod authority;
pub mod config;
pub mod decimal;
pub mod delcore;
pub mod error;
pub mod exec;
pub mod execution;
pub mod gateway;
pub mod http;
pub mod json;
pub mod keys;
pub mod language;
pub mod platform;
pub mod pricing;
pub mod prompt;
pub mod readiness;
pub mod rpc;
pub mod schema;
pub mod settings;
pub mod store;
pub mod telemetry;
pub mod usage;
pub mod worker;

use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use delino_protocol::delitools::v1::*;
use error::{Fault, REQUEST_ID_HEADER, reason_label};
use execution::{CallContext, ExecutionService, NotYetAvailable};
use platform::Platform;
use std::{future::Future, sync::Arc, time::Instant};
use telemetry::Procedure;
use tracing::Instrument;

/// The Connect service implementation.
pub struct Server {
    pub platform: Arc<Platform>,
    execution: Arc<dyn ExecutionService>,
}

impl Server {
    /// A server whose execution RPCs answer `unimplemented` until an execution service is installed.
    pub fn new(platform: Arc<Platform>) -> Self {
        Self::with_execution(platform, Arc::new(NotYetAvailable))
    }

    /// Install the paid execution pipeline.
    pub fn with_execution(platform: Arc<Platform>, execution: Arc<dyn ExecutionService>) -> Self {
        Self {
            platform,
            execution,
        }
    }

    /// Request ID and credential for one call. The HTTP policy layer assigns the request ID before
    /// the handler runs; a call that bypasses it (direct service use) gets a fresh UUID v7. A
    /// client-supplied value is never trusted: the policy layer overwrites it.
    fn begin(&self, ctx: &RequestContext) -> (String, String) {
        let request_id = ctx
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .filter(|v| server_common::canonical_id(v))
            .map(str::to_owned)
            .unwrap_or_else(server_common::id);
        let authorization = ctx
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        (request_id, authorization)
    }

    fn call(&self, request_id: &str, authorization: &str) -> CallContext {
        CallContext {
            request_id: request_id.to_owned(),
            authorization: authorization.to_owned(),
            platform: self.platform.clone(),
        }
    }

    /// Run a state-changing RPC body to completion even when the caller stops waiting. The HTTP
    /// policy layer drops the handler future at the request deadline, and a drop between a
    /// database commit and the step that depends on it would leave them apart: a stored settings
    /// change that the live settings never receive, or a stored key without its audit record. The
    /// body runs on its own task; the caller still waits for it and only its wait can be
    /// cancelled. A client that gave up learns nothing about the outcome except `deadline_exceeded`
    /// and must read the state back (list keys, get settings) before retrying. Every dependency
    /// call inside the body has its own timeout, so a detached body cannot run unbounded.
    fn detached<R: Send + 'static>(
        body: impl Future<Output = Result<R, Fault>> + Send + 'static,
    ) -> impl Future<Output = Result<R, Fault>> {
        let task = tokio::spawn(body.in_current_span());
        async move { task.await.unwrap_or_else(|_| Err(Fault::internal())) }
    }

    /// Run one RPC body, record bounded metrics and structured logs, and convert failures to
    /// typed Connect errors. Logs carry the request ID, procedure and outcome only.
    async fn finish<R>(
        &self,
        procedure: Procedure,
        request_id: &str,
        body: impl Future<Output = Result<R, Fault>>,
    ) -> ServiceResult<R> {
        let start = Instant::now();
        let span = tracing::info_span!(
            "delitools.rpc",
            procedure = procedure.as_str(),
            request_id,
            outcome = tracing::field::Empty,
        );
        let result = body.instrument(span.clone()).await;
        let outcome = match &result {
            Ok(_) => "ok",
            Err(fault) => reason_label(fault.reason),
        };
        span.record("outcome", outcome);
        let metrics = &self.platform.metrics;
        metrics
            .requests
            .with_label_values(&[procedure.as_str(), outcome])
            .inc();
        metrics
            .duration
            .with_label_values(&[procedure.as_str()])
            .observe(start.elapsed().as_secs_f64());
        match result {
            Ok(body) => {
                tracing::info!(
                    procedure = procedure.as_str(),
                    request_id,
                    duration_ms = start.elapsed().as_millis() as u64,
                    "rpc completed"
                );
                Ok(Response::new(body).with_header(REQUEST_ID_HEADER, request_id.to_owned()))
            }
            Err(fault) => {
                tracing::warn!(
                    procedure = procedure.as_str(),
                    request_id,
                    reason = outcome,
                    code = fault.code.as_str(),
                    "rpc rejected"
                );
                Err(fault.into_connect(request_id))
            }
        }
    }
}

#[allow(refining_impl_trait)]
impl DeliToolsService for Server {
    async fn associate_generation(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, AssociateGenerationRequest>,
    ) -> ServiceResult<AssociateGenerationResponse> {
        let (id, authorization) = self.begin(&ctx);
        let q = request.to_owned_message();
        self.finish(
            Procedure::AssociateGeneration,
            &id,
            self.execution
                .associate_generation(self.call(&id, &authorization), q),
        )
        .await
    }
    async fn translate_text_array(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, TranslateTextArrayRequest>,
    ) -> ServiceResult<TranslateTextArrayResponse> {
        let (id, authorization) = self.begin(&ctx);
        let q = request.to_owned_message();
        self.finish(
            Procedure::TranslateTextArray,
            &id,
            self.execution
                .translate_text_array(self.call(&id, &authorization), q),
        )
        .await
    }
    async fn extract_data_from_text(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ExtractDataFromTextRequest>,
    ) -> ServiceResult<ExtractDataFromTextResponse> {
        let (id, authorization) = self.begin(&ctx);
        let q = request.to_owned_message();
        self.finish(
            Procedure::ExtractDataFromText,
            &id,
            self.execution
                .extract_data_from_text(self.call(&id, &authorization), q),
        )
        .await
    }
    async fn create_key(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, CreateKeyRequest>,
    ) -> ServiceResult<CreateKeyResponse> {
        let (id, authorization) = self.begin(&ctx);
        let q = request.to_owned_message();
        self.finish(
            Procedure::CreateKey,
            &id,
            Self::detached({
                let platform = self.platform.clone();
                let (id, authorization) = (id.clone(), authorization);
                async move { platform.create_key(&id, &authorization, q).await }
            }),
        )
        .await
    }
    async fn list_keys(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListKeysRequest>,
    ) -> ServiceResult<ListKeysResponse> {
        let (id, authorization) = self.begin(&ctx);
        let q = request.to_owned_message();
        self.finish(
            Procedure::ListKeys,
            &id,
            self.platform.list_keys(&id, &authorization, q),
        )
        .await
    }
    async fn revoke_key(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, RevokeKeyRequest>,
    ) -> ServiceResult<RevokeKeyResponse> {
        let (id, authorization) = self.begin(&ctx);
        let q = request.to_owned_message();
        self.finish(
            Procedure::RevokeKey,
            &id,
            Self::detached({
                let platform = self.platform.clone();
                let (id, authorization) = (id.clone(), authorization);
                async move { platform.revoke_key(&id, &authorization, q).await }
            }),
        )
        .await
    }
    async fn get_settings(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, GetSettingsRequest>,
    ) -> ServiceResult<GetSettingsResponse> {
        let (id, authorization) = self.begin(&ctx);
        self.finish(
            Procedure::GetSettings,
            &id,
            self.platform.get_settings(&id, &authorization),
        )
        .await
    }
    async fn update_settings(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, UpdateSettingsRequest>,
    ) -> ServiceResult<UpdateSettingsResponse> {
        let (id, authorization) = self.begin(&ctx);
        let q = request.to_owned_message();
        self.finish(
            Procedure::UpdateSettings,
            &id,
            Self::detached({
                let platform = self.platform.clone();
                let (id, authorization) = (id.clone(), authorization);
                async move { platform.update_settings(&id, &authorization, q).await }
            }),
        )
        .await
    }
    async fn inspect_request(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, InspectRequestRequest>,
    ) -> ServiceResult<InspectRequestResponse> {
        let (id, authorization) = self.begin(&ctx);
        let q = request.to_owned_message();
        self.finish(
            Procedure::InspectRequest,
            &id,
            self.platform.inspect_request(&id, &authorization, q),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// Dropping the caller of a state-changing RPC, as the request deadline does, must not stop
    /// the work that was already under way.
    #[tokio::test]
    async fn a_detached_body_finishes_after_its_caller_is_dropped() {
        let finished = Arc::new(AtomicBool::new(false));
        let waiting = Server::detached({
            let finished = finished.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                finished.store(true, Ordering::SeqCst);
                Ok::<_, Fault>(())
            }
        });
        let timed_out = tokio::time::timeout(Duration::from_millis(10), waiting).await;
        assert!(timed_out.is_err(), "the caller was dropped at its deadline");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            finished.load(Ordering::SeqCst),
            "the body ran to completion"
        );
    }

    #[tokio::test]
    async fn a_panicking_detached_body_is_an_internal_fault() {
        let result = Server::detached::<()>(async { panic!("boom") }).await;
        let fault: Fault = result.unwrap_err();
        assert_eq!(fault.reason, ErrorReason::ERROR_REASON_INTERNAL);
    }
}
