// SPDX-License-Identifier: FSL-1.1-ALv2
//! The paid execution pipeline behind `TranslateTextArray`, `ExtractDataFromText` and
//! `AssociateGeneration`.
//!
//! Flow of one call: authenticate and authorize (both credential types, fail closed) → validate
//! the input against the operator settings and the schema subset (no AI call yet) → tenant
//! admission → resolve the allowlisted model and freeze its catalog price → claim the optional
//! idempotency key and create the request record → run the attempt loop in a task that outlives a
//! disconnecting client → persist the terminal state.
//!
//! The attempt loop retries only transient provider failures, never malformed output, never on
//! another model, re-authorizes before every retry, persists each attempt before its call and
//! hands measured usage to the delivery path. A client disconnect or the execution deadline
//! cancels the in-flight HTTP call and stops retries; usage of the cancelled call is reconciled in
//! the background. Inputs, schemas, results and provider bodies are never stored or logged.
use crate::{
    error::{Fault, Result},
    execution::{CallContext, ExecutionService},
    gateway::{ChatRequest, EXTRACTION_MODEL, FailureKind, Gateway, GenerationSink, Usage},
    language,
    platform::{ExecutionPrincipal, Platform},
    pricing::PriceSnapshot,
    prompt::{self, Prompt},
    schema::{Output as SchemaOutput, Schema},
    settings::Settings,
    store::{
        self,
        audit::{self, Action, AuditEntry, EventCategory, Severity},
        exec::{self as db, Association, IdempotencyClaim, NewAttempt, NewRequest, Start, State},
    },
    telemetry::{FailureKind as Failure, LimitKind, Metrics},
    usage::UsageService,
};
use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use delino_protocol::delitools::v1::*;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, time::Instant};
use uuid::Uuid;

/// Longest accepted caller idempotency key.
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;
/// First pause before an AI retry; doubles per retry up to the cap. Bounded by the deadline.
const AI_RETRY_BASE: Duration = Duration::from_millis(100);
const AI_RETRY_CAP: Duration = Duration::from_secs(2);

/// What a validated call needs for execution. Holds the input only in memory.
struct Plan {
    capability: Capability,
    model: String,
    input_hash: [u8; 32],
    key: Option<String>,
    prompt: Prompt,
    reader: Reader,
}

enum Reader {
    Translation { count: usize },
    Extraction(Box<Schema>),
}

enum Done {
    Texts(Vec<String>),
    Data(String),
}

/// Why reading the model output did not produce a result.
enum Rejected {
    Impossible,
    Malformed,
}

impl Reader {
    fn read(&self, content: &str) -> std::result::Result<Done, Rejected> {
        match self {
            Reader::Translation { count } => prompt::read_translations(content, *count)
                .map(Done::Texts)
                .ok_or(Rejected::Malformed),
            Reader::Extraction(schema) => match schema.read_output(content) {
                SchemaOutput::Extracted(json) => Ok(Done::Data(json)),
                SchemaOutput::Impossible => Err(Rejected::Impossible),
                SchemaOutput::Malformed => Err(Rejected::Malformed),
            },
        }
    }
}

pub struct Executor {
    platform: Arc<Platform>,
    gateway: Arc<Gateway>,
    usage: Arc<UsageService>,
}

impl Executor {
    pub fn new(
        platform: Arc<Platform>,
        gateway: Arc<Gateway>,
        usage: Arc<UsageService>,
    ) -> Arc<Self> {
        Arc::new(Self {
            platform,
            gateway,
            usage,
        })
    }
}

fn parse_request_id(id: &str) -> Uuid {
    Uuid::parse_str(id).unwrap_or_else(|_| Uuid::now_v7())
}

/// Optional caller idempotency key: empty means none.
fn idempotency_key(key: &str) -> Result<Option<String>> {
    if key.is_empty() {
        return Ok(None);
    }
    if key.len() > MAX_IDEMPOTENCY_KEY_BYTES || !key.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(Fault::invalid_input(
            "idempotencyKey must be 1 to 256 visible ASCII characters",
        ));
    }
    Ok(Some(key.to_owned()))
}

fn hash_fields(domain: &str, tenant: Uuid, fields: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"delitools.idempotency.v1\0");
    hasher.update(domain.as_bytes());
    hasher.update(tenant.as_bytes());
    for field in fields {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    hasher.finalize().into()
}

