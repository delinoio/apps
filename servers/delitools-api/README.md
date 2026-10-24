# DeliTools API

The DeliTools API provides paid, synchronous **translation** and **structured extraction** to external developers over Connect RPC (`protos/delitools/v1/delitools.proto`). It executes through the Vercel AI Gateway, authorizes every paid call through Delcore, and reports measured token usage to Delcore for billing. Server files use [FSL-1.1-ALv2](../LICENSE). The crate's rustdoc (`cargo doc -p delitools-api --open`) documents each module.

Delcore stays generic. It knows the DeliTools service, its `translate` and `extract` capabilities, generic meters, prices and events. It never sees models, Gateway behavior or submitted content. DeliTools owns the Gateway calls, the model allowlist, the mapping from models to meters, and the interpretation of provider usage.

## Contents

- [Request flow](#request-flow)
- [Authentication](#authentication)
- [RPCs](#rpcs)
- [Errors](#errors)
- [Extraction schema subset](#extraction-schema-subset)
- [Billing and the usage outbox](#billing-and-the-usage-outbox)
- [Delcore registration](#delcore-registration)
- [Configuration](#configuration)
- [Running](#running)
- [Operations](#operations)
- [Privacy](#privacy)
- [Validation](#validation)

## Request flow

1. Authenticate and authorize the caller with Delcore (`paid=true`). Fail closed when Delcore is unavailable or answers in an unexpected shape.
2. Admit by tenant request rate and tenant concurrency.
3. Validate the request (shape, limits, model allowlist, schema subset).
4. Verify through Delcore's catalog that the model's input and output meters have an active usage price. A missing price rejects the request; no other model is substituted.
5. Execute against the Gateway. Retry only transient failures (network, HTTP 5xx, HTTP 429) up to the configured budget, and re-run the authorization check before every retry.
6. Store the confirmed provider usage in PostgreSQL **before** the result is returned, then validate the output.
7. A background worker delivers stored usage to Delcore with `IngestEvent`.

Each call is a new execution and can incur a new charge. There is no caller idempotency and no result replay.

## Authentication

Send `Authorization: Bearer <token>`.

| Token                                                      | Check                                                                                                                                                                                                                                                                                                                                         |
| ---------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `dlk_...` Delcore API key (personal or tenant service key) | Delcore `AuthorizeApiKey` with the DeliTools service token, `paid=true` and capability `translate` or `extract`. Delcore verifies the secret, the grant for this service, the tenant and billing state and, for personal keys, the owner's current permission. `tenant_id` in the request is optional; if set it must equal the key's tenant. |
| Any other token (Logto user token)                         | `Authority::identify` resolves the user through Delcore `GetCurrentUser`; `tenant_id` is required; Delcore `Authorize{tenant, user, capability, paid=true}` decides.                                                                                                                                                                          |

Revocation, expiry, permission loss or a billing block blocks new requests and further retry attempts; a provider call that is already running may finish. The service token is a Logto client-credentials token with the `authorize usage:write` scopes.

Browser requests (any request with an `Origin` header) are rejected; there is no CORS.

## RPCs

Procedures: `delitools.v1.DelitoolsService/Translate` and `/Extract` over Connect (binary protobuf or JSON, unary, HTTP POST). Every response carries the request ID in the message and in the `X-Delitools-Request-Id` header.

**Translate** takes `texts` (1 to `DELITOOLS_MAX_TRANSLATION_ITEMS` non-empty strings, at most `DELITOOLS_MAX_INPUT_BYTES` UTF-8 bytes in total), a required `target_language` and an optional `source_language` (BCP 47: well-formed tags whose primary language subtag has two or three letters; private-use-only and grandfathered tags are rejected), and an operator-allowed `model`. It returns `translations` with the same order and length. One result is atomic: the model output must be a JSON array of exactly that many strings (the server asks for `{"translations":[...]}` and also accepts the bare array) or the whole request fails with `INVALID_OUTPUT`. Omitted source language, mixed-language input and same-language input are passed to the model without special handling.

**Extract** takes `text` and a `json_schema` string, and always uses the fixed model `openai/gpt-6-luna`. It returns `json`, a compact JSON document that was validated against the accepted schema (draft 2020-12, `jsonschema` crate). The model is told to answer only from the text and to report missing information through an explicit envelope (`{"status":"extracted","data":...}` or `{"status":"missing_information"}`); missing information is `EXTRACTION_FAILED`. The server never retries to repair invalid output.

Usage and charges are queried through Delcore, not returned here.

## Errors

Every error is a Connect error with an English message and a `delitools.v1.ErrorDetail{reason, request_id}` detail. Messages never contain submitted text, schemas, outputs or credentials.

| Reason                   | Connect code                               | Meaning                                                                                                                                                                      |
| ------------------------ | ------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `INVALID_INPUT`          | `invalid_argument`                         | Validation failed (empty array, empty string, bad language tag, missing model, malformed schema JSON, missing tenant for a user token).                                      |
| `UNAUTHENTICATED`        | `unauthenticated`                          | Missing, malformed, expired, revoked or unknown credential.                                                                                                                  |
| `PERMISSION_DENIED`      | `permission_denied`                        | Missing grant, lost permission, or a tenant that does not match the key.                                                                                                     |
| `BILLING_BLOCKED`        | `failed_precondition`                      | Delcore blocked paid execution (balance, renewal, dispute, operator block, deletion).                                                                                        |
| `LIMIT_EXCEEDED`         | `resource_exhausted`                       | Input size, item count or schema size over the configured limit.                                                                                                             |
| `RATE_LIMITED`           | `resource_exhausted`                       | Tenant rate or concurrency limit; `Retry-After` is set for the rate limit.                                                                                                   |
| `MODEL_UNAVAILABLE`      | `invalid_argument` / `failed_precondition` | Model not on the allowlist (`invalid_argument`) or without registered input and output prices (`failed_precondition`).                                                       |
| `UNSUPPORTED_SCHEMA`     | `invalid_argument`                         | The schema uses a construct outside the subset; the message names the unsupported keyword.                                                                                   |
| `PROVIDER_FAILED`        | `unavailable` / `internal`                 | Transient provider failures exhausted the retries (`unavailable`), or the provider rejected the request (`internal`).                                                        |
| `INVALID_OUTPUT`         | `internal`                                 | Malformed or nonconforming output, truncated output or an unreadable provider reply.                                                                                         |
| `EXTRACTION_FAILED`      | `failed_precondition`                      | The text does not contain information the schema requires.                                                                                                                   |
| `TIMEOUT`                | `deadline_exceeded`                        | `DELITOOLS_EXECUTION_TIMEOUT` elapsed (covers authorization, validation and execution).                                                                                      |
| `CANCELED`               | `canceled`                                 | The caller canceled.                                                                                                                                                         |
| `DEPENDENCY_UNAVAILABLE` | `unavailable`                              | Delcore (authorization, catalog) or PostgreSQL is unavailable, or answered malformed data. Paid execution fails closed. Also returned when confirmed usage cannot be stored. |

An unexpected internal failure uses `internal` with the unspecified reason.

## Extraction schema subset

A schema is a JSON object. Every schema node (the root, each property schema and the `items` schema) may use only:

- `type`: `object`, `array`, `string`, `number`, `integer`, `boolean`, `null`, or a non-empty array of distinct names (use `["string","null"]` for a nullable value);
- `enum` (1 to 64 distinct values) and `const`;
- `properties` (at most 64), `required` (names must be declared in `properties`) and `additionalProperties` (boolean only);
- `items` (one schema; tuple form is unsupported);
- `minLength`, `maxLength`, `minItems`, `maxItems` (integers from 0 to 1,000,000) and `minimum`, `maximum` (finite numbers), each minimum not above its maximum;
- the annotations `title` and `description`, and `$schema` on the root only.

Every node needs `type`, `enum` or `const`. The schema text may use at most `DELITOOLS_MAX_SCHEMA_BYTES` bytes, 8 nesting levels and 256 nodes. Everything else is rejected before execution with `UNSUPPORTED_SCHEMA`: `$ref`, `$defs`, `$id`, `oneOf`, `anyOf`, `allOf`, `not`, `if/then/else`, `patternProperties`, `pattern`, `format`, `prefixItems`, `contains`, `uniqueItems`, `multipleOf`, exclusive bounds, `default`, boolean schemas and unknown keywords.

## Billing and the usage outbox

- **Quantities.** Billed input is exactly `usage.prompt_tokens` and billed output exactly `usage.completion_tokens` from the Gateway response. Cached-token and reasoning-token breakdown fields are already inside those totals and are never added, discounted or surcharged. Missing or invalid totals are _unknown usage_: nothing is charged or estimated, `delitools_unknown_usage_total` counts it and a warning is logged. A zero quantity creates no event.
- **Prices.** DeliTools reads no Gateway prices and writes no Delcore prices. The operator registers selling prices in Delcore's catalog; Delcore attributes the price at the execution start (`occurred_at`).
- **Meters.** `DELITOOLS_MODELS` maps each allowed model to an input and an output meter key. Under Delcore's compatibility meter definition an event's type is the meter key and the event carries a numeric `quantity` attribute. Translation and extraction share these meters.
- **Every attempt with confirmed usage is billed**, including results that fail with invalid output, missing information or a cancellation that arrives after the usage was confirmed. The rows are written in PostgreSQL before the response is returned; storing (acquisition, up to three tries with backoff, and commit) shares one budget of `DEPENDENCY_TIMEOUT`, enforced with PostgreSQL `lock_timeout` and `statement_timeout` and a client deadline, so a held table lock cannot hold the request, its tenant slot or a pooled connection longer; if they cannot be stored within it the request fails with `DEPENDENCY_UNAVAILABLE` and the alert metric `delitools_usage_persist_failures_total` increments.
- **Stable identity.** Event keys are `<request_id>:<attempt>:in` and `<request_id>:<attempt>:out`; each row also keeps one Delcore request key for all deliveries. Delcore's service-scoped event key makes retries idempotent: an identical replay is success.
- **Delivery.** A worker reads due rows every `DELITOOLS_OUTBOX_INTERVAL`, drops rows of tenants that Delcore reports `DELETED`, and calls `IngestEvent`. Success deletes the row. Retryable failures (unavailable, rejected service token, missing meter, ...) use exponential backoff between `DELITOOLS_OUTBOX_BACKOFF_MIN` and `DELITOOLS_OUTBOX_BACKOFF_MAX`. Events Delcore can never accept (`EVENT_IDENTITY_CONFLICT`, `INVALID_ATTRIBUTE`, `INVALID_INPUT`, `REPORTING_WINDOW_CLOSED`) become `poisoned`: delivery stops, an error with `alert=true` is logged and `delitools_outbox_poisoned` shows the count.
- **Expiry.** Every row expires 30 days after its first creation in any state. Retries never extend it; a database check constraint and trigger enforce this. Expired undelivered usage is deleted and counted in `delitools_usage_expired_total`; accepted Delcore accounting keeps its own retention.
- **Alerts.** `delitools_outbox_alert` is 1 and an error is logged when pending rows exceed `DELITOOLS_OUTBOX_ALERT_BACKLOG` or the oldest pending row is older than `DELITOOLS_OUTBOX_ALERT_AGE`.
- Authorization does not reserve funds: concurrent requests can overspend, as with other post-execution billing in Delcore.

### Cancellation and timeouts

Each accepted request runs in its own task. If the caller disconnects, the in-flight Gateway call is aborted, no retry follows, and the task still finishes storing usage that was already confirmed. A provider call that is aborted has no confirmed usage and is not charged. When the execution timeout elapses the Gateway call is aborted the same way. The tenant concurrency slot is released when the task ends.

## Delcore registration

Register DeliTools as a Delcore consumer service before use:

- capabilities `translate` and `extract`, scopes `authorize` and `usage:write`;
- for each allowed model, two meters (input and output) with the compatibility definition and an active usage price on at least one plan;
- the Logto machine-to-machine application whose client ID and secret are `CLIENT_ID` and `CLIENT_SECRET`, for the `DELCORE_RESOURCE` audience;
- the service's UUID as `DELITOOLS_SERVICE_ID`.

DeliTools reads prices with `GetCatalog` (available to authenticated service callers) and never writes catalog data.

## Configuration

Copy the names in `.env.example` into the operator's environment. Development mode (`--dev-env`) loads an optional service-local `.env` without overriding injected variables; production commands never load it. Every value is validated at startup; the process refuses missing, non-positive or contradictory settings.

| Variable                                                          | Default                           | Meaning                                                                                                   |
| ----------------------------------------------------------------- | --------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `DATABASE_URL`                                                    | required                          | PostgreSQL.                                                                                               |
| `LISTEN`                                                          | `127.0.0.1:46322`                 | Listener address. Set `0.0.0.0:46322` in a container.                                                     |
| `DELCORE_URL`, `LOGTO_TOKEN_URL`                                  | required                          | https or loopback http only.                                                                              |
| `DELCORE_RESOURCE`, `CLIENT_ID`, `CLIENT_SECRET`                  | required                          | Service identity.                                                                                         |
| `DELITOOLS_SERVICE_ID`                                            | required                          | Delcore service UUID v7.                                                                                  |
| `AI_GATEWAY_API_KEY`                                              | required                          | Gateway credential.                                                                                       |
| `AI_GATEWAY_BASE_URL`                                             | `https://ai-gateway.vercel.sh/v1` | OpenAI-compatible endpoint.                                                                               |
| `DELITOOLS_MODELS`                                                | required                          | Allowlist file; JSON if the path ends in `.json`, otherwise YAML. Must list `openai/gpt-6-luna`.          |
| `DELITOOLS_MAX_INPUT_BYTES`                                       | 262144 (max 4 MiB)                | Total text bytes per request.                                                                             |
| `DELITOOLS_MAX_TRANSLATION_ITEMS`                                 | 200 (max 1000)                    | Items per translation.                                                                                    |
| `DELITOOLS_MAX_SCHEMA_BYTES`                                      | 32768                             | Schema bytes; not above the input limit.                                                                  |
| `DELITOOLS_MAX_OUTPUT_TOKENS`                                     | 16384                             | Completion tokens requested per attempt.                                                                  |
| `DELITOOLS_EXECUTION_TIMEOUT`                                     | `120s`                            | Whole-request deadline.                                                                                   |
| `DELITOOLS_TENANT_RATE_PER_SECOND`, `DELITOOLS_TENANT_RATE_BURST` | 5, 10                             | Tenant token bucket; burst not below rate.                                                                |
| `DELITOOLS_TENANT_CONCURRENCY`                                    | 4                                 | Concurrent executions per tenant.                                                                         |
| `DELITOOLS_MAX_RETRIES`                                           | 2 (max 5)                         | Retries after the first Gateway attempt.                                                                  |
| `DELITOOLS_RETRY_BASE_DELAY`, `DELITOOLS_RETRY_MAX_DELAY`         | `250ms`, `5s`                     | Retry backoff bounds; a `Retry-After` hint (seconds or HTTP-date) is honored up to the maximum.           |
| `DEPENDENCY_TIMEOUT`                                              | `3s`                              | Delcore call timeout, usage persistence budget and pool acquire timeout; not above the execution timeout. |
| `DELITOOLS_PRICE_CACHE_TTL`                                       | `30s`                             | Reuse of a successful price check. A missing price is re-read after 5 seconds.                            |
| `DELITOOLS_OUTBOX_INTERVAL`                                       | `5s`                              | Delivery cycle interval.                                                                                  |
| `DELITOOLS_OUTBOX_BATCH`                                          | 50 (max 500)                      | Rows per cycle.                                                                                           |
| `DELITOOLS_OUTBOX_BACKOFF_MIN`, `DELITOOLS_OUTBOX_BACKOFF_MAX`    | `5s`, `15m`                       | Delivery backoff bounds.                                                                                  |
| `DELITOOLS_OUTBOX_ALERT_BACKLOG`, `DELITOOLS_OUTBOX_ALERT_AGE`    | 1000, `15m`                       | Alert thresholds.                                                                                         |
| `LOG_FORMAT`, `NO_COLOR`, `OTEL_EXPORTER_OTLP_ENDPOINT`           | text, color, off                  | Logs are ANSI colored by default; `NO_COLOR=1` disables it and `LOG_FORMAT=json` emits JSON.              |

Durations accept `ms`, `s`, `m` and `h` units, for example `1m30s`.

## Running

Build from the repository root (Cargo needs `protoc`):

```sh
cargo build --locked --manifest-path servers/Cargo.toml -p delitools-api
./servers/target/debug/delitools-api migrate        # apply explicit migrations; safe to repeat
./servers/target/debug/delitools-api check-schema   # fail unless the schema matches this binary
./servers/target/debug/delitools-api                # serve
```

Normal startup never migrates. `serve` validates configuration, checks schema versions and digests, takes the PostgreSQL session lock `41840002` so only one process owns API execution and usage delivery, then listens. It stops if the lock session is lost. Root `pnpm dev` builds the server workspace once and starts this API through its Portless route on port 46322; service-local `pnpm dev` runs the already built binary.

The OCI image is built from the repository root with `docker build -f servers/delitools-api/Dockerfile .` (`pnpm --filter @delinoio/delitools-api ci:oci`). It contains the Rust executable and the server and protocol license notices, runs as a non-root user and exposes port 46322.

### Backup, update and fix-forward

1. Back up PostgreSQL and verify the backup.
2. Run `migrate` with the new image, then `check-schema`, then replace the running instance. Pending usage rows resume delivery after a restart.
3. Recovery is fix-forward: deploy a corrected image and, if data needs correction, a new migration. Previous-image rollback and reverse migrations are not supported; an older binary refuses a newer schema.

## Operations

- `/healthz` is liveness, `/readyz` checks schema compatibility, `/metrics` exposes Prometheus metrics with bounded labels (no tenant, request, model or content labels).
- Every request logs `request_id` and procedure on its span; the same ID is returned to the caller. Logs contain bounded categories, counts and token totals only.
- Metrics: `delitools_requests_total{procedure,outcome}`, `delitools_request_duration_seconds`, `delitools_denied_total{reason}`, `delitools_execution_failures_total{procedure,reason}`, `delitools_gateway_attempts_total{outcome}`, `delitools_unknown_usage_total{reason}`, `delitools_usage_persisted_total{direction}`, `delitools_usage_persist_failures_total`, `delitools_usage_deliveries_total{outcome}`, `delitools_usage_expired_total`, `delitools_outbox_pending`, `delitools_outbox_poisoned`, `delitools_outbox_oldest_pending_age_seconds`, `delitools_outbox_alert`.
- Tracing: spans `delitools.request` and `delitools.execute` export through OTLP when `OTEL_EXPORTER_OTLP_ENDPOINT` is set.

Troubleshooting:

| Symptom                                 | Check                                                                                                                                                                                                                                                                                                                                 |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `DEPENDENCY_UNAVAILABLE` for every call | Delcore reachability, `LOGTO_TOKEN_URL`, client credentials, the `authorize` and `usage:write` scopes. Delcore answers `unauthenticated` both for a bad API key and for a rejected service token, so a misconfigured service identity makes every key look invalid; check `delcore_token` and `delcore_authorization` log categories. |
| `MODEL_UNAVAILABLE`                     | The model is missing from `DELITOOLS_MODELS`, or its input and output meters have no active usage price in Delcore.                                                                                                                                                                                                                   |
| `delitools_outbox_pending` grows        | Delcore ingestion failing; inspect `usage_delivery_retry` logs by failure category.                                                                                                                                                                                                                                                   |
| `delitools_outbox_poisoned` > 0         | Delcore rejected an event permanently; inspect `usage_poison` logs and correct the catalog or accounting by hand within 30 days.                                                                                                                                                                                                      |
| `delitools_unknown_usage_total` grows   | The Gateway or provider omits `usage`; those attempts are uncharged by design.                                                                                                                                                                                                                                                        |

## Privacy

Submitted text, schemas, outputs and credentials are never persisted or logged. PostgreSQL holds only `usage_outbox` (tenant, event key, meter key, quantity, `occurred_at` and delivery state) and the schema version table. Content is sent to the Vercel AI Gateway and its provider; DeliTools not storing it does not constrain provider retention.

## Validation

Run these from the repository root:

```sh
cargo fmt --manifest-path servers/Cargo.toml --all --check
cargo clippy --locked --manifest-path servers/Cargo.toml -p delitools-api --all-targets -- -D warnings
cargo test --locked --manifest-path servers/Cargo.toml -p delitools-api
TEST_DATABASE_URL=postgres://... cargo test --locked --manifest-path servers/Cargo.toml -p delitools-api --test postgres -- --ignored
pnpm --filter @delinoio/delitools-api ci:oci
```

The mocked-service tests (`tests/service.rs`) start loopback mocks of Delcore, Logto and the Gateway and call the real HTTP surface. The PostgreSQL tests (`tests/postgres.rs`) create and drop an isolated schema per test in the database named by `TEST_DATABASE_URL`; use a dedicated disposable database. These checks use mocks and do not establish live Gateway, Logto or Delcore account acceptance.
