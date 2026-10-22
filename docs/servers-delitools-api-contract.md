# DeliTools API contract

## Scope

`servers/delitools-api` owns the independent API, explicit migrations and OCI packaging. Existing application and mobile release boundaries are unchanged.

## Runtime and Language

Rust in the independent `servers/Cargo.toml` workspace (package `delitools-api`, binary `delitools-api`), PostgreSQL, one process and one running instance. The package inherits the server workspace edition, license and publication settings and reuses `servers/crates/server-common` for authentication, configuration, HTTP policy, rate limits and telemetry. Protocol bindings come from `servers/crates/delino-protos`; no schema or generated source is copied under `servers/delitools-api`. API and background delivery, reconciliation, catalog synchronization and retention share the process.

## Users and Operators

External developers, tenant Owner/Admin, personal-key owners and dedicated Logto platform operators.

## Interfaces and Contracts

New persisted IDs are UUID v7. Provider identifiers remain external references. Connect schemas are owned by the [protocol contract](protos-delitools-v1-contract.md).

### Translator: `translateTextArray()`

- Accept an output language, optional input language, selected LLM model, and array of strings. Use BCP 47 language tags.
- The output language is required. If the input language is omitted, detect one input language for the array as a whole. Attempt translation even when the array contains mixed languages.
- Reject an empty array, any empty string, and explicitly specified identical input/output languages as invalid input.
- Automatic language detection is an exception to identical-language rejection: if the detected language equals the requested output language, still let the model process each item. Do not short-circuit to an unprocessed original array.
- Only operator-allowlisted models may be selected. Operators control model eligibility; catalog price synchronization does not automatically allow new models.
- Return translated strings in the original order and with the original array length. Return a result only if all items succeed. A failed item makes the entire request fail; do not return partial translations or substitute the source text as a failure fallback.
- Use Vercel AI Gateway. Apply a 15% markup to the chargeable AI cost.

### Data Extractor: `extractDataFromText()`

- Accept text and a JSON schema. Use the fixed `gpt-6-luna` model through Vercel AI Gateway; the caller does not select the extraction model.
- Support only the schema subset supported by this model's structured output. Validate the schema before AI execution and reject unsupported schemas without an AI call. Document and test the supported subset in the owning API contract.
- Instruct the model to use only information grounded in the input. If information required by the schema is missing from the source, return an extraction-impossible error; do not invent a value to satisfy the schema.
- Validate the output against the accepted schema on the server. Malformed or nonconforming output is a failure, not a successful extraction.
- Use automated evaluation fixtures for semantic correctness. Schema validation and prompting do not establish a guarantee of complete semantic accuracy. Per-value evidence generation and verification are not required.
- Apply a 30% markup to the chargeable AI cost.

### Public response and execution contract

- Both methods are synchronous Connect RPC calls. Return the completed result or an error in the same RPC; no asynchronous job submission/result retrieval flow is in scope.
- Success messages contain the translation array or extracted data and a request ID. Do not add model, token usage, or estimated charge fields. Users inspect usage and charges through the separate billing interface.
- Errors contain a Connect standard code, stable reason code, request ID, and English message. Document the mapping for invalid input/schema/model, authentication/permission failure, billing blocks, limits, processing/duplicate/input-conflict states, unknown execution outcome, extraction failure, provider failure, timeout, and cancellation.
- Require operator settings for input size, translation item count, execution timeout, tenant request rate, and tenant concurrency. Reject missing or invalid settings at startup; do not invent fixed product limits.
- Retry only transient AI errors. Require an operator-configured maximum retry count. Do not retry malformed model output or automatically switch to another model.
- When the client cancels or disconnects, attempt to cancel the AI call and stop further AI retries. Continue usage reconciliation and reporting in the background.

### Authentication, tenancy, and API keys