fn limit(metrics: &Metrics, kind: LimitKind, message: &str) -> Fault {
    metrics.limit(kind);
    Fault::limit(message)
}

fn validate_translation(
    metrics: &Metrics,
    settings: &Settings,
    tenant: Uuid,
    request: &TranslateTextArrayRequest,
) -> Result<Plan> {
    let key = idempotency_key(&request.idempotency_key)?;
    if request.model.is_empty() || !settings.translation_models.contains(&request.model) {
        return Err(Fault::invalid_model("model is not enabled for translation"));
    }
    if request.output_language.is_empty() {
        return Err(Fault::invalid_input("outputLanguage is required"));
    }
    let output = language::normalize(&request.output_language)
        .ok_or_else(|| Fault::invalid_input("outputLanguage must be a BCP 47 language tag"))?;
    let input =
        if request.input_language.is_empty() {
            None
        } else {
            Some(language::normalize(&request.input_language).ok_or_else(|| {
                Fault::invalid_input("inputLanguage must be a BCP 47 language tag")
            })?)
        };
    // Detection may legitimately find the output language; only explicit duplicates are invalid.
    if input.as_deref() == Some(output.as_str()) {
        return Err(Fault::invalid_input(
            "inputLanguage and outputLanguage must differ",
        ));
    }
    if request.texts.is_empty() {
        return Err(Fault::invalid_input("texts must not be empty"));
    }
    if request.texts.len() > settings.max_translation_items as usize {
        return Err(limit(
            metrics,
            LimitKind::ItemCount,
            "texts exceeds the configured item count limit",
        ));
    }
    let bytes: usize = request.texts.iter().map(String::len).sum();
    if bytes > settings.max_input_bytes as usize {
        return Err(limit(
            metrics,
            LimitKind::InputSize,
            "texts exceeds the configured input size limit",
        ));
    }
    if request.texts.iter().any(|text| text.trim().is_empty()) {
        return Err(Fault::invalid_input("texts must not contain empty items"));
    }
    let mut fields: Vec<&[u8]> = vec![
        output.as_bytes(),
        input.as_deref().unwrap_or("").as_bytes(),
        request.model.as_bytes(),
    ];
    fields.extend(request.texts.iter().map(|t| t.as_bytes()));
    Ok(Plan {
        capability: Capability::CAPABILITY_TRANSLATOR,
        model: request.model.clone(),
        input_hash: hash_fields("translate", tenant, &fields),
        key,
        prompt: prompt::translation_prompt(
            &request.output_language,
            (!request.input_language.is_empty()).then_some(request.input_language.as_str()),
            &request.texts,
        ),
        reader: Reader::Translation {
            count: request.texts.len(),
        },
    })
}

fn validate_extraction(
    metrics: &Metrics,
    settings: &Settings,
    tenant: Uuid,
    request: &ExtractDataFromTextRequest,
) -> Result<Plan> {
    let key = idempotency_key(&request.idempotency_key)?;
    if request.text.trim().is_empty() {
        return Err(Fault::invalid_input("text must not be empty"));
    }
    if request.text.len() + request.json_schema.len() > settings.max_input_bytes as usize {
        return Err(limit(
            metrics,
            LimitKind::InputSize,
            "text and jsonSchema exceed the configured input size limit",
        ));
    }
    // The complete schema validation happens before any AI call.
    let schema = Schema::parse(&request.json_schema)
        .map_err(|error| Fault::invalid_schema(error.to_string()))?;
    let envelope = schema.envelope();
    Ok(Plan {
        capability: Capability::CAPABILITY_EXTRACTOR,
        model: EXTRACTION_MODEL.to_owned(),
        input_hash: hash_fields(
            "extract",
            tenant,
            &[request.text.as_bytes(), request.json_schema.as_bytes()],
        ),
        key,
        prompt: prompt::extraction_prompt(&request.text, envelope),
        reader: Reader::Extraction(Box::new(schema)),
    })
}

/// Cancels the pipeline when the call is dropped before it finishes (client disconnect).
struct CancelOnDrop {
    tx: watch::Sender<bool>,
    armed: bool,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.tx.send(true);
        }
    }
}

