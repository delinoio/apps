# DeliTools API contract

## Scope

`servers/delitools-api` owns the independent API, explicit migrations and OCI packaging. Existing application and mobile release boundaries are unchanged.

## Runtime and Language

Go in the root module, PostgreSQL, one process and one running instance. API and background delivery, reconciliation, catalog synchronization and retention share the process.

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

- Deliver OCI image build support and explicit database migrations. Production infrastructure and deployment are the requester's responsibility.
- Check schema compatibility at startup. Document backup before updates and use **fix-forward only** for application/schema defects. Image rollback and reverse schema migrations are not supported recovery strategies for this scope.
- Provide structured `log/slog` logs, safe correlation IDs, health/readiness interfaces, Prometheus metrics, OpenTelemetry traces, and operational procedures. Require operator-configured alert thresholds.
- Cover AI failures/retries, authorization or billing-check failures, price-sync failures/missing prices, pending usage/reconciliation backlog, and records nearing expiration with operational diagnostics. Keep telemetry free of raw content and secrets and use bounded metric dimensions.
- No separate SLO or throughput guarantee is required. Verify the configured timeout, size, rate, concurrency, and failure behavior through automated tests.
- No feature flag applies: this is a new independent service, and release is controlled through deployment, authentication, and billing. These enforcement checks remain required without a feature flag.
- Provide state inspection and Gateway reconciliation procedures for unknown outcomes. Do not implement manual AI replay or manual fee entry.

## Storage

PostgreSQL stores only key verification hashes, authorization attribution, input hashes, request states, attempt identities, measured usage, price snapshots and safe audit/operational metadata. Never store inputs, schemas or results. Deduplication and usage records expire independently 30 days after creation, in every state. Keys do not expire with request records. Delcore accounting retention is independent.

## Security

Recheck authorization before duplicate disclosure and each AI attempt. Keep credentials out of database payloads and telemetry. Operator mutations require a dedicated role, reason and audit record. Inputs and schemas cross Gateway and provider boundaries; no provider-wide zero-retention promise applies.

## Logging

Use structured slog events, bounded metric dimensions and OpenTelemetry. Persist safe operational event categories and correlation IDs for 30 days; operators must apply the same TTL to exported telemetry. Never log upstream bodies, inputs, results or secrets.

## Build and Test

Run root Go tests, vet, server race tests, command builds, disposable PostgreSQL integration and mocked-provider OCI checks. Run root protocol lint, generation, freshness and FILE breaking checks. Preserve actual results and unperformed live-account checks in PR/CI records. Semantic fixtures test expected behavior without promising perfect extraction accuracy.

## Dependencies and Integrations

Use pinned Connect, pgx, OIDC and telemetry dependencies. Delcore owns rating, credits and settlement. Gateway owns model execution and usage lookup. Background delivery retries persist until expiry; unknown usage never becomes an estimated charge.

## Change Triggers

Update the project index, protocol and delcore contracts with boundary changes. Update scoped instructions only for development procedures or ownership.

## References

- [Project](project-delitools.md)
- [Repository defaults](repository-defaults.md)
- [Gateway REST API](https://vercel.com/docs/ai-gateway/sdks-and-apis/rest-api)