- Support both Logto user tokens and DeliTools API keys for external callers. Integrate protected execution with current delcore tenant authorization and billing checks. Fail closed for new paid execution when these checks cannot be completed.
- Provide tenant service keys and user personal keys. Tenant Owner/Admin can manage service keys. Users manage their own personal keys, which remain subject to the user's current permissions.
- Assign Translator and Extractor capabilities per key. Personal-key permissions are the intersection of key capabilities and current user permissions. Tenant authorization and billing checks apply independently to both key types.
- A service key belongs to the tenant rather than the issuing user's ongoing membership. Personal keys cannot retain permissions after their user's access is removed.
- Show a secret key only once at creation. Persist only its verification hash, not a recoverable secret. A lost key requires a new key.
- Expiration is optional. Without an expiration date, a key remains valid until explicitly revoked, subject to current authorization.
- After key revocation or user permission removal, allow an already-running AI call to finish, but reject new requests and additional AI retry attempts. Bill incurred usage.
- Restrict platform-wide model administration, operational settings, and full request-state inspection to a dedicated Logto platform-operator role. Tenant Owner/Admin does not imply platform authority. Operator mutations require a reason and an audit record.
- Follow repository environment/secret rules. Do not commit real credentials or environment files. Use the service identity and reporting scopes required by delcore for downstream service calls.

### Optional idempotency

- Accept an optional caller-provided idempotency key. Without one, every call is a new execution and may incur a new charge.
- Scope deduplication by **tenant + API method + key**, including calls made through different credentials for the same tenant. Check current authorization before exposing duplicate state.
- Persist the key, an input-comparison hash, and processing state; do not persist the source or result. The same key with a different input fails with an input-conflict error.
- An in-progress duplicate immediately returns a processing error and the existing request ID. It does not wait for completion.
- A duplicate of a completed request returns an already-processed error. Do not replay or cache a result.
- If a crash or provider ambiguity leaves execution status unknown, return an unknown-state error and do not automatically repeat AI execution. Provide operator state inspection and a Gateway usage-reconciliation procedure, not original-request replay or manual charge entry.
- After expiration and deletion of the deduplication record, the same key is accepted as a new request and charged normally. Deduplication is guaranteed only within the retention window.

### Pricing, usage, and settlement

- Use the Vercel AI Gateway model catalog's public token prices for both APIs. Compute the basis from applicable published prices and actual measured token usage. Gateway discounts and promotional credits do not reduce this basis.
- Translator charge basis is multiplied by **1.15**; Extractor charge basis is multiplied by **1.30**. Keep exact decimals and use delcore's accounting and final rounding rules; do not use floating-point monetary arithmetic.
- Automatically synchronize public catalog prices. Require an operator-set synchronization interval. On synchronization failure, continue using the last known price without an age limit. If no price exists, reject new AI execution.
- Apply changed prices to every customer for requests that start after the price becomes effective. Freeze the price at the initial request start and use that snapshot for all AI retry attempts within that request.
- Include confirmed token usage from retry attempts. Failure, timeout, cancellation, or inability to deliver a result does not waive incurred AI cost or its markup.
- If actual usage is unknown, defer billing and reconcile with Gateway. Do not estimate usage for charging. If usage cannot be established before the record's 30-day expiration, abandon that charge when the record expires.
- Report measured usage through delcore; delcore remains the authority for rating and settlement. Implement the delcore support for the specified effective-price and markup behavior within this issue, without shifting settlement ownership into DeliTools.
- Continue retrying usage delivery and unknown-usage lookup until the 30-day record expiration. Require operator settings for retry interval, maximum backoff delay, and alert thresholds. Background reporting retries are distinct from bounded AI execution retries.
- Preserve durable usage identities to prevent duplicate ledger effects. Retain usage attribution and the applicable pricing snapshot needed for delivery and reconciliation, without source text or results.
- Use and validate delcore's existing USD-only operation. Currency conversion is not part of this implementation.
- Keep existing delcore blocking, credits, settlement, and service-scoped reporting rules. Post-execution metering does not prevent all concurrent overspend.

### Data retention and privacy

- Do not persist input text, input content as raw payloads, or translation/extraction results. Do not put them in logs, traces, or audit events. Exclude credentials and secrets from telemetry.
- Retain idempotency records and usage-delivery/reconciliation records for **30 days from each record's first creation**, without extending expiration after retries or state changes.
- Delete these records at expiration **regardless of processing state**. This includes unresolved executions and undelivered/unconfirmed usage. The requester explicitly accepts possible duplicate execution/charging after deduplication expiry and permanent loss of pending usage. Do not silently substitute an unresolved-record retention hold.
- Include the DeliTools pending-usage deletion exception in the owning delcore contract as part of this issue, with any required implementation changes and automated integration coverage. It does not change delcore's ledger retention or delete already accepted accounting entries.
- Retain platform audit records and general operational logs for **30 days from creation**. Do not conflate their retention with API-key validity.
- Document that text and schemas are sent to Vercel AI Gateway and the model provider. Describe their data-handling policies so users can decide whether to submit their data. DeliTools does not promise provider-wide zero retention or include sensitive-data detection in v1.