/// Persists the generation ID the moment the first chunk arrives.
struct AttemptSink {
    pool: PgPool,
    attempt: Uuid,
}
#[async_trait]
impl GenerationSink for AttemptSink {
    async fn captured(&self, generation_id: &str) {
        if !db::set_generation(&self.pool, self.attempt, generation_id, store::now()).await {
            tracing::warn!(
                category = "generation_not_stored",
                attempt_id = %self.attempt,
                "generation identifier could not be stored"
            );
        }
    }
}

/// Everything the attempt loop owns.
struct Run {
    platform: Arc<Platform>,
    gateway: Arc<Gateway>,
    usage: Arc<UsageService>,
    request_id: Uuid,
    principal: ExecutionPrincipal,
    settings: Arc<Settings>,
    price: PriceSnapshot,
    plan: Plan,
    deadline: Instant,
}

/// How the loop ended; decides the stored request state.
enum Ending {
    Success(Done),
    /// A definitive failure of the request.
    Failed(Fault),
    /// The call was cancelled or timed out; `ambiguous` means an AI call was in flight.
    Interrupted {
        fault: Fault,
        ambiguous: bool,
    },
}

enum Wait {
    Elapsed,
    Canceled,
    Timeout,
}

async fn wait(cancel: &mut watch::Receiver<bool>, deadline: Instant, pause: Duration) -> Wait {
    if *cancel.borrow() {
        return Wait::Canceled;
    }
    tokio::select! {
        () = tokio::time::sleep(pause) => Wait::Elapsed,
        () = canceled(cancel) => Wait::Canceled,
        () = tokio::time::sleep_until(deadline) => Wait::Timeout,
    }
}

/// Resolves when cancellation is requested; never resolves if the sender went away uncancelled.
async fn canceled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn provider_fault() -> Fault {
    Fault::new(
        ErrorReason::ERROR_REASON_PROVIDER,
        "the AI provider could not complete the request",
    )
}

fn timeout_fault() -> Fault {
    Fault::new(
        ErrorReason::ERROR_REASON_TIMEOUT,
        "the request exceeded the configured execution deadline",
    )
}

fn canceled_fault() -> Fault {
    Fault::new(
        ErrorReason::ERROR_REASON_CANCELED,
        "the request was canceled",
    )
}

