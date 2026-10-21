// SPDX-License-Identifier: FSL-1.1-ALv2
//! Extension point for the paid execution RPCs.
//!
//! The service foundation registers `TranslateTextArray`, `ExtractDataFromText` and
//! `AssociateGeneration` on the Connect router and forwards them to an [`ExecutionService`]. The
//! default [`NotYetAvailable`] implementation answers Connect `unimplemented` with an English
//! message and no `INTERNAL` reason. The execution pipeline (Gateway client, translator and
//! extractor execution, schema validation, idempotency admission, price snapshots, usage delivery
//! and reconciliation) supplies the real implementation through
//! [`crate::Server::with_execution`] and replaces this placeholder. Everything the pipeline needs
//! is reachable through [`CallContext::platform`]:
//!
//! * `platform.authenticate_execution` and `platform.reauthorize` for credential, tenant, key and
//!   billing checks (both credential types, fail closed);
//! * `platform.admission` for tenant rate and concurrency limits;
//! * `platform.settings()` for the live operator settings and `platform.metrics`;
//! * `platform.authenticate_operator` and `store::audit` for `AssociateGeneration`;
//! * the `catalog_prices`, `requests`, `attempts` and `idempotency_records` tables created by the
//!   migrations, none of which may hold input text, schemas or results.
use crate::{error::Fault, platform::Platform};
use async_trait::async_trait;
use delino_protocol::delitools::v1::*;
use std::sync::Arc;

/// Per-call data supplied by the transport. `request_id` is the UUID v7 returned to the caller in
/// the response or error detail; an execution creating a request record uses it as the record ID.
pub struct CallContext {
    pub request_id: String,
    /// Raw `Authorization` header value; empty when absent.
    pub authorization: String,
    pub platform: Arc<Platform>,
}

#[async_trait]
pub trait ExecutionService: Send + Sync + 'static {
    async fn translate_text_array(
        &self,
        call: CallContext,
        request: TranslateTextArrayRequest,
    ) -> Result<TranslateTextArrayResponse, Fault>;
    async fn extract_data_from_text(
        &self,
        call: CallContext,
        request: ExtractDataFromTextRequest,
    ) -> Result<ExtractDataFromTextResponse, Fault>;
    /// Operator-only: associate a Gateway generation ID with an attempt. Requires a reason and an
    /// audit record, accepts no tokens, prices or fees and never replays inference.
    async fn associate_generation(
        &self,
        call: CallContext,
        request: AssociateGenerationRequest,
    ) -> Result<AssociateGenerationResponse, Fault>;
}

/// Placeholder until the execution pipeline is installed.
pub struct NotYetAvailable;

fn unavailable(method: &str) -> Fault {
    Fault::not_implemented(format!("{method} is not available in this build"))
}

#[async_trait]
impl ExecutionService for NotYetAvailable {
    async fn translate_text_array(
        &self,
        _call: CallContext,
        _request: TranslateTextArrayRequest,
    ) -> Result<TranslateTextArrayResponse, Fault> {
        Err(unavailable("TranslateTextArray"))
    }
    async fn extract_data_from_text(
        &self,
        _call: CallContext,
        _request: ExtractDataFromTextRequest,
    ) -> Result<ExtractDataFromTextResponse, Fault> {
        Err(unavailable("ExtractDataFromText"))
    }
    async fn associate_generation(
        &self,
        _call: CallContext,
        _request: AssociateGenerationRequest,
    ) -> Result<AssociateGenerationResponse, Fault> {
        Err(unavailable("AssociateGeneration"))
    }
}