### Operations, release, and recovery

- Deliver OCI image build support for the Rust executable built from the locked server workspace, with canonical migrations and server/protocol license notices, and explicit database migrations. Production infrastructure and deployment are the requester's responsibility.
- Check schema compatibility at startup. Document backup before updates and use **fix-forward only** for application/schema defects. Image rollback and reverse schema migrations are not supported recovery strategies for this scope.
- Provide structured Rust `tracing` logs, safe correlation IDs, health/readiness interfaces, Prometheus metrics, OpenTelemetry traces, and operational procedures. Require operator-configured alert thresholds.
- Cover AI failures/retries, authorization or billing-check failures, price-sync failures/missing prices, pending usage/reconciliation backlog, and records nearing expiration with operational diagnostics. Keep telemetry free of raw content and secrets and use bounded metric dimensions.
- No separate SLO or throughput guarantee is required. Verify the configured timeout, size, rate, concurrency, and failure behavior through automated tests.
- No feature flag applies: this is a new independent service, and release is controlled through deployment, authentication, and billing. These enforcement checks remain required without a feature flag.
- Provide state inspection and Gateway reconciliation procedures for unknown outcomes. Do not implement manual AI replay or manual fee entry.

## Storage

PostgreSQL stores only key verification hashes, authorization attribution, input hashes, request states, attempt identities, measured usage, price snapshots and safe audit/operational metadata. Never store inputs, schemas or results. Deduplication and usage records expire independently 30 days after creation, in every state. Keys do not expire with request records. Delcore accounting retention is independent. Migrations live in `servers/delitools-api/migrations` as forward-only SQL files applied by an explicit `--migrate` command; startup verifies the schema version and refuses to serve an incompatible schema.

## Security

Recheck authorization before duplicate disclosure and each AI attempt. Keep credentials out of database payloads and telemetry. Operator mutations require a dedicated role, reason and audit record. Inputs and schemas cross Gateway and provider boundaries; no provider-wide zero-retention promise applies.

## Logging

Use structured `tracing` events with safe correlation IDs, bounded metric dimensions and OpenTelemetry. Persist safe operational event categories and correlation IDs for 30 days. Never log upstream bodies, inputs, schemas, results or secrets. A failure that stops the process is logged once as `startup_or_runtime_failure` with its redacted cause chain, so configuration, database, identity-provider, bind and serving failures stay distinguishable. When logging itself cannot be initialized (an unsupported `LOG_FORMAT` or an OTLP exporter that cannot be built), no subscriber exists, so the same fields are written as one JSON line on standard error instead and the process never exits silently. Operators must apply the same 30-day TTL to exported telemetry in their collector; process output cannot enforce external retention.

## Build and Test

Once the planned `servers/delitools-api` and `examples/delitools-client` paths exist, run the server checks in [the Rust workspace contract](servers-rust-workspace-contract.md): format, Clippy with warnings denied, unit tests and the ignored PostgreSQL tests against a disposable `TEST_DATABASE_URL`, plus workspace binary builds and root `cargo test`. Run protocol lint, generation, freshness and FILE breaking checks, `go test ./...` and the Go example build. Mocked Gateway, Logto and delcore servers replace live providers. Semantic fixtures test expected behavior without promising perfect extraction accuracy. Preserve actual results and unperformed live-account checks in PR/CI records.

## Dependencies and Integrations

Delcore owns rating, credits and settlement. Gateway owns model execution and usage lookup. Logto issues user and service tokens. Background delivery retries persist until expiry; unknown usage never becomes an estimated charge.

## Change Triggers

Update the project index, protocol and delcore contracts with boundary changes. Update scoped instructions only for development procedures or ownership.

## References