impl Run {
    async fn attempt_loop(&self, cancel: &mut watch::Receiver<bool>) -> Ending {
        let pool = &self.platform.store.pool;
        let metrics = &self.platform.metrics;
        let mut last = provider_fault();
        for number in 0..=self.settings.max_ai_retries {
            if number > 0 {
                metrics.ai_retries.inc();
                let pause = (AI_RETRY_BASE * 2_u32.saturating_pow(number - 1)).min(AI_RETRY_CAP);
                match wait(cancel, self.deadline, pause).await {
                    Wait::Elapsed => {}
                    Wait::Canceled => {
                        return Ending::Interrupted {
                            fault: canceled_fault(),
                            ambiguous: false,
                        };
                    }
                    Wait::Timeout => {
                        return Ending::Interrupted {
                            fault: timeout_fault(),
                            ambiguous: false,
                        };
                    }
                }
                // A revoked key or a removed permission stops further attempts. A call that is
                // already running is never interrupted by this check.
                if let Err(fault) = self.platform.reauthorize(&self.principal).await {
                    return Ending::Failed(fault);
                }
            }
            // Price lookup, request creation and the retry pause or re-authorization may have used
            // up the deadline or seen a cancellation. No provider call is in flight yet, so a
            // definitive timeout or cancellation is returned and nothing billable is started.
            if *cancel.borrow() {
                return Ending::Interrupted {
                    fault: canceled_fault(),
                    ambiguous: false,
                };
            }
            if Instant::now() >= self.deadline {
                return Ending::Interrupted {
                    fault: timeout_fault(),
                    ambiguous: false,
                };
            }

            let attempt = Uuid::now_v7();
            let occurred_at = store::now();
            if db::insert_attempt(
                pool,
                &NewAttempt {
                    id: attempt,
                    request_id: self.request_id,
                    tenant_id: self.principal.tenant_id,
                    capability: self.plan.capability,
                    price: &self.price,
                    occurred_at,
                },
            )
            .await
            .is_err()
            {
                return Ending::Failed(Fault::internal());
            }
            let user = attempt.to_string();
            let sink = AttemptSink {
                pool: pool.clone(),
                attempt,
            };
            let request = ChatRequest {
                model: &self.price.model,
                route: &self.price.route,
                user: &user,
                system: self.plan.prompt.system,
                user_message: &self.plan.prompt.user,
                schema_name: self.plan.prompt.schema_name,
                schema: &self.plan.prompt.schema,
            };
            // Boxed so that dropping it really drops the future and with it the HTTP call.
            let mut chat = Box::pin(self.gateway.chat(&request, &sink));
            let reconcile_delay =
                ChronoDuration::seconds(i64::from(self.settings.delivery_retry_seconds));
            let outcome = tokio::select! {
                outcome = &mut chat => Some(outcome),
                () = canceled(cancel) => None,
                () = tokio::time::sleep_until(self.deadline) => None,
            };
            let Some(outcome) = outcome else {
                // Dropping the call closes the connection and cancels the in-flight request.
                drop(chat);
                let _ = db::finish_attempt(
                    pool,
                    attempt,
                    Usage::Unknown,
                    reconcile_delay,
                    store::now(),
                )
                .await;
                metrics
                    .ai_attempts
                    .with_label_values(&["interrupted"])
                    .inc();
                let timed_out = !*cancel.borrow();
                return Ending::Interrupted {
                    fault: if timed_out {
                        timeout_fault()
                    } else {
                        canceled_fault()
                    },
                    ambiguous: true,
                };
            };
            if db::finish_attempt(pool, attempt, outcome.usage, reconcile_delay, store::now())
                .await
                .is_err()
            {
                tracing::error!(
                    category = "attempt_not_recorded",
                    attempt_id = %attempt,
                    "attempt usage could not be recorded"
                );
            }
            match outcome.result {
                Ok(content) => match self.plan.reader.read(&content) {
                    Ok(done) => {
                        metrics.ai_attempts.with_label_values(&["success"]).inc();
                        return Ending::Success(done);
                    }
                    Err(Rejected::Impossible) => {
                        metrics
                            .ai_attempts
                            .with_label_values(&["extraction_impossible"])
                            .inc();
                        return Ending::Failed(Fault::new(
                            ErrorReason::ERROR_REASON_EXTRACTION_IMPOSSIBLE,
                            "the source text does not contain the information required by the schema",
                        ));
                    }
                    Err(Rejected::Malformed) => {
                        metrics
                            .ai_attempts
                            .with_label_values(&["invalid_output"])
                            .inc();
                        metrics.failure(Failure::Provider);
                        return Ending::Failed(invalid_output());
                    }
                },
                Err(failure) => match failure.kind {
                    FailureKind::Transient => {
                        metrics.ai_attempts.with_label_values(&["transient"]).inc();
                        metrics.failure(Failure::Provider);
                        tracing::warn!(
                            category = "ai_attempt_failed",
                            request_id = %self.request_id,
                            attempt_id = %attempt,
                            cause = failure.category,
                            number,
                            "transient AI failure"
                        );
                        last = provider_fault();
                    }
                    FailureKind::Permanent => {
                        metrics.ai_attempts.with_label_values(&["permanent"]).inc();
                        metrics.failure(Failure::Provider);
                        tracing::warn!(
                            category = "ai_attempt_failed",
                            request_id = %self.request_id,
                            attempt_id = %attempt,
                            cause = failure.category,
                            "non-transient AI failure"
                        );
                        return Ending::Failed(provider_fault());
                    }
                    FailureKind::Output => {
                        metrics
                            .ai_attempts
                            .with_label_values(&["invalid_output"])
                            .inc();
                        metrics.failure(Failure::Provider);
                        tracing::warn!(
                            category = "ai_attempt_failed",
                            request_id = %self.request_id,
                            attempt_id = %attempt,
                            cause = failure.category,
                            "unusable AI output"
                        );
                        return Ending::Failed(invalid_output());
                    }
                },
            }
        }
        audit::record_event(
            pool,
            EventCategory::ProviderFailure,
            Severity::Warn,
            Some(self.request_id),
            self.request_id,
            serde_json::json!({"cause": "retries_exhausted"}),
        )
        .await;
        Ending::Failed(last)
    }
}

fn invalid_output() -> Fault {
    Fault::new(
        ErrorReason::ERROR_REASON_INVALID_OUTPUT,
        "the AI provider returned output that could not be accepted",
    )
}

