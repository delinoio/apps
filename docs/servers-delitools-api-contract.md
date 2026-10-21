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
- Provider data handling, for the public documentation: every translation text, extraction text and caller schema is sent to Vercel AI Gateway and to the model provider that serves the request (the extraction model is `openai/gpt-6-luna`; translation models are chosen by the operator). Vercel documents its own [Zero Data Retention](https://vercel.com/docs/ai-gateway/security-and-compliance/zdr) and [security and compliance](https://vercel.com/docs/ai-gateway/security-and-compliance) posture and states that provider-side retention depends on the provider's terms and per-request or team settings. DeliTools does not send the zero-data-retention routing option and does not choose providers by retention policy, so it cannot promise zero retention. The safe attempt UUID is sent as the provider user reference for usage reconciliation; no tenant, user or key identifier is sent. Users decide whether to submit sensitive data; DeliTools performs no sensitive-data detection.

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

Use structured `tracing` events with safe correlation IDs, bounded metric dimensions and OpenTelemetry. Persist safe operational event categories and correlation IDs for 30 days. Never log upstream bodies, inputs, schemas, results or secrets. A failure that stops the process is logged once as `startup_or_runtime_failure` with its redacted cause chain (an HTTP request error shows only the origin of its URL, never the path or query, which can carry signed credentials), so configuration, database, identity-provider, bind and serving failures stay distinguishable. When logging itself cannot be initialized (an unsupported `LOG_FORMAT` or an OTLP exporter that cannot be built), no subscriber exists, so the same fields are written as one JSON line on standard error instead and the process never exits silently. Operators must apply the same 30-day TTL to exported telemetry in their collector; process output cannot enforce external retention.

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

Configuration uses environment variables, with an optional working-directory `.env` that never overrides process variables, matching delcore. The binary publishes that file to the process environment before it starts logging, so `LOG_FORMAT`, `RUST_LOG` and `OTEL_EXPORTER_OTLP_*` values supplied only through `.env` take effect (an explicitly empty process value still wins). Required deployment variables: `LISTEN`, `DATABASE_URL`, `ISSUER`, `AUDIENCE`, `OPERATOR_ROLE`, `DELCORE_URL`, `DELCORE_RESOURCE`, `LOGTO_TOKEN_URL`, `CLIENT_ID`, `CLIENT_SECRET`, `SERVICE_ID`, `GATEWAY_URL`, `GATEWAY_KEY`, `TIMEOUT_SECONDS`, `MAX_GATEWAY_RESPONSE_BYTES`, `LOG_FORMAT` (optional) and the initial execution settings below. URLs require HTTPS outside loopback fixtures. `SERVICE_ID` is the registered UUID v7 of the delcore service named `delitools`. `GATEWAY_URL` is the Gateway base including the `/v1` prefix (for example `https://ai-gateway.vercel.sh/v1`) and `GATEWAY_KEY` its API key; the public model catalog is read without the key. `CLIENT_ID` and `CLIENT_SECRET` are the Logto machine-to-machine application registered as the delcore service client, which must hold the `authorize` and `usage:write` scopes; `DELCORE_RESOURCE` is the resource requested in the token call. Logto user tokens are forwarded to delcore `GetCurrentUser`, so they must also be accepted by delcore. The local development listener is `127.0.0.1:46322`. Only `DATABASE_URL` is needed for the explicit `--migrate` command.

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

The service in `servers/delitools-api` consists of the foundation described in this section and the [execution pipeline](#execution-pipeline). `TranslateTextArray`, `ExtractDataFromText` and `AssociateGeneration` are registered on the Connect router and forwarded to the `ExecutionService` installed with `Server::with_execution`; the binary installs `exec::Executor`. A build without an installed execution service answers these RPCs with Connect `unimplemented`, an English message and an unspecified reason. Everything is verified against disposable PostgreSQL databases and mocked Gateway, identity and delcore services.

### Commands and startup

`delitools-api` serves by default. `--migrate` applies the embedded forward-only migrations in one transaction under a PostgreSQL advisory lock and is safe to repeat; `check-schema` fails unless every embedded migration is applied with an identical digest and no newer migration exists. Both read only `DATABASE_URL`. The launcher flag `--dev-env` is accepted for compatibility; the working-directory `.env` is always read and never overrides process variables. Serving never migrates. It checks the schema, takes the session lock that rejects a second instance, seeds the settings table from the environment only when it is empty, marks requests and idempotency records still `processing` as `unknown` (a restart never resumes or repeats an AI prompt), verifies the Logto issuer and key set, and then serves. Losing the lock session, or a background task terminating abnormally, stops the process. A transport failure such as a listener accept error also signals background work to stop, so the process exits for the orchestrator to restart it instead of waiting for tasks that never end. Shutdown drains connections for up to `TIMEOUT_SECONDS` plus the largest `executionTimeoutSeconds` an operator can set (3600), so a live increase never cuts a request before its deadline (the idle socket read limit uses the same ceiling, because a running handler keeps a quiet read pending), and lets running background work finish within the current execution deadline.

### Stored settings bounds

The contract defines no defaults, but values are bounded so a mistake cannot disable protection: `maxInputBytes` 1 to 8 MiB, `maxTranslationItems` 1 to 10000, `executionTimeoutSeconds` 1 to 3600, `tenantRequestsPerMinute` 1 to 1000000, `tenantConcurrency` 1 to 10000, `maxAiRetries` 0 to 10 (an explicit zero is valid and an absent value is rejected), `priceSyncSeconds` 1 to 604800, `deliveryRetrySeconds` 1 to 86400, `maxBackoffSeconds` from `deliveryRetrySeconds` to 30 days, `alertBacklog` at least 1, `alertExpirySeconds` 1 to under 30 days, and `translationModels` a non-empty duplicate-free list of at most 1000 identifiers of at most 128 characters from `A-Z a-z 0-9 . _ - / :`. `TIMEOUT_SECONDS` is 1 to 120 and bounds outbound dependency calls; the whole HTTP request is bounded by the live `executionTimeoutSeconds` plus `TIMEOUT_SECONDS`. `MAX_GATEWAY_RESPONSE_BYTES` is 1024 to 64 MiB. The Connect message limit is the 8 MiB input ceiling plus 64 KiB of envelope. Configuration `Debug` output redacts secrets and the database URL.

### API keys and credentials

A key is `dtk_` followed by the 32 hex digits of its UUID v7 and `_` and 43 URL-safe base64 characters of 32 random bytes. The service stores a domain-separated SHA-256 of the secret part, compares it in constant time (also for unknown key IDs) and returns the full token only in the `CreateKey` response. Unknown, mismatching, revoked and expired keys are indistinguishable authentication failures; an expiry instant is exclusive. `CreateKey` rejects an `expiresAt` that is not in the future and judges it again, and takes `createdAt`, after the tenant authority answers, so a slow authority call never yields a key that is already expired. `Authorization` accepts a case-insensitive `Bearer` scheme with exactly one token. A `dtk_` token is an API key; anything else is a Logto JWT validated for signature (RS256 or ES256; a key set entry whose `use` is not `sig`, whose `key_ops` lacks `verify` or whose `alg` differs from the token's algorithm never verifies it), issuer, audience, expiry and a five-minute `nbf` allowance, with throttled key-set refresh. The service reads at most 64 KiB of an OIDC discovery document and 256 KiB of a key set, at startup and on every refresh, and treats a larger response as a failed fetch. A fetched key set is trusted for at most five minutes: after that no token verifies from it until a refresh succeeds, so a signing key the provider retires stops working within that bound, and JWT authentication fails closed while the provider is unreachable (API keys do not depend on it). The key endpoint discovered from the issuer must satisfy the same URL rule as configured URLs (HTTPS, or loopback HTTP for fixtures); startup fails otherwise. A machine-to-machine token (subject equal to client ID) never acts as a user and never acquires the operator role.

Key management (`CreateKey`, `ListKeys`, `RevokeKey`) requires a Logto user token; API keys cannot manage keys. Personal-key management needs current tenant membership of the acting user and service-key management needs the Owner or Admin role, each checked with the tenant authority on every call. A personal key belongs to its creator; only that user revokes it. A service key records the issuing user for attribution only. `RevokeKey` is idempotent and keeps the first revocation time. A key that does not exist, belongs to another tenant, or is another user's personal key is reported as `FORBIDDEN`, so key IDs are not disclosed. `ListKeys` with a kind lists that kind (own personal keys, or service keys for Owner/Admin); without a kind it lists the caller's own personal keys plus service keys when the caller is Owner or Admin. Pagination orders by key ID with a default page of 50 and a maximum of 100. Key creation and revocation write safe operational events. `CreateKey`, `RevokeKey` and `UpdateSettings` run to completion even when the request deadline drops their caller, so a committed change is never separated from its audit record or, for settings, from publication to the live service. A client that receives `deadline_exceeded` from one of them cannot know the outcome: before retrying `CreateKey`, list the tenant's keys and revoke an unclaimed key whose secret it never received; read the settings back before retrying `UpdateSettings`. Capabilities are `TRANSLATOR`, `EXTRACTOR` or both, each at most once.

### Tenant authority

`platform::Platform` asks a `TenantAuthority` (`authority.rs`) for every management check and every new paid execution: `resolve_user` maps a verified user bearer to a platform user, `authorize_execution` takes the tenant, capability and consumer mode (`TenantService` for service keys, `User` for user tokens and personal keys) and answers allowed, forbidden or billing-blocked, and `authorize_management` answers for personal-key or service-key scopes. A question the authority cannot answer fails closed with Connect `unavailable` and reason `INTERNAL`. `Platform::authenticate_execution` implements the credential rules: a service key is tenant-level and requires its capability and tenant; a personal key additionally requires the user's current access; a user token acts as the user; billing blocks apply to all. `Platform::reauthorize` repeats the check immediately before a duplicate disclosure or an AI attempt and rejects revoked or expired keys. The in-memory implementation serves tests and local fixtures. The binary installs `delcore::DelcoreAuthority`, which calls delcore with the service's Logto machine-to-machine identity (the shared `server-common` `Authority`, scope `authorize`):

| Question                             | Delcore call                                                                                                                                                                               |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Resolve a Logto user token           | `GetCurrentUser` with the user's bearer token (the token must be accepted by delcore as well as by DeliTools). Rejection means "not a platform user"; delcore unavailability fails closed. |
| Service-key execution                | `Authorize` with `paid` true, the capability name (`translator` or `extractor`), an empty user and `CONSUMER_AUTHORIZATION_TENANT_SERVICE_KEY`.                                            |
| Personal-key or user-token execution | `Authorize` with `paid` true, the capability name and the user ID, in the unspecified mode.                                                                                                |
| Personal-key management              | `Authorize` with `paid` false, the user ID, no capability and `CONSUMER_AUTHORIZATION_PERSONAL_KEY_MANAGEMENT`.                                                                            |
| Service-key management               | The same with `CONSUMER_AUTHORIZATION_SERVICE_KEY_MANAGEMENT`.                                                                                                                             |

A denied answer with `BLOCK_REASON_PERMISSION` (or no reason) is forbidden; any other block reason (operator block, renewal, dispute, balance, deletion) is a billing block. An unreachable delcore, a failing token endpoint or an answer without a decision fails closed with Connect `unavailable` and reason `INTERNAL`. Tenant membership, roles and billing state are never cached.

### Operator RPCs

`GetSettings`, `UpdateSettings` and `InspectRequest` require a verified Logto user token whose signed `roles` claim contains `OPERATOR_ROLE`; tenant roles, machine tokens and API keys never qualify. `UpdateSettings` requires a reason of 3 to 1000 characters after trimming and a complete valid `Settings` message, and in one transaction replaces the settings and writes an audit record (actor `operator:<SHA-256 of issuer and subject>`, reason, request ID, changed names with previous and new values). The live settings snapshot changes after commit. `InspectRequest` returns state, attempts, frozen prices and measured usage, writes an audit record with the inspected request and no reason, and reports an unknown or expired request as Connect `not_found` with reason `INVALID_INPUT`. Rejected calls leave no audit record.

### Storage

Migration `00001` creates operator settings, API keys, audit records and operational events; `00002` creates catalog price versions (effective time, provider route and per-category USD prices, where a missing category means no published price), requests, attempts (frozen price snapshot, occurrence time, measured usage, delivery state, retry time) and idempotency records. Requests, attempts and idempotency records have no foreign keys to each other, and each attempt carries its own tenant, model and price snapshot, so every record expires on its own `created_at` and delivery never depends on another row. A database trigger rejects any change to `created_at` on every retained table. Tenant rate and concurrency state is in memory (`admission.rs`) and not persisted. No column holds input text, schemas, results or recoverable secrets.

### Background work and retention

The worker (`worker/`) runs registered `BackgroundTask`s, each on its own interval taken from the live settings. A failing run is logged, counted and retried at the next interval without stopping other tasks. The foundation registers retention (every five minutes) and backlog sampling (every 15 seconds); the binary registers usage delivery and Gateway reconciliation (both every `deliveryRetrySeconds`) and catalog price synchronization (every `priceSyncSeconds`, first run at startup) with `Worker::register`. Retention deletes idempotency records, attempts, requests, audit records and operational events whose `created_at` is at or before now minus 30 days, in batches, regardless of state, retry count or update time. API keys are never deleted by retention.

### Observability

Every response and error carries a server-issued UUID v7 request ID in `X-Delitools-Request-Id` (a client-supplied value is ignored) and in `ErrorDetail`. This includes Connect unary errors produced before a handler runs, such as an undecodable JSON or protobuf body, a body over the message limit, an unknown procedure or an unsupported content type: the service keeps the framework's HTTP status and Connect code (when the framework's error body is too large to inspect, it derives the code from the kept status rather than reporting an internal fault), replaces its message with a fixed English one that never quotes request content, and attaches the `ErrorDetail` (`INVALID_INPUT` for `invalid_argument`, including the bare 415 for an unsupported content type, which gains a JSON error body; `LIMIT` for `resource_exhausted`; `UNSPECIFIED` for `unimplemented`; `INTERNAL` otherwise). `/healthz` is process liveness. `/readyz` is false unless the schema is compatible and `catalog_prices` holds a synchronized price; the execution layer satisfies the rule by populating the table and may add checks with `Platform::add_readiness_check`. Until a price exists the service is intentionally not ready. `/metrics` exposes request counts and durations by procedure and bounded reason, failure counts by kind (authentication, authorization, authority unavailable, provider, catalog sync, missing price, usage delivery, worker), limit rejections, AI attempts by outcome (`success`, `transient`, `permanent`, `invalid_output`, `extraction_impossible`, `interrupted`) and AI retries, usage deliveries by outcome (`delivered`, `failed`), Gateway reconciliation lookups by outcome (`confirmed`, `not_yet`, `unreadable`, `unavailable`), catalog synchronizations by outcome (`ok`, `failed`), retention runs and deletions by table, worker runs by task, and gauges for unknown-usage attempts, pending deliveries, unknown requests, attempts nearing expiry, price availability and the two configured alerts. Tracing spans and logs carry the request ID, procedure and bounded outcome only.

## Execution pipeline

`exec::Executor` implements `TranslateTextArray`, `ExtractDataFromText` and `AssociateGeneration`. One call runs these steps in order, and every step before the AI call fails without contacting the Gateway:

1. Parse the tenant ID (UUID v7).
2. Authenticate and authorize through `Platform::authenticate_execution` (credential rules above, delcore tenant and billing check). Nothing about duplicates, limits or inputs is disclosed to an unauthorized caller.
3. Validate the input against the live operator settings (below). Validation failures consume no admission budget and create no record.
4. Answer a known unexpired duplicate of the idempotency key (below) before admission, so a duplicate neither consumes tenant budget nor waits.
5. Tenant admission: requests per minute and concurrency (`admission.rs`). A rejected call is `LIMIT`; a rejection by concurrency spends no rate budget. The concurrency slot is held until the execution task ends, not until the client leaves.
6. Resolve the catalog price for the model effective now. No price is `PRICE_UNAVAILABLE` (a missing-price failure counted in `delitools_failures_total{kind="missing_price"}`) and creates no record.
7. Create the request record (state `processing`, credential kind and attribution, frozen price version) and claim the idempotency key in one transaction. A concurrent first call that raced past step 4 is decided here by the primary key.
8. Run the attempt loop in a spawned task. The handler waits for it; if the client disconnects, the handler future is dropped, which signals cancellation to the task (see below).
9. Store the terminal state and start immediate delivery of every attempt with known usage. The idempotent terminal write is retried with a doubling pause (five attempts) so a transient storage error does not leave the request and its idempotency record `processing`. If every attempt fails, the failure is logged as `request_not_finalized`, the call still returns its outcome, and the next startup recovery marks the record `unknown`.

### Translator rules

- `outputLanguage` is required. `outputLanguage` and, when present, `inputLanguage` must be well-formed BCP 47 tags (RFC 5646 syntax, including private-use and the irregular grandfathered tags; registry membership is not checked). Tags compare case-insensitively after validation, so `en-US` and `EN-us` are identical explicit languages and are rejected; `en` and `en-US` differ.
- `texts` must not be empty, must not exceed `maxTranslationItems` (`LIMIT`) and must not contain an empty or whitespace-only item. The summed UTF-8 length must not exceed `maxInputBytes` (`LIMIT`).
- `model` must be in the operator allowlist `translationModels` (`INVALID_MODEL` otherwise, including an unknown, empty or merely cataloged model) and must have a synchronized price (`PRICE_UNAVAILABLE`).
- Without `inputLanguage` the prompt omits the source language, so the model detects one language for the whole array. A detected language equal to the output language, and mixed-language arrays, are still processed by the model; the service never returns the source array unprocessed.
- The prompt carries the target tag, the optional source tag and the texts as JSON string data in the user message (never as instructions) and requires strict structured output `{"translations": [...]}` with exactly one non-empty string per input item in the same order. A different length, a non-string or empty item, extra fields or unparseable content is `INVALID_OUTPUT` for the whole request: no partial result, no source-text substitution and no retry.

### Extractor rules and the supported JSON Schema subset

`ExtractDataFromText` always uses the Gateway model `openai/gpt-6-luna` (catalog entry <https://vercel.com/ai-gateway/models/gpt-6-luna>); the caller cannot select a model. The caller `jsonSchema` is validated completely before any AI call and an unsupported schema is `INVALID_SCHEMA` with an English message naming the schema location (never other schema content). The subset is implemented in `schema.rs` and verified by the fixtures below:

| Rule            | Supported                                                                                                                                                                                                                                                                                                    |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Document        | One JSON object without duplicate keys (a raw nesting limit of 64 protects the parser). Numbers keep their exact lexemes.                                                                                                                                                                                    |
| Root            | `"type": "object"`.                                                                                                                                                                                                                                                                                          |
| Keywords        | Only `type`, `description` (a string), `properties`, `required`, `additionalProperties`, `items` and `enum`. Any other keyword (`$ref`, `$defs`, `anyOf`, `oneOf`, `allOf`, `not`, `const`, `pattern`, `format`, `minimum`, `minLength`, `title`, ...) is rejected, which also rejects recursive references. |
| Types           | One type string per node: `object`, `array`, `string`, `number`, `integer`, `boolean`, `null`. Type lists are rejected.                                                                                                                                                                                      |
| Objects         | `properties` (names of 1 to 256 characters), `additionalProperties: false` and a `required` list that names every property exactly once; no optional properties and no undefined names. An empty object is allowed.                                                                                          |
| Arrays          | `items` with one schema. No other array keyword.                                                                                                                                                                                                                                                             |
| Enums           | Non-empty, unique, scalar values of the node's type on string, number, integer, boolean and null nodes; integer enum values must be whole numbers. Numeric uniqueness and equality use exact decimals.                                                                                                       |
| Nesting         | At most 8 levels; the root object is level 1 and objects and arrays each add one.                                                                                                                                                                                                                            |
| Provider budget | At most 5,000 properties (the envelope counts 2), 1,000 enum values and 120,000 characters of property names plus enum values; an enum of more than 250 values is limited to 15,000 string characters.                                                                                                       |
| Size            | `text` plus `jsonSchema` must not exceed `maxInputBytes` (`LIMIT`); an empty or whitespace-only `text` is `INVALID_INPUT`.                                                                                                                                                                                   |

The service reserves two provider nesting levels for its response envelope and uses one now: the provider receives a strict schema `{status, data}` where `status` is `"extracted"` or `"impossible"` and `data` is the caller schema. This is the extraction-impossible sentinel: the prompt instructs the model to use only information the text states or directly entails and never to invent a value; when required information is missing it sets `status` to `"impossible"` and fills `data` with placeholders that the service discards. `impossible` is `EXTRACTION_IMPOSSIBLE` (the call consumed tokens and is billed). With `extracted`, the service validates `data` against the accepted schema itself: exact types, whole-number integers (including exponent spellings), no extra or missing properties, exact enum comparison on decimals (no floating point) and bounded exponents. Anything else, or an unreadable envelope, is `INVALID_OUTPUT`. There is no repair retry and no second model. The returned `jsonData` is the validated value serialized compactly with keys sorted and number lexemes unchanged. Semantic correctness is evaluated with fixtures (`tests/execution.rs`) against a rule-based stand-in model; schema validation and prompting do not guarantee complete semantic accuracy.

Supported fixtures (all accepted):

```json
{"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}
{"type":"object","properties":{"kind":{"type":"string","enum":["a","b"]},"level":{"type":"integer","enum":[1,2,3]}},"required":["kind","level"],"additionalProperties":false}
{"type":"object","properties":{"lines":{"type":"array","items":{"type":"object","properties":{"sku":{"type":"string"},"qty":{"type":"integer"}},"required":["sku","qty"],"additionalProperties":false}}},"required":["lines"],"additionalProperties":false}
```

Unsupported fixtures (all rejected with `INVALID_SCHEMA` before any AI call): a non-object root, an object without `additionalProperties: false`, a property missing from `required`, `{"pattern": ...}` or any other validation keyword, `$ref`, `anyOf`, `const`, an array without `items`, an empty or duplicate enum, an integer enum with a fraction, nesting beyond 8 levels, more than 250 enum values with more than 15,000 string characters, and malformed JSON or duplicate keys. The full fixture set is in `schema.rs`.

### Attempts, retries, deadline and cancellation

- Settings are read once per call. The whole request, including retries and backoff, is bounded by `executionTimeoutSeconds`; the HTTP layer adds `TIMEOUT_SECONDS`. Expiry returns `TIMEOUT`.
- The deadline and a pending cancellation are checked immediately before every attempt record is stored, after the price lookup, request creation, retry pause and re-authorization. When either has passed, no attempt is stored and no Gateway call starts; the request ends as completed with `TIMEOUT` or `CANCELED`.
- Each attempt is one streamed Gateway call. Its record is stored first (attempt UUID, frozen price, occurrence time), then the call is made with `user` set to the attempt UUID and, when the frozen price has a provider route, `providerOptions.gateway.only` and `order` set to that route. No `models` fallback list is ever sent. The generation ID of the first stream chunk is stored at once. The response stream is bounded by `MAX_GATEWAY_RESPONSE_BYTES`.
- Transient failures are retried up to `maxAiRetries` additional attempts after a short bounded pause (100 ms doubling to 2 s, within the deadline): a connection failure, HTTP 408, 425, 429 and 5xx, an interrupted stream or a provider error event. Rejections (other 4xx, including authentication problems) are `PROVIDER` without retry. Truncated (`length`), filtered, refused or oversized output and malformed or nonconforming content are never retried (`INVALID_OUTPUT`); the same holds for extraction-impossible.
- Immediately before every retry the service repeats the credential, tenant and billing check (`Platform::reauthorize`). A revoked or expired key, a removed user permission or a billing block stops further attempts and the call returns that authorization error. An already running call is never interrupted by revocation or permission removal and its confirmed usage is billed.
- When the client cancels or disconnects, or the deadline passes, the in-flight HTTP call is dropped (closing the connection), retries stop, and the attempt is recorded with unknown usage. Usage reconciliation and delivery continue in the background. The request is stored as `unknown` when a call was in flight (a later duplicate of its idempotency key receives the unknown-outcome error), otherwise as completed with `CANCELED` or `TIMEOUT`.

### Error mapping

Every error carries `ErrorDetail` (reason and request ID), an English message and a Connect code; messages never contain inputs, schemas, results, credentials, provider bodies or database text.

| Reason                  | Connect code          | Cases                                                                                                                 |
| ----------------------- | --------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `INVALID_INPUT`         | `invalid_argument`    | Bad tenant, key, language tag, empty or blank item, empty array, identical explicit languages, empty extraction text. |
| `INVALID_SCHEMA`        | `invalid_argument`    | Schema outside the supported subset.                                                                                  |
| `INVALID_MODEL`         | `invalid_argument`    | Model not in the allowlist.                                                                                           |
| `UNAUTHENTICATED`       | `unauthenticated`     | Missing, malformed, unknown, revoked or expired credential.                                                           |
| `FORBIDDEN`             | `permission_denied`   | Capability, tenant or user permission missing; operator role missing; another tenant's key.                           |
| `BILLING_BLOCKED`       | `failed_precondition` | Delcore blocks new paid execution (operator block, renewal, dispute, balance, deletion).                              |
| `PRICE_UNAVAILABLE`     | `failed_precondition` | No synchronized catalog price for the model.                                                                          |
| `LIMIT`                 | `resource_exhausted`  | Input size, item count, tenant rate or tenant concurrency.                                                            |
| `PROCESSING`            | `aborted`             | A request with this idempotency key is still running (existing request ID).                                           |
| `ALREADY_PROCESSED`     | `already_exists`      | A request with this key was completed; no result is replayed (existing request ID).                                   |
| `INPUT_CONFLICT`        | `failed_precondition` | The key was used with a different input.                                                                              |
| `UNKNOWN_OUTCOME`       | `unknown`             | The earlier request with this key has an unknown outcome and is not repeated (existing request ID).                   |
| `EXTRACTION_IMPOSSIBLE` | `failed_precondition` | Required information is missing from the source.                                                                      |
| `INVALID_OUTPUT`        | `data_loss`           | Malformed, nonconforming, truncated, refused or oversized provider output.                                            |
| `PROVIDER`              | `unavailable`         | Provider failure after the allowed retries, or a rejected provider request.                                           |
| `TIMEOUT`               | `deadline_exceeded`   | The execution deadline passed.                                                                                        |
| `CANCELED`              | `canceled`            | The call was cancelled between attempts.                                                                              |
| `INTERNAL`              | `internal`            | Storage failure.                                                                                                      |
| `INTERNAL`              | `unavailable`         | Deliberate deviation: delcore or its token endpoint could not answer; new paid work fails closed.                     |
| `INVALID_INPUT`         | `not_found`           | Deliberate deviation: an operator target (request or attempt) does not exist or has expired.                          |
| unspecified             | `unimplemented`       | Deliberate deviation: a build without an installed execution service.                                                 |

For `PROCESSING`, `ALREADY_PROCESSED` and `UNKNOWN_OUTCOME` the `ErrorDetail` request ID and the `X-Delitools-Request-Id` header both carry the existing request's ID instead of the new call's ID.

### Optional idempotency

An idempotency key is 1 to 256 visible ASCII characters; an empty key means none and every call is a new, charged execution. The stored record holds the tenant, method, key, the request ID, a SHA-256 over the tenant, method and every input field (length-prefixed) and a state; the source, schema and result are never stored.

| Stored state | Duplicate with the same input                          | Duplicate with a different input |
| ------------ | ------------------------------------------------------ | -------------------------------- |
| processing   | `PROCESSING` immediately, with the existing request ID | `INPUT_CONFLICT`                 |
| completed    | `ALREADY_PROCESSED`, no replay                         | `INPUT_CONFLICT`                 |
| unknown      | `UNKNOWN_OUTCOME`, no re-execution                     | `INPUT_CONFLICT`                 |

A request is `unknown` when the process restarted while it was processing (startup recovery never resumes a prompt) or when a call in flight was cancelled or timed out. Definitive failures (including provider failure, invalid output and extraction-impossible) are `completed`. Authorization precedes every duplicate answer. A record older than 30 days from its first creation is treated as expired even before the retention sweep removes it, and the same key then starts a fresh, normally charged request with a new 30-day record; state changes never move `created_at`.

### Pricing and usage

- **Price versions.** `catalog_prices` holds one effective-dated version per change of a model's price. A request freezes the newest version effective at its start (price ID and effective time on the request, the full snapshot on every attempt), so retries and later reporting never see a newer price, and requests starting after a new version use it. A model without a version cannot execute.
- **Synchronization.** The price synchronization task runs at startup and every `priceSyncSeconds`. It reads the public `GET {GATEWAY_URL}/models` without credentials and prices only the allowlisted `translationModels` and the fixed extraction model of type `language`; the catalog never enables a model. Prices are canonicalized to plain non-negative decimals (at most 64 characters; exponent notation is accepted from the catalog and expanded). A model with context-tiered pricing (`*_tiers`) has no single flat price and keeps its last known version. The provider route is fixed with `GET {GATEWAY_URL}/models/{creator}/{model}/endpoints`: the alphabetically first active provider whose endpoint prices equal the public model price; a model with no such endpoint is not priced. A changed price or route inserts a new version effective at the observation time; an unchanged one only refreshes `synced_at`. A failed catalog fetch or write keeps every known version indefinitely, counts `catalog_sync` failures and writes a `price_sync_failure` operational event; readiness stays true once any price exists. A newly allowlisted model becomes executable at the next synchronization.
- **Measured usage.** A stream's `usage` is accepted as confirmed only when `prompt_tokens`, `completion_tokens`, `prompt_tokens_details.cached_tokens` and a cache-creation count (`cache_creation_input_tokens`, at the top level or in the details; both must agree when both are present) are all present and consistent. Prompt totals include cached and cache-creation tokens, so the plain input count is the remainder. Any missing or inconsistent count leaves the usage unknown; a missing cache count is never assumed zero. Provably unsent requests (a connection that was never established, HTTP 425, 429 and other 4xx rejections) are recorded as zero usage and have nothing to report; HTTP 408, a 5xx, an interrupted stream, a cancellation or a timeout may have consumed tokens (the Gateway can time out after it forwarded the request to the provider), so their usage is unknown and is reconciled.
- **Delivery.** See [Delcore usage report](#delcore-usage-report). Each attempt is claimed with a lease, reported with `IngestEvent`, and retried by the delivery task with a delay of `deliveryRetrySeconds` doubling per failure up to `maxBackoffSeconds`, until the record is deleted at 30 days. Reports use the `usage:write` scope token. Failures of every kind (token, transport, delcore rejection, an unpublished price category for nonzero usage) are retried; the first failure and every tenth are written as `usage_delivery_failure` events. An attempt at or past its 30-day boundary is neither delivered nor reconciled and is deleted by retention.
- **Reconciliation.** The reconciliation task looks up attempts with unknown usage and a stored generation ID with `GET {GATEWAY_URL}/generation?id=` (404 means "not yet", because usage events are ingested asynchronously). A lookup is accepted only when the returned `model` equals the attempt's model and `native_tokens_prompt`, `native_tokens_completion`, `native_tokens_cached` and `native_tokens_cache_creation` are present and consistent; the same remainder rule gives the plain input count. Accepted counts are stored and delivered immediately. Anything else is retried with the same backoff. Reconciliation never uses cost fields and never estimates; usage that cannot be established before the 30-day expiry is abandoned with the record. The assumption that native prompt totals include cached tokens follows the Gateway's OpenAI-compatible accounting and is part of the unperformed live-account acceptance.
- **Attempts without a generation ID** (for example a 5xx before the first chunk) cannot be looked up and stay pending until an operator associates a generation or the record expires.

### AssociateGeneration

Platform operators only. It requires a reason of 3 to 1000 characters, a UUID v7 attempt ID and a generation ID of 1 to 200 characters from `A-Z a-z 0-9 _ - . :`. The attempt must exist, have unknown usage, no generation and no delivery; the generation must be readable at the Gateway (`GET /generation`) for the attempt's stored model and must not be associated with another retained attempt (a unique index enforces it). On success the generation and an audit record (action `generation.associate`, reason, actor, tenant, attempt, request and generation ID) are stored in one transaction and normal reconciliation is scheduled. Rejected calls leave no audit record. The message carries no tokens, prices or fees, and inference is never replayed.

## Recovery and diagnostics

Use `InspectRequest` with the recorded request UUID to view request state, the safe terminal `error_reason`, attempt identifiers, generation IDs, price snapshots and measured usage; never collect original text or results. The inspection itself is audited.

**Gateway usage-reconciliation procedure** for an unknown outcome or an attempt with pending usage:

1. Inspect the request. Attempts with `usageKnown` false and a generation ID are reconciled automatically; watch `delitools_attempts_unknown_usage`, `delitools_usage_reconciliations_total` and `deliveryFailures`.
2. Where an attempt has no generation ID, find the Gateway generation through the safe attempt UUID sent as the provider `user` reference (Gateway request logs or the custom reporting API), verify tenant, model and time, then call `AssociateGeneration` with the attempt ID, generation ID and an audit reason. Normal reconciliation then reads the measured usage.
3. If evidence cannot establish usage, let the record expire. No charge is estimated, no AI call is replayed and no fee is entered by hand.
4. For a stuck delivery, check `delitools_usage_deliveries_total{outcome="failed"}` and the `usage_delivery_failure` events, and verify the delcore meters `translator-usd` and `extractor-usd`, the `usage:write` scope of the service, and the 30-day reporting windows of delcore. Delivery resumes by itself once delcore accepts the report; a repeated report is answered from delcore's retained identity and never charges twice.

Restart marks interrupted requests unknown and resumes usage processing from the stored attempts; it never resumes an AI prompt. Pending delivery uses a durable attempt UUID as both usage event identity and delivery request key; each attempt keeps its occurrence time and the original request pricing snapshot. Background retries use capped exponential delay until expiration. Deduplication, attempt, audit and operational-log deletion use each row's immutable creation time.

Alerts: `delitools_backlog_alert` (pending plus unknown attempts at `alertBacklog`), `delitools_expiry_alert` (undelivered attempts inside the `alertExpirySeconds` window before expiry), `delitools_catalog_price_available`, and the failure counters above. Diagnose a spike of provider failures with `delitools_ai_attempts_total` outcomes and the `ai_attempt_failed` log category (bounded cause, no bodies); authorization or billing check outages with `delitools_failures_total{kind="authority_unavailable"}`; missing prices with `kind="missing_price"` and the `price_unavailable` log category.

Monitor `/healthz`, `/readyz` and `/metrics` on a restricted network. Readiness is false until the catalog holds a synchronized price; later refresh failures preserve readiness with the last known prices. Prometheus reports pending and unknown usage, unknown requests, provider, authorization and catalog failures, missing prices and pending records near expiration, without tenant, model or request labels.

Back up before updates. Stop the old process, run forward migrations and start the matching binary. Startup validates the schema version and rejects a second instance through a PostgreSQL session lock. Fix defects forward; neither image rollback nor reverse migration is supported. Infrastructure provisioning, live account acceptance and deployment are separate from automated tests.

## Local development

Run service-local `pnpm dev` to inject the Infisical development folder `/delitools-api` and execute the binary built by the shared server builder, as for the other Rust APIs. Root `pnpm dev` starts the owning Compose database and includes the task. The disposable database uses `127.0.0.1:55434` by default, overridden by `DELITOOLS_POSTGRES_PORT`. Apply migrations explicitly with `delitools-api --migrate`; development launchers never run migrations.