- [Project](project-delitools.md)
- [Protocol](protos-delitools-v1-contract.md)
- [Delcore server](servers-delcore-api-contract.md)
- [Rust workspace](servers-rust-workspace-contract.md)
- [Repository defaults](repository-defaults.md)
- [Gateway REST API](https://vercel.com/docs/ai-gateway/sdks-and-apis/rest-api)

## Delcore usage report

DeliTools reports each attempt's measured usage with `IngestEvent` as the registered service `SERVICE_ID`, using the durable attempt UUID as both `request_key` and `event_key`, the attempt's actual occurrence time, and the tenant of the request. The event type is `translator.usage` for Translator or `extractor.usage` for Extractor. Attributes are flat: text `model` and `price_effective_at` (RFC 3339), number `input_tokens`, `output_tokens`, `cache_read_tokens` and `cache_write_tokens` (canonical non-negative integers), and number `input_usd_per_token`, `output_usd_per_token`, `cache_read_usd_per_token` and `cache_write_usd_per_token` (canonical decimals from the request's frozen public catalog price snapshot). Categories without a published price are omitted together with their token counts; usage that needs an unpublished category stays pending and is never estimated. The delcore catalog is expected to declare the `translator-usd` and `extractor-usd` meters over these events, and delcore is expected to compute the exact USD token cost from the supplied per-token prices and apply the 1.15 and 1.30 markups. That call-time rating is a delcore-owned dependency specified in the [delcore server contract](servers-delcore-api-contract.md); this contract only fixes the report DeliTools sends. Until delcore provides it, the report cannot be rated, so it is not authoritative and paid DeliTools traffic must not be enabled. DeliTools never sends a computed charge, markup or currency amount.

For tenant-key authorization, DeliTools calls delcore `Authorize` with the registered service identity and no member user, because a service key must stay valid independently of the member who issued it. That requires a delcore-owned consumer authorization mode in the [delcore protocol](protos-delcore-v1-contract.md) and server contract; without it delcore cannot authorize a service-key execution, so DeliTools must fail closed and not enable paid service-key traffic.

## Delcore integration requirements

Delcore must rate AI usage reports at the price snapshot supplied with each report, as the [delcore server contract](servers-delcore-api-contract.md) specifies for this consumer. When delcore serves DeliTools, configure both `USAGE_LATENESS_SECONDS` and `REPORTING_GRACE_SECONDS` to at least 2592000 (30 days), and keep raw usage and webhook retention at least as long as the corresponding reporting windows. Verify these settings before enabling paid DeliTools traffic and after configuration changes.

## Operator configuration

Configuration uses environment variables, with an optional working-directory `.env` that never overrides process variables, matching delcore. The binary publishes that file to the process environment before it starts logging, so `LOG_FORMAT`, `RUST_LOG` and `OTEL_EXPORTER_OTLP_*` values supplied only through `.env` take effect (an explicitly empty process value still wins). Required deployment variables: `LISTEN`, `DATABASE_URL`, `ISSUER`, `AUDIENCE`, `OPERATOR_ROLE`, `DELCORE_URL`, `DELCORE_RESOURCE`, `LOGTO_TOKEN_URL`, `CLIENT_ID`, `CLIENT_SECRET`, `SERVICE_ID`, `GATEWAY_URL`, `GATEWAY_KEY`, `TIMEOUT_SECONDS`, `MAX_GATEWAY_RESPONSE_BYTES`, `LOG_FORMAT` (optional) and the initial execution settings below. URLs require HTTPS outside loopback fixtures. `SERVICE_ID` is the registered UUID v7 of the delcore service named `delitools`. The local development listener is `127.0.0.1:46322`. Only `DATABASE_URL` is needed for the explicit `--migrate` command.

Stored execution settings are initialized once from the environment (`MAX_INPUT_BYTES`, `MAX_TRANSLATION_ITEMS`, `EXECUTION_TIMEOUT_SECONDS`, `TENANT_REQUESTS_PER_MINUTE`, `TENANT_CONCURRENCY`, `MAX_AI_RETRIES`, `PRICE_SYNC_SECONDS`, `DELIVERY_RETRY_SECONDS`, `MAX_BACKOFF_SECONDS`, `ALERT_BACKLOG`, `ALERT_EXPIRY_SECONDS`, `TRANSLATION_MODELS` as a JSON array), then changed only by an authenticated platform operator through `UpdateSettings` with a reason and audit record. Startup rejects a missing or invalid value; the contract defines no defaults.

| Settings field            | Meaning                                                              |
| ------------------------- | -------------------------------------------------------------------- |
| `maxInputBytes`           | Input text/schema and decoded request bound.                         |
| `maxTranslationItems`     | Largest accepted translation array.                                  |
| `executionTimeoutSeconds` | Whole paid request deadline, including retries.                      |
| `tenantRequestsPerMinute` | Per-tenant fixed-minute admission count.                             |
| `tenantConcurrency`       | Maximum simultaneously running calls for a tenant.                   |
| `maxAiRetries`            | Explicit retry count; zero is valid.                                 |
| `priceSyncSeconds`        | Public catalog refresh interval.                                     |
| `deliveryRetrySeconds`    | Background tick and initial delivery retry delay.                    |
| `maxBackoffSeconds`       | Maximum background retry delay; at least the initial delay.          |
| `alertBacklog`            | Pending-attempt count that emits a backlog alert.                    |
| `alertExpirySeconds`      | Warning window before the 30-day expiration boundary; under 30 days. |
| `translationModels`       | Explicit model allowlist; catalog synchronization never extends it.  |

## Service foundation

The foundation in `servers/delitools-api` implements everything except the paid execution pipeline. `TranslateTextArray`, `ExtractDataFromText` and `AssociateGeneration` are registered on the Connect router and answer Connect `unimplemented` with an English message and no reason until an execution service is installed through `Server::with_execution`. Everything below is verified against disposable PostgreSQL databases and mocked identity and tenant services.

### Commands and startup

`delitools-api` serves by default. `--migrate` applies the embedded forward-only migrations in one transaction under a PostgreSQL advisory lock and is safe to repeat; `check-schema` fails unless every embedded migration is applied with an identical digest and no newer migration exists. Both read only `DATABASE_URL`. The launcher flag `--dev-env` is accepted for compatibility; the working-directory `.env` is always read and never overrides process variables. Serving never migrates. It checks the schema, takes the session lock that rejects a second instance, seeds the settings table from the environment only when it is empty, marks requests and idempotency records still `processing` as `unknown` (a restart never resumes or repeats an AI prompt), verifies the Logto issuer and key set, and then serves. Losing the lock session, or a background task terminating abnormally, stops the process. A transport failure such as a listener accept error also signals background work to stop, so the process exits for the orchestrator to restart it instead of waiting for tasks that never end. Shutdown drains connections for up to `TIMEOUT_SECONDS` plus the largest `executionTimeoutSeconds` an operator can set (3600), so a live increase never cuts a request before its deadline (the idle socket read limit uses the same ceiling, because a running handler keeps a quiet read pending), and lets running background work finish within the current execution deadline.

### Stored settings bounds

The contract defines no defaults, but values are bounded so a mistake cannot disable protection: `maxInputBytes` 1 to 8 MiB, `maxTranslationItems` 1 to 10000, `executionTimeoutSeconds` 1 to 3600, `tenantRequestsPerMinute` 1 to 1000000, `tenantConcurrency` 1 to 10000, `maxAiRetries` 0 to 10 (an explicit zero is valid and an absent value is rejected), `priceSyncSeconds` 1 to 604800, `deliveryRetrySeconds` 1 to 86400, `maxBackoffSeconds` from `deliveryRetrySeconds` to 30 days, `alertBacklog` at least 1, `alertExpirySeconds` 1 to under 30 days, and `translationModels` a non-empty duplicate-free list of at most 1000 identifiers of at most 128 characters from `A-Z a-z 0-9 . _ - / :`. `TIMEOUT_SECONDS` is 1 to 120 and bounds outbound dependency calls; the whole HTTP request is bounded by the live `executionTimeoutSeconds` plus `TIMEOUT_SECONDS`. `MAX_GATEWAY_RESPONSE_BYTES` is 1024 to 64 MiB. The Connect message limit is the 8 MiB input ceiling plus 64 KiB of envelope. Configuration `Debug` output redacts secrets and the database URL.

### API keys and credentials

A key is `dtk_` followed by the 32 hex digits of its UUID v7 and `_` and 43 URL-safe base64 characters of 32 random bytes. The service stores a domain-separated SHA-256 of the secret part, compares it in constant time (also for unknown key IDs) and returns the full token only in the `CreateKey` response. Unknown, mismatching, revoked and expired keys are indistinguishable authentication failures; an expiry instant is exclusive. `CreateKey` rejects an `expiresAt` that is not in the future and judges it again, and takes `createdAt`, after the tenant authority answers, so a slow authority call never yields a key that is already expired. `Authorization` accepts a case-insensitive `Bearer` scheme with exactly one token. A `dtk_` token is an API key; anything else is a Logto JWT validated for signature (RS256 or ES256), issuer, audience, expiry and a five-minute `nbf` allowance, with throttled key-set refresh. A fetched key set is trusted for at most five minutes: after that no token verifies from it until a refresh succeeds, so a signing key the provider retires stops working within that bound, and JWT authentication fails closed while the provider is unreachable (API keys do not depend on it). The key endpoint discovered from the issuer must satisfy the same URL rule as configured URLs (HTTPS, or loopback HTTP for fixtures); startup fails otherwise. A machine-to-machine token (subject equal to client ID) never acts as a user and never acquires the operator role.

Key management (`CreateKey`, `ListKeys`, `RevokeKey`) requires a Logto user token; API keys cannot manage keys. Personal-key management needs current tenant membership of the acting user and service-key management needs the Owner or Admin role, each checked with the tenant authority on every call. A personal key belongs to its creator; only that user revokes it. A service key records the issuing user for attribution only. `RevokeKey` is idempotent and keeps the first revocation time. A key that does not exist, belongs to another tenant, or is another user's personal key is reported as `FORBIDDEN`, so key IDs are not disclosed. `ListKeys` with a kind lists that kind (own personal keys, or service keys for Owner/Admin); without a kind it lists the caller's own personal keys plus service keys when the caller is Owner or Admin. Pagination orders by key ID with a default page of 50 and a maximum of 100. Key creation and revocation write safe operational events. Capabilities are `TRANSLATOR`, `EXTRACTOR` or both, each at most once.

### Tenant authority

`platform::Platform` asks a `TenantAuthority` (`authority.rs`) for every management check and every new paid execution: `resolve_user` maps a verified user bearer to a platform user, `authorize_execution` takes the tenant, capability and consumer mode (`TenantService` for service keys, `User` for user tokens and personal keys) and answers allowed, forbidden or billing-blocked, and `authorize_management` answers for personal-key or service-key scopes. A question the authority cannot answer fails closed with Connect `unavailable` and reason `INTERNAL`. `Platform::authenticate_execution` implements the credential rules: a service key is tenant-level and requires its capability and tenant; a personal key additionally requires the user's current access; a user token acts as the user; billing blocks apply to all. `Platform::reauthorize` repeats the check immediately before a duplicate disclosure or an AI attempt and rejects revoked or expired keys. The in-memory implementation serves tests and local fixtures. Until the delcore-backed adapter is installed, the binary uses a fail-closed authority: tenant-scoped RPCs return `unavailable`, while operator RPCs, health, readiness and metrics work.

### Operator RPCs

`GetSettings`, `UpdateSettings` and `InspectRequest` require a verified Logto user token whose signed `roles` claim contains `OPERATOR_ROLE`; tenant roles, machine tokens and API keys never qualify. `UpdateSettings` requires a reason of 3 to 1000 characters after trimming and a complete valid `Settings` message, and in one transaction replaces the settings and writes an audit record (actor `operator:<SHA-256 of issuer and subject>`, reason, request ID, changed names with previous and new values). The live settings snapshot changes after commit. `InspectRequest` returns state, attempts, frozen prices and measured usage, writes an audit record with the inspected request and no reason, and reports an unknown or expired request as Connect `not_found` with reason `INVALID_INPUT`. Rejected calls leave no audit record.

### Storage

Migration `00001` creates operator settings, API keys, audit records and operational events; `00002` creates catalog price versions (effective time, provider route and per-category USD prices, where a missing category means no published price), requests, attempts (frozen price snapshot, occurrence time, measured usage, delivery state, retry time) and idempotency records. Requests, attempts and idempotency records have no foreign keys to each other, and each attempt carries its own tenant, model and price snapshot, so every record expires on its own `created_at` and delivery never depends on another row. A database trigger rejects any change to `created_at` on every retained table. Tenant rate and concurrency state is in memory (`admission.rs`) and not persisted. No column holds input text, schemas, results or recoverable secrets.

### Background work and retention

The worker (`worker/`) runs registered `BackgroundTask`s, each on its own interval taken from the live settings. A failing run is logged, counted and retried at the next interval without stopping other tasks. The foundation registers retention (every five minutes) and backlog sampling (every 15 seconds); the execution layer registers usage delivery, Gateway reconciliation and catalog price synchronization with `Worker::register`. Retention deletes idempotency records, attempts, requests, audit records and operational events whose `created_at` is at or before now minus 30 days, in batches, regardless of state, retry count or update time. API keys are never deleted by retention.

### Observability

Every response and error carries a server-issued UUID v7 request ID in `X-Delitools-Request-Id` (a client-supplied value is ignored) and in `ErrorDetail`. This includes Connect unary errors produced before a handler runs, such as an undecodable JSON or protobuf body, a body over the message limit, an unknown procedure or an unsupported content type: the service keeps the framework's HTTP status and Connect code, replaces its message with a fixed English one that never quotes request content, and attaches the `ErrorDetail` (`INVALID_INPUT` for `invalid_argument`, including the bare 415 for an unsupported content type, which gains a JSON error body; `LIMIT` for `resource_exhausted`; `UNSPECIFIED` for `unimplemented`; `INTERNAL` otherwise). `/healthz` is process liveness. `/readyz` is false unless the schema is compatible and `catalog_prices` holds a synchronized price; the execution layer satisfies the rule by populating the table and may add checks with `Platform::add_readiness_check`. Until a price exists the service is intentionally not ready. `/metrics` exposes request counts and durations by procedure and bounded reason, failure counts by kind (authentication, authorization, authority unavailable, provider, catalog sync, missing price, usage delivery, worker), limit rejections, retention runs and deletions by table, worker runs by task, and gauges for unknown-usage attempts, pending deliveries, unknown requests, attempts nearing expiry, price availability and the two configured alerts. Tracing spans and logs carry the request ID, procedure and bounded outcome only.

## Recovery and diagnostics

Use `InspectRequest` with the recorded request UUID to view request state, attempt identifiers, generation IDs, price snapshots and measured usage; never collect original text or results. Where an attempt has no generation ID, find the Gateway generation through the safe attempt UUID sent as the provider user reference, verify tenant, model and time, then call `AssociateGeneration` with the attempt ID, generation ID and an audit reason. The method validates readable Gateway usage for the stored model, rejects a generation already associated with another retained attempt, and schedules normal reconciliation. It accepts no tokens, prices or fees and never replays inference. If evidence cannot establish usage, let the record expire; no charge is estimated.

Restart marks interrupted requests unknown and resumes usage processing; it never resumes an AI prompt. Pending delivery uses a durable attempt UUID as both usage event identity and delivery request key; each attempt keeps its occurrence time and the original request pricing snapshot. Background retries use capped exponential delay until expiration. Deduplication, attempt, audit and operational-log deletion use each row's immutable creation time.

Monitor `/healthz`, `/readyz` and `/metrics` on a restricted network. Readiness is false until the catalog holds a synchronized price; later refresh failures preserve readiness with the last known prices. Prometheus reports pending and unknown usage, unknown requests, provider, authorization and catalog failures, missing prices and pending records near expiration, without tenant, model or request labels.

Back up before updates. Stop the old process, run forward migrations and start the matching binary. Startup validates the schema version and rejects a second instance through a PostgreSQL session lock. Fix defects forward; neither image rollback nor reverse migration is supported. Infrastructure provisioning, live account acceptance and deployment are separate from automated tests.

## Local development

Run service-local `pnpm dev` to inject the Infisical development folder `/delitools-api` and execute the binary built by the shared server builder, as for the other Rust APIs. Root `pnpm dev` starts the owning Compose database and includes the task. The disposable database uses `127.0.0.1:55434` by default, overridden by `DELITOOLS_POSTGRES_PORT`. Apply migrations explicitly with `delitools-api --migrate`; development launchers never run migrations.