impl Executor {
    /// Authorized, validated execution shared by both methods.
    async fn execute(
        &self,
        call: &CallContext,
        principal: ExecutionPrincipal,
        settings: Arc<Settings>,
        plan: Plan,
    ) -> Result<Done> {
        let platform = &self.platform;
        let request_id = parse_request_id(&call.request_id);
        let started = Instant::now();
        // Authorization already succeeded, so duplicate state may be disclosed. Answer a known
        // duplicate before admission: it does no AI work and must not consume tenant budget.
        if let Some(key) = plan.key.as_deref()
            && let Some(existing) = db::peek_duplicate(
                &platform.store.pool,
                principal.tenant_id,
                plan.capability,
                key,
                store::now(),
            )
            .await?
        {
            return Err(duplicate_fault(&existing, &plan.input_hash));
        }
        let permit = platform
            .admission
            .admit(principal.tenant_id, &settings, &platform.metrics)?;

        // Only an operator-allowlisted model with a synchronized public price can execute.
        let price = db::latest_price(&platform.store.pool, &plan.model, store::now())
            .await?
            .ok_or_else(|| {
                platform.metrics.failure(Failure::MissingPrice);
                tracing::warn!(
                    category = "price_unavailable",
                    request_id = %request_id,
                    "new execution rejected: no catalog price"
                );
                Fault::price_unavailable()
            })?;

        let claim = plan.key.as_deref().map(|key| IdempotencyClaim {
            key,
            input_hash: plan.input_hash,
        });
        let start = db::start_request(
            &platform.store.pool,
            &NewRequest {
                id: request_id,
                principal: &principal,
                model: &plan.model,
                price: &price,
            },
            claim.as_ref(),
            store::now(),
        )
        .await?;
        if let Start::Duplicate(existing) = start {
            return Err(duplicate_fault(&existing, &plan.input_hash));
        }

        let (tx, rx) = watch::channel(false);
        let mut guard = CancelOnDrop { tx, armed: true };
        let tenant = principal.tenant_id;
        let capability = plan.capability;
        let key = plan.key.clone();
        let panic_key = plan.key.clone();
        let run = Run {
            platform: platform.clone(),
            gateway: self.gateway.clone(),
            usage: self.usage.clone(),
            request_id,
            principal,
            settings: settings.clone(),
            price,
            plan,
            deadline: started + Duration::from_secs(u64::from(settings.execution_timeout_seconds)),
        };
        let pipeline = tokio::spawn(async move {
            // The permit is released when the pipeline ends, not when the client leaves.
            let _permit = permit;
            let mut cancel = rx;
            let ending = run.attempt_loop(&mut cancel).await;
            let (state, reason, result) = match ending {
                Ending::Success(done) => (
                    State::Completed,
                    ErrorReason::ERROR_REASON_UNSPECIFIED,
                    Ok(done),
                ),
                Ending::Failed(fault) => (State::Completed, fault.reason, Err(fault)),
                Ending::Interrupted { fault, ambiguous } => {
                    // An interrupted call may have consumed tokens, so its outcome is unknown
                    // and a duplicate is not re-executed.
                    let state = if ambiguous {
                        State::Unknown
                    } else {
                        State::Completed
                    };
                    let reason = if ambiguous {
                        ErrorReason::ERROR_REASON_UNSPECIFIED
                    } else {
                        fault.reason
                    };
                    (state, reason, Err(fault))
                }
            };
            if let Err(fault) = db::finish_request(
                &run.platform.store.pool,
                &db::Terminal {
                    request_id: run.request_id,
                    tenant_id: tenant,
                    capability,
                    idempotency_key: key.as_deref(),
                    state,
                    reason,
                },
                store::now(),
            )
            .await
            {
                tracing::error!(
                    category = "request_not_finalized",
                    request_id = %run.request_id,
                    reason = crate::error::reason_label(fault.reason),
                    "request terminal state could not be stored"
                );
            }
            run.usage.deliver_request(run.request_id);
            result
        });
        let joined = pipeline.await;
        guard.armed = false;
        match joined {
            Ok(result) => result,
            Err(_) => {
                tracing::error!(category = "pipeline_panic", request_id = %request_id, "execution task failed");
                let _ = db::finish_request(
                    &platform.store.pool,
                    &db::Terminal {
                        request_id,
                        tenant_id: tenant,
                        capability,
                        idempotency_key: panic_key.as_deref(),
                        state: State::Unknown,
                        reason: ErrorReason::ERROR_REASON_UNSPECIFIED,
                    },
                    store::now(),
                )
                .await;
                Err(Fault::internal())
            }
        }
    }
}

/// The deduplication answer for a key that already has a record. Authorization was checked before
/// this point, so the existing request ID is only disclosed to an authorized caller.
fn duplicate_fault(existing: &db::Duplicate, input_hash: &[u8; 32]) -> Fault {
    if existing.input_hash != input_hash {
        return Fault::new(
            ErrorReason::ERROR_REASON_INPUT_CONFLICT,
            "the idempotency key was already used with a different input",
        );
    }
    let (reason, message) = match existing.state {
        State::Processing => (
            ErrorReason::ERROR_REASON_PROCESSING,
            "a request with this idempotency key is still processing",
        ),
        State::Completed => (
            ErrorReason::ERROR_REASON_ALREADY_PROCESSED,
            "a request with this idempotency key was already processed; its result is not replayed",
        ),
        State::Unknown => (
            ErrorReason::ERROR_REASON_UNKNOWN_OUTCOME,
            "the outcome of the earlier request with this idempotency key is unknown; it is not repeated automatically",
        ),
    };
    Fault::new(reason, message).with_request_id(existing.request_id.to_string())
}

#[async_trait]
impl ExecutionService for Executor {
    async fn translate_text_array(
        &self,
        call: CallContext,
        request: TranslateTextArrayRequest,
    ) -> Result<TranslateTextArrayResponse> {
        let tenant = crate::rpc::canonical_tenant(&request.tenant_id)?;
        let principal = self
            .platform
            .authenticate_execution(
                &call.authorization,
                tenant,
                Capability::CAPABILITY_TRANSLATOR,
            )
            .await?;
        let settings = self.platform.settings();
        let plan = validate_translation(&self.platform.metrics, &settings, tenant, &request)?;
        match self.execute(&call, principal, settings, plan).await? {
            Done::Texts(texts) => Ok(TranslateTextArrayResponse {
                request_id: call.request_id,
                texts,
                ..Default::default()
            }),
            Done::Data(_) => Err(Fault::internal()),
        }
    }

    async fn extract_data_from_text(
        &self,
        call: CallContext,
        request: ExtractDataFromTextRequest,
    ) -> Result<ExtractDataFromTextResponse> {
        let tenant = crate::rpc::canonical_tenant(&request.tenant_id)?;
        let principal = self
            .platform
            .authenticate_execution(
                &call.authorization,
                tenant,
                Capability::CAPABILITY_EXTRACTOR,
            )
            .await?;
        let settings = self.platform.settings();
        let plan = validate_extraction(&self.platform.metrics, &settings, tenant, &request)?;
        match self.execute(&call, principal, settings, plan).await? {
            Done::Data(json_data) => Ok(ExtractDataFromTextResponse {
                request_id: call.request_id,
                json_data,
                ..Default::default()
            }),
            Done::Texts(_) => Err(Fault::internal()),
        }
    }

    async fn associate_generation(
        &self,
        call: CallContext,
        request: AssociateGenerationRequest,
    ) -> Result<AssociateGenerationResponse> {
        let platform = &self.platform;
        let operator = platform.authenticate_operator(&call.authorization).await?;
        let reason = crate::rpc::operator::valid_reason(&request.reason)?;
        let attempt_id = Uuid::parse_str(&request.attempt_id)
            .ok()
            .filter(|id| id.get_version_num() == 7)
            .ok_or_else(|| Fault::invalid_input("attemptId must be a UUID v7"))?;
        let generation = request.generation_id.as_str();
        if !crate::gateway::valid_generation_id(generation) {
            return Err(Fault::invalid_input(
                "generationId must be 1 to 200 characters of letters, digits, underscore, hyphen, dot or colon",
            ));
        }
        let attempt = db::attempt_info(&platform.store.pool, attempt_id)
            .await?
            .ok_or_else(|| Fault::not_found("attempt not found or already expired"))?;
        if attempt.usage_known || attempt.delivered || !attempt.generation_id.is_empty() {
            return Err(Fault::invalid_input(
                "the attempt already has a generation or confirmed usage",
            ));
        }
        // The generation must be readable at the Gateway for the model stored on the attempt.
        match self.gateway.generation(generation).await {
            Ok(crate::gateway::Lookup::Usage { model, .. }) if model == attempt.model => {}
            Ok(crate::gateway::Lookup::Usage { .. }) => {
                return Err(Fault::invalid_input(
                    "the generation belongs to a different model than the attempt",
                ));
            }
            Ok(_) => {
                return Err(Fault::invalid_input(
                    "the generation has no readable usage at the Gateway",
                ));
            }
            Err(_) => {
                platform.metrics.failure(Failure::Provider);
                return Err(Fault::new(
                    ErrorReason::ERROR_REASON_PROVIDER,
                    "the Gateway generation lookup is unavailable",
                ));
            }
        }
        let outcome = db::associate_generation(
            &platform.store.pool,
            attempt_id,
            generation,
            AuditEntry {
                action: Action::GenerationAssociate,
                actor: &operator.actor,
                reason,
                request_id: parse_request_id(&call.request_id),
                tenant_id: Some(attempt.tenant_id),
                target_id: Some(attempt_id),
                details: serde_json::json!({
                    "requestId": attempt.request_id.to_string(),
                    "generationId": generation,
                }),
            },
            store::now(),
        )
        .await?;
        match outcome {
            Association::Associated => {
                tracing::info!(
                    request_id = %call.request_id,
                    attempt_id = %attempt_id,
                    "generation associated; reconciliation scheduled"
                );
                Ok(AssociateGenerationResponse {
                    request_id: call.request_id,
                    ..Default::default()
                })
            }
            Association::GenerationInUse => Err(Fault::invalid_input(
                "the generation is already associated with another attempt",
            )),
            Association::NotEligible => Err(Fault::invalid_input(
                "the attempt already has a generation or confirmed usage",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::fixture;

    fn platform_settings() -> Settings {
        Settings {
            translation_models: vec!["openai/gpt-6-luna".into()],
            max_translation_items: 3,
            max_input_bytes: 64,
            ..fixture()
        }
    }

    fn translate(request: TranslateTextArrayRequest) -> Result<Plan> {
        validate_translation(
            &Metrics::new(),
            &platform_settings(),
            Uuid::now_v7(),
            &request,
        )
    }

    fn request(texts: &[&str]) -> TranslateTextArrayRequest {
        TranslateTextArrayRequest {
            tenant_id: Uuid::now_v7().to_string(),
            output_language: "ko".into(),
            input_language: String::new(),
            model: "openai/gpt-6-luna".into(),
            texts: texts.iter().map(|t| (*t).to_owned()).collect(),
            idempotency_key: String::new(),
            ..Default::default()
        }
    }

    fn reason(result: Result<Plan>) -> ErrorReason {
        result.err().expect("rejected").reason
    }

    #[test]
    fn translation_rules_follow_the_contract() {
        assert!(translate(request(&["hello"])).is_ok());
        assert_eq!(
            reason(translate(request(&[]))),
            ErrorReason::ERROR_REASON_INVALID_INPUT
        );
        assert_eq!(
            reason(translate(request(&["a", ""]))),
            ErrorReason::ERROR_REASON_INVALID_INPUT
        );
        assert_eq!(
            reason(translate(request(&["a", "  \n"]))),
            ErrorReason::ERROR_REASON_INVALID_INPUT
        );
        assert_eq!(
            reason(translate(request(&["a", "b", "c", "d"]))),
            ErrorReason::ERROR_REASON_LIMIT
        );
        assert_eq!(
            reason(translate(request(&[&"x".repeat(65)]))),
            ErrorReason::ERROR_REASON_LIMIT
        );
        let mut bad = request(&["a"]);
        bad.model = "openai/other".into();
        assert_eq!(
            reason(translate(bad)),
            ErrorReason::ERROR_REASON_INVALID_MODEL
        );
        let mut bad = request(&["a"]);
        bad.model = String::new();
        assert_eq!(
            reason(translate(bad)),
            ErrorReason::ERROR_REASON_INVALID_MODEL
        );
        let mut bad = request(&["a"]);
        bad.output_language = String::new();
        assert_eq!(
            reason(translate(bad)),
            ErrorReason::ERROR_REASON_INVALID_INPUT
        );
        let mut bad = request(&["a"]);
        bad.output_language = "not a tag".into();
        assert_eq!(
            reason(translate(bad)),
            ErrorReason::ERROR_REASON_INVALID_INPUT
        );
        let mut bad = request(&["a"]);
        bad.input_language = "en_US".into();
        assert_eq!(
            reason(translate(bad)),
            ErrorReason::ERROR_REASON_INVALID_INPUT
        );
    }

    #[test]
    fn identical_explicit_languages_are_rejected_case_insensitively() {
        for (input, output) in [("en", "en"), ("en-US", "EN-us"), ("ko", "KO")] {
            let mut bad = request(&["a"]);
            bad.input_language = input.into();
            bad.output_language = output.into();
            assert_eq!(
                reason(translate(bad)),
                ErrorReason::ERROR_REASON_INVALID_INPUT,
                "{input}/{output}"
            );
        }
        let mut regional = request(&["a"]);
        regional.input_language = "en".into();
        regional.output_language = "en-US".into();
        assert!(
            translate(regional).is_ok(),
            "different tags are different languages"
        );
        // An omitted input language is never "identical": detection may find the output language.
        assert!(translate(request(&["안녕"])).is_ok());
    }

    #[test]
    fn idempotency_keys_are_bounded_visible_ascii() {
        assert_eq!(idempotency_key("").unwrap(), None);
        assert_eq!(
            idempotency_key("order-42_a.b").unwrap().as_deref(),
            Some("order-42_a.b")
        );
        for bad in ["has space", "tab\t", "한글", &"k".repeat(257), "new\nline"] {
            assert!(idempotency_key(bad).is_err(), "{bad:?}");
        }
        assert!(idempotency_key(&"k".repeat(256)).is_ok());
    }

    #[test]
    fn input_hash_separates_tenants_methods_fields_and_boundaries() {
        let tenant = Uuid::now_v7();
        let base = hash_fields("translate", tenant, &[b"ab", b"c"]);
        assert_eq!(base, hash_fields("translate", tenant, &[b"ab", b"c"]));
        assert_ne!(base, hash_fields("translate", tenant, &[b"a", b"bc"]));
        assert_ne!(base, hash_fields("extract", tenant, &[b"ab", b"c"]));
        assert_ne!(
            base,
            hash_fields("translate", Uuid::now_v7(), &[b"ab", b"c"])
        );
    }

    #[test]
    fn duplicate_state_machine_maps_each_stored_state() {
        let hash = [7_u8; 32];
        let existing = |state, hash: [u8; 32]| db::Duplicate {
            request_id: Uuid::now_v7(),
            state,
            input_hash: hash.to_vec(),
        };
        for (state, expected) in [
            (State::Processing, ErrorReason::ERROR_REASON_PROCESSING),
            (
                State::Completed,
                ErrorReason::ERROR_REASON_ALREADY_PROCESSED,
            ),
            (State::Unknown, ErrorReason::ERROR_REASON_UNKNOWN_OUTCOME),
        ] {
            let duplicate = existing(state, hash);
            let fault = duplicate_fault(&duplicate, &hash);
            assert_eq!(fault.reason, expected);
            assert_eq!(
                fault.request_id.as_deref(),
                Some(duplicate.request_id.to_string().as_str())
            );
            // A different input always wins as a conflict, whatever the stored state.
            let conflict = duplicate_fault(&duplicate, &[8_u8; 32]);
            assert_eq!(conflict.reason, ErrorReason::ERROR_REASON_INPUT_CONFLICT);
            assert_eq!(conflict.request_id, None);
        }
    }

    #[test]
    fn readers_enforce_translation_shape_and_extraction_sentinel() {
        let reader = Reader::Translation { count: 2 };
        assert!(matches!(
            reader.read(r#"{"translations":["a","b"]}"#),
            Ok(Done::Texts(_))
        ));
        assert!(matches!(
            reader.read(r#"{"translations":["a"]}"#),
            Err(Rejected::Malformed)
        ));
        let schema = Schema::parse(
            r#"{"type":"object","properties":{"n":{"type":"string"}},"required":["n"],"additionalProperties":false}"#,
        )
        .unwrap();
        let reader = Reader::Extraction(Box::new(schema));
        assert!(matches!(
            reader.read(r#"{"status":"extracted","data":{"n":"x"}}"#),
            Ok(Done::Data(_))
        ));
        assert!(matches!(
            reader.read(r#"{"status":"impossible","data":{"n":""}}"#),
            Err(Rejected::Impossible)
        ));
        assert!(matches!(
            reader.read(r#"{"status":"extracted","data":{"n":1}}"#),
            Err(Rejected::Malformed)
        ));
    }
}
