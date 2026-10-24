# DeliTools API server contract

The Rust runtime follows the [server Rust workspace contract](servers-rust-workspace-contract.md). Root development uses Turborepo watch mode with one shared Cargo builder and direct binary launchers.

## Scope

`servers/delitools-api` owns one independent Rust server, its PostgreSQL migration, usage-delivery worker and OCI packaging. It provides two paid, synchronous Connect RPC methods for external developers: translation of a text array and extraction of a JSON value from text. The schemas and generated bindings live in root `protos/delitools/v1` and the Go consumer example in `examples/delitools-client`; the [protocol contract](protos-delitools-v1-contract.md) owns the wire messages and this contract owns the behavior behind them.

The server calls Vercel AI Gateway for execution and delcore for authentication, authorization and token billing. It has no web console, REST endpoint, handwritten SDK, published client package, asynchronous job API, result history, stored source or result, caller idempotency or result replay. Each caller request is a new execution and can incur a new charge. Production deployment, live-account acceptance and numeric performance commitments are outside this contract. The `apps/delitools` application is a separate scaffold and does not call this server.

## Runtime and Language

Use the independent server Cargo workspace, Connect RPC and PostgreSQL. Run one server process against one database; the process holds the PostgreSQL session advisory lock `41840002` so a second instance cannot start, and it owns both API execution and usage delivery. `migrate` serializes on the advisory lock `41840001`. Request limiters and the delivery loop therefore need no cross-instance coordination, and the process stops when it loses the lock session. A detected lock loss is not an ordinary shutdown: see [Instance lock loss](#instance-lock-loss). Rust is the default server language and no deviation is documented. Root `pnpm dev` includes the server through its workspace script.

The binary has three commands: `serve` (the default), `migrate` and `check-schema`. `migrate` and `check-schema` need only `DATABASE_URL`; `serve` also validates every other setting.

## Users and Operators

- External developers and their servers, authenticated with a Delino login token, a personal API key or a tenant service API key. Browsers are not supported callers.
- Tenant Owners and Admins, who issue tenant service keys through delcore. Users, who issue personal keys through delcore. The [delcore protocol contract](protos-delcore-v1-contract.md#api-keys) and [delcore server contract](servers-delcore-api-contract.md#api-keys) own key behavior.
- Operators who deploy the server, register its delcore service identity, catalog entries and model allowlist, apply migrations and watch usage delivery.

## Interfaces and Contracts

### Methods

`delitools.v1.DelitoolsService` exposes `Translate` and `Extract` as Connect unary RPCs over HTTP (binary protobuf or JSON). Both require `Authorization: Bearer <credential>` and return a `request_id` that is also sent in the `X-Delitools-Request-Id` response header. A request ID is a UUID v7 generated for every invocation. It is not forwarded to delcore; delcore assigns its own correlation IDs.

Every failure produced by a handler carries a standard Connect code, `ErrorDetail` with a stable `ErrorReason` and the request ID, and an English message. The transport rejection below is the one exception. Messages never contain submitted text, schemas, outputs, credentials or provider payloads. The transport rejects a message larger than six times the sum of the input limit and the schema limit (the worst-case JSON escaping of one byte as `\u00XX`) plus a fixed 64 KiB allowance before any handler runs, with a plain Connect `resource_exhausted` error that has no `ErrorDetail`. A request that carries an `Origin` header is rejected with HTTP 403 before authentication. An unexpected internal failure uses `internal` with the unspecified reason. Responses do not report usage or charges; callers query delcore for those.

| Reason                                | Connect code                               | Meaning and caller action                                                                                                                                                                              |
| ------------------------------------- | ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `ERROR_REASON_INVALID_INPUT`          | `invalid_argument`                         | A field failed validation: empty array or string, bad language tag, missing model, missing or malformed tenant for a user token, a `json_schema` that is not valid JSON.                               |
| `ERROR_REASON_UNAUTHENTICATED`        | `unauthenticated`                          | The credential is missing, malformed, expired, revoked or unknown. A key request also receives it when the DeliTools service credential itself is rejected, because delcore answers both the same way. |
| `ERROR_REASON_PERMISSION_DENIED`      | `permission_denied`                        | The key lacks the service grant or capability, the user lacks tenant permission, or the request tenant differs from the key's tenant.                                                                  |
| `ERROR_REASON_BILLING_BLOCKED`        | `failed_precondition`                      | Delcore blocked paid execution for any non-permission reason (operator block, renewal, dispute, balance, deleting tenant).                                                                             |
| `ERROR_REASON_LIMIT_EXCEEDED`         | `resource_exhausted`                       | Input bytes, item count or schema bytes exceeded a configured limit. Reduce the input; retrying does not help.                                                                                         |
| `ERROR_REASON_RATE_LIMITED`           | `resource_exhausted`                       | The tenant request rate or concurrency limit was reached. `Retry-After` is set for the rate limit only.                                                                                                |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | `invalid_argument` / `failed_precondition` | The model is not on the allowlist (`invalid_argument`), or it is allowed but its input or output meter has no active usage price (`failed_precondition`). No other model is substituted.               |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | `invalid_argument`                         | The schema is not a JSON object or uses a construct outside the supported subset. Rejected before execution; the message names the keyword.                                                            |
| `ERROR_REASON_PROVIDER_FAILED`        | `unavailable` / `internal`                 | Transient Gateway failures exhausted the retries (`unavailable`), or the Gateway rejected the request permanently (`internal`).                                                                        |
| `ERROR_REASON_INVALID_OUTPUT`         | `internal`                                 | The model output was malformed, truncated, missing or nonconforming, or the Gateway reply was unreadable. Never retried; usage is charged when it was confirmed.                                       |
| `ERROR_REASON_EXTRACTION_FAILED`      | `failed_precondition`                      | The text lacks information that the schema requires. Usage is charged.                                                                                                                                 |
| `ERROR_REASON_TIMEOUT`                | `deadline_exceeded`                        | The execution timeout elapsed. It covers authorization, validation, the price check and every attempt.                                                                                                 |
| `ERROR_REASON_CANCELED`               | `canceled`                                 | The caller canceled or disconnected.                                                                                                                                                                   |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | `unavailable`                              | Delcore, the delcore catalog or PostgreSQL is unreachable or answered malformed data, or confirmed usage could not be stored. Paid execution fails closed.                                             |

Clients treat an unknown future reason as the failure of its Connect code.

### Authentication paths

A bearer token that starts with `dlk_` is an API key; any other token is a Logto user token. The server calls delcore only through its Connect API and never reads its database. Both paths use the DeliTools service token, a Logto client-credentials token with the scopes `authorize usage:write`, cached until shortly before it expires.

| Path             | Identify and authorize                                                                                                                                                         | Tenant                                                                                                                         |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------ |
| API key (`dlk_`) | `AuthorizeApiKey` with the service token, the secret, capability `translate` or `extract`, and `paid=true`. Delcore evaluates only grants of the calling service.              | From the key, which delcore returns. A non-empty request `tenant_id` must equal it, else `PERMISSION_DENIED`.                  |
| Logto user token | `GetCurrentUser` with the caller's token forwarded unchanged (through the shared authority), then `Authorize{tenantId, userId, capability, paid=true}` with the service token. | Required in the request and a canonical UUID v7, else `INVALID_INPUT`. Delcore checks current membership and group permission. |

Capabilities are `translate` and `extract`; both are declared by the registered DeliTools service. Map delcore answers as follows, and fail closed otherwise:

- `allowed=true` with no reason is allowed. `BLOCK_REASON_PERMISSION` among the reasons is `PERMISSION_DENIED`; any other reason, including reasons this server does not know, is `BILLING_BLOCKED`. `allowed=true` with reasons, `allowed=false` without reasons, a non-object body or a field of the wrong type is a malformed answer and `DEPENDENCY_UNAVAILABLE`. A valid key answer must also carry a canonical tenant ID.
- API key path: delcore `unauthenticated` is `UNAUTHENTICATED`, and the cached service token is dropped on that answer; every other failure is `DEPENDENCY_UNAVAILABLE`.
- User-token path: an authentication failure of the caller's token while identifying the user is `UNAUTHENTICATED`, a permission failure there is `PERMISSION_DENIED`, and any other identification failure is `DEPENDENCY_UNAVAILABLE`. The following `Authorize` call uses only the service token, so any failure of it, including a rejected service token, is `DEPENDENCY_UNAVAILABLE` and never `UNAUTHENTICATED`.
- Timeouts, transport failures, rejected service-token requests and non-success answers are `DEPENDENCY_UNAVAILABLE`.

Authentication and authorization run before request validation, rate limits and model checks. They are never cached: every request, and every retry attempt after the first, checks delcore again. Before a retry, the server repeats the same check with the key secret held in memory for the request, or with the user ID already identified. A retry stops, and the request fails with the matching reason, when the check no longer allows, the key's tenant changed or the check cannot be performed; usage of earlier attempts is still recorded. A provider call that is already running may finish. Revocation, expiry, lost personal permission and billing blocks therefore apply to new requests and to further retry attempts, not to a running call. The secret is never written to disk, logs, metrics, traces or the database.

### Limits and rate control

All limits are configured and validated at startup (see [environment configuration](#environment-configuration)); the server rejects missing, contradictory or out-of-range values; every limit must be positive except `DELITOOLS_MAX_RETRIES`, which may be 0 to disable retries. Defaults and bounds:

| Limit                                 | Setting                                                           | Default | Bound                            |
| ------------------------------------- | ----------------------------------------------------------------- | ------- | -------------------------------- |
| Input bytes (all texts, or the text)  | `DELITOOLS_MAX_INPUT_BYTES`                                       | 262144  | at most 4194304                  |
| Translation items                     | `DELITOOLS_MAX_TRANSLATION_ITEMS`                                 | 200     | at most 1000                     |
| Extraction schema bytes               | `DELITOOLS_MAX_SCHEMA_BYTES`                                      | 32768   | not above the input limit        |
| Provider output tokens per attempt    | `DELITOOLS_MAX_OUTPUT_TOKENS`                                     | 16384   | at most 1000000                  |
| Execution timeout (whole request)     | `DELITOOLS_EXECUTION_TIMEOUT`                                     | 120s    | see the monotonic clock bound    |
| Tenant request rate and burst         | `DELITOOLS_TENANT_RATE_PER_SECOND`, `DELITOOLS_TENANT_RATE_BURST` | 5, 10   | burst not below rate, at least 1 |
| Tenant concurrent executions          | `DELITOOLS_TENANT_CONCURRENCY`                                    | 4       | at least 1                       |
| Transient retries after the first try | `DELITOOLS_MAX_RETRIES`                                           | 2       | 0 to 5                           |
| Delcore call timeout                  | `DEPENDENCY_TIMEOUT`                                              | 3s      | not above the execution timeout  |

Input bytes count UTF-8 bytes of the submitted texts (or of the extraction text). Schema depth (8), schema nodes (256), properties per object (64) and enum values (64) are fixed. The rate limit and concurrency limit are process-local token bucket and counter keyed by the authenticated tenant ID, evicted when idle; extra credentials never add capacity. A rate rejection sets `Retry-After`. A limit is a protection, not a throughput guarantee.

**Monotonic clock bound.** Deadlines are monotonic instants. Startup rejects a `DELITOOLS_EXECUTION_TIMEOUT` that, together with the fixed 5-second transport margin (HTTP layer and connection write limit), cannot be represented as a duration or as an instant measured from the current monotonic clock. The shutdown and connection write allowances also include the persistence budget (`DEPENDENCY_TIMEOUT`); `serve` computes them with checked arithmetic and refuses to start, without panicking, if they are not representable. The error names the setting and never echoes the value; nothing is clamped. The largest accepted value depends on the platform's clock range, so operators should choose a practical timeout far below it. If a configuration that skipped validation ever reaches request handling, the request fails closed with the `INTERNAL` error instead of panicking.

Apply checks in this order: transport message bound, credential classification, delcore authentication and authorization, tenant rate and concurrency, request validation, model and price check, execution. The execution timeout starts when the request is received and covers all of these steps and every backoff; expiry before execution is `TIMEOUT`, and during execution it aborts the provider call. A shorter deadline set by the caller is not interpreted separately: when the caller closes the call, the request is canceled as for a disconnect.

The shared transport's 60-second socket idle-read interval does not shorten a completed-body HTTP/1 call: it is suspended from request-body completion until the response finishes, so the execution timeout is the only deadline for such a call, while a real client close still cancels the provider call. Follow the [server Rust workspace contract](servers-rust-workspace-contract.md#http-transport-limits).

### Translation

`TranslateRequest` carries `tenant_id`, `texts`, `target_language`, optional `source_language` and `model`.

- `texts` must be non-empty, contain no empty string, hold at most `DELITOOLS_MAX_TRANSLATION_ITEMS` items and at most `DELITOOLS_MAX_INPUT_BYTES` UTF-8 bytes in total. Whitespace-only strings are non-empty and are passed through. Validation order is shape, language tags, model presence, item count, size and allowlist.
- Language tags must be well-formed BCP 47 (RFC 5646 syntax), at most 64 characters, with a primary language subtag of two or three letters. Private-use-only and grandfathered tags are rejected, and subtags are not checked against the IANA registry or against model support. `target_language` is required; an empty `source_language` means the model detects the source. Omitted source language, mixed-language input and same-language input go to the model without special rejection and without automatically returning the source text.
- `model` must be exactly one of the operator-allowed models (case-sensitive match), otherwise the reason is `MODEL_UNAVAILABLE` with `invalid_argument`. Because the extraction model must appear in the same allowlist, it is also an allowed translation model. An allowed model without both selling prices is `MODEL_UNAVAILABLE` with `failed_precondition`.
- Execution is one atomic result. The model must return a JSON array of strings with exactly the length of `texts`, either bare or as the only member `translations` of an object (the server requests the object form through structured output, because structured-output formats need an object root, and also accepts the bare array). Any other shape, any non-string element, a length mismatch or a truncated response is `INVALID_OUTPUT`; there is no partial result. The response preserves the order and length of the request.

### Extraction

`ExtractRequest` carries `tenant_id`, `text` and `json_schema`. The model is fixed to `openai/gpt-6-luna` and is not selectable. It needs the same model-catalog mapping and both selling prices as a translation model; otherwise the reason is `MODEL_UNAVAILABLE`.

- `text` must be non-empty and within `DELITOOLS_MAX_INPUT_BYTES`; `json_schema` must be non-empty and within `DELITOOLS_MAX_SCHEMA_BYTES`. Size violations are `LIMIT_EXCEEDED`.
- The schema is checked against the supported subset below before any provider call. The submitted `text` is free text and never has to be JSON. A `json_schema` that is not valid JSON is `INVALID_INPUT`; a document that is not a JSON object or uses constructs outside the subset is `UNSUPPORTED_SCHEMA`.
- Ground extraction in the text. The instruction to the model forbids inventing values and defines an explicit envelope, separate from the caller's schema: `{"status":"extracted","data":...}` or `{"status":"missing_information","missing":[...]}`. Missing information is `EXTRACTION_FAILED`; the server never fills defaults or echoes the missing names.
- Validate `data` against the accepted schema with the `jsonschema` crate (draft 2020-12) and return it as compact JSON. Malformed JSON, a missing status or data, or a nonconforming value is `INVALID_OUTPUT`. Semantic accuracy is not guaranteed; automated semantic fixtures are examples, not accuracy claims.

**Supported JSON Schema subset.** A schema is a JSON object. Every schema node (the root, each property schema and the `items` schema) may use only the keywords below, and every node needs `type`, `enum` or `const`. The root may also carry `$schema` as a string, which the server ignores; the document is compiled as draft 2020-12.

| Keyword                                          | Rule                                                                                                         |
| ------------------------------------------------ | ------------------------------------------------------------------------------------------------------------ |
| `type`                                           | `object`, `array`, `string`, `number`, `integer`, `boolean`, `null`, or a non-empty array of distinct names. |
| `enum`, `const`                                  | `enum` has 1 to 64 distinct values; `const` takes any JSON value.                                            |
| `properties`, `required`                         | At most 64 properties per object. Every `required` name is distinct and declared in `properties`.            |
| `additionalProperties`                           | A boolean only.                                                                                              |
| `items`                                          | One schema. The tuple form (an array) is rejected.                                                           |
| `minLength`, `maxLength`, `minItems`, `maxItems` | Integers from 0 to 1,000,000, each minimum not above its maximum.                                            |
| `minimum`, `maximum`                             | Finite numbers, minimum not above maximum.                                                                   |
| `title`, `description`                           | Strings; annotations that never change validation.                                                           |

Also bounded: nesting depth 8 and 256 schema nodes in total. Everything else is rejected with `UNSUPPORTED_SCHEMA`, including `$ref`, `$defs`, `definitions`, `$id`, `$anchor`, `$comment`, `oneOf`, `anyOf`, `allOf`, `not`, `if`/`then`/`else`, `patternProperties`, `pattern`, `format`, `propertyNames`, `dependentRequired`, `dependentSchemas`, `unevaluatedProperties`, `unevaluatedItems`, `prefixItems`, `contains`, `uniqueItems`, `multipleOf`, `exclusiveMinimum`, `exclusiveMaximum`, `default`, boolean schemas and unknown keywords. References are never resolved, so a schema cannot cause a network or file read.

### Model catalog and prices

`DELITOOLS_MODELS` names a file, parsed as JSON when its path ends in `.json` and as YAML otherwise, that holds no secrets:

```yaml
models:
  - model: openai/gpt-6-luna
    input_meter_key: luna-input-tokens
    output_meter_key: luna-output-tokens
  - model: google/gemini-3-flash
    input_meter_key: gemini-flash-input-tokens
    output_meter_key: gemini-flash-output-tokens
```

Every entry is an allowed translation model, and the list must include the fixed extraction model `openai/gpt-6-luna`, whose entry also maps extraction. Validate at startup: the file parses without unknown fields, has 1 to 200 models, model identifiers are unique and use only `A-Za-z0-9._:/-` up to 200 bytes, meter keys use only `A-Za-z0-9._:-` up to 100 bytes, the input and output meter of one model differ, and no meter key is shared between models. A missing or invalid file stops startup.

Delcore catalog meters are generic. Register each meter key with the compatibility definition: event type equal to the meter key, `sum` over the numeric `quantity` attribute, no filters and no group-by fields. Usage unit prices are model-specific input and output token prices that operators set in delcore through its existing generic catalog; translation and extraction share them. DeliTools maps a model and token direction to its meters and adds no pricing of its own.

Before executing, verify through delcore `GetCatalog`, called with the service token and followed across pages of 100 (at most 200 pages), that an active `PRICE_KIND_USAGE` price with a plain nonnegative decimal `unit_price` exists for both mapped meters of the service whose ID is `DELITOOLS_SERVICE_ID`. Any absent price rejects the request with `MODEL_UNAVAILABLE` before any provider call; a failed, malformed or unpaginated catalog read is `DEPENDENCY_UNAVAILABLE`. The set of priced meters is cached for `DELITOOLS_PRICE_CACHE_TTL` (default 30s), and a miss is re-read after 5 seconds, so a removed price blocks new requests within the cache window. The check cannot see the paying tenant's own plan: delcore rates a meter only when a plan or add-on that the tenant holds contains a usage price for it, so operators must include DeliTools prices in the plans they sell, otherwise usage is aggregated but not charged. It does not verify the service capabilities. The server has no Gateway price lookup, no price synchronization, no markup and no price-write permission. Delcore attributes the effective price at the initial execution start when it accepts the usage event, so a price change during a request follows delcore's [call-time price](servers-delcore-api-contract.md#call-time-usage-prices) rules.

### Provider calls

The server calls Vercel AI Gateway through the OpenAI-compatible chat completions endpoint (`AI_GATEWAY_BASE_URL`, default `https://ai-gateway.vercel.sh/v1`) with `AI_GATEWAY_API_KEY`: non-streaming, `temperature` 0, `max_tokens` set to `DELITOOLS_MAX_OUTPUT_TOKENS`, a fixed instruction message and the submitted content as data in the user message, and a `json_schema` `response_format` (strict for translation). It sends only the requested model, never a fallback model list or provider-routing override, and never sends tenant, user, key or request identifiers to the Gateway. The server cannot control routing inside the Gateway; the usage reported for a response is the usage billed. A finish reason of `length`, a missing text content and an unreadable 2xx body are `INVALID_OUTPUT`. Responses larger than 8 MiB are treated as unreadable.

**Retries.** Transient failures are network errors, HTTP 5xx and HTTP 429 from the Gateway only. Retry at most `DELITOOLS_MAX_RETRIES` times. The wait before retry _n_ is `DELITOOLS_RETRY_BASE_DELAY` times 2^(n-1), capped at `DELITOOLS_RETRY_MAX_DELAY`, raised to a `Retry-After` hint up to that cap, with jitter between half and the full value. The hint is either delay-seconds (ASCII digits) or an HTTP-date (IMF-fixdate, RFC 850 or asctime) converted to the time remaining from the moment the response headers arrive; a date that is not in the future is a zero hint, and an absent, malformed, negative, fractional or overflowing value carries no hint. Exhausting retries is `PROVIDER_FAILED` (`unavailable`). Any other HTTP 4xx is `PROVIDER_FAILED` (`internal`) without retry. Invalid or nonconforming model output is never retried and never repaired, and no other model is substituted.

**Cancellation.** When the caller cancels or disconnects, the server aborts the in-flight provider call, stops further retries and returns `CANCELED` (the response may never reach the caller). The execution runs in its own task, so confirmed usage from a completed response is still persisted and delivered. An aborted provider call, whether by cancellation or by the execution timeout, yields no usage report and is therefore not charged, even if the provider already generated tokens that it bills to the Gateway account. This is a documented limit: the operator bears that cost.

**Instance lock loss.** A monitor probes the dedicated lock session every five seconds with a one-second timeout. When a probe fails, a replacement may already hold the lock, so this process no longer owns execution or delivery. The process fences itself at once, without the transport drain of an ordinary shutdown: it starts no further Gateway attempt and no retry (including the retry authorization check), cancels every unconfirmed in-flight provider call as for a caller cancellation (`CANCELED`, no usage report, no estimated usage), rejects requests that are admitted while the fence closes, and stops the delivery worker so no further delivery cycle or `IngestEvent` call starts. Every job's cancellation is a child of one service-wide fence, so a request cannot miss the signal because its admission races the loss. Usage that a provider response already confirmed is still stored, but only within the persistence budget (`DEPENDENCY_TIMEOUT` from when storing began, hence no later than `DEPENDENCY_TIMEOUT` after the fence closes); an unfinished write is reported as a persistence failure (`DEPENDENCY_UNAVAILABLE`, `delitools_usage_persist_failures_total`, `usage_persist` alert) and no usage is estimated. Stored rows stay in the outbox and the replacement delivers them with their original event and request keys; Delcore's idempotency makes an `IngestEvent` that was in flight at the loss safe to replay. The process then exits with a non-zero status after the short drain and the `instance lock session lost` error log; closing the database pool is bounded by `DEPENDENCY_TIMEOUT` in this case. Detection still takes up to the probe period; this contract does not fence earlier.

### Billing and usage delivery

Billing is post-execution. Authorization reserves nothing and does not guarantee that concurrent executions cannot overspend a balance or feature limit.

- **Measurement.** The input total is `usage.prompt_tokens` and the output total is `usage.completion_tokens` of the successful Gateway response. Cached-token and reasoning-token breakdown fields are already part of those totals; the server never adds them. No cache discount and no reasoning surcharge exist. Usage is valid when both values are present non-negative integers that fit a signed 64-bit integer; otherwise the attempt has unknown usage and neither direction is charged. A direction with zero tokens creates no event.
- **Which attempts.** Every attempt with confirmed usage is billed, including attempts that end in `INVALID_OUTPUT`, `EXTRACTION_FAILED`, a caller cancellation that arrives after the response, or a later failed retry, and attempts whose result is never delivered to the caller. Usage is stored before the model output is interpreted.
- **Unknown usage** (missing or invalid totals, or no response) is not charged and not estimated. The server writes a safe warning log and increments `delitools_unknown_usage_total`; it does no provider lookup or later reconciliation.
- **Durable outbox.** Before the result or error is returned, the server inserts one `usage_outbox` row for each non-zero direction of each attempt with confirmed usage in one PostgreSQL transaction, retrying the insert up to three times. The whole sequence (connection acquisition, transaction, retries, backoff and commit) shares one finite persistence budget equal to `DEPENDENCY_TIMEOUT`. It starts when persistence begins and is independent of caller cancellation and of the execution deadline. Each try sets the transaction-local PostgreSQL `lock_timeout` and `statement_timeout` to the remaining budget, so a conflicting table lock cannot hold the job, the tenant permit or a pooled connection beyond the budget. A connection whose operation did not finish within the budget is detached from the pool and closed, never reused. The pool also bounds connection acquisition with an acquire timeout equal to `DEPENDENCY_TIMEOUT`. An uncertain commit is safe to retry because `ON CONFLICT (event_key) DO NOTHING` keeps each event key unique and both directions of an attempt commit atomically. If persistence still fails, including when the budget is exhausted, the request fails with `DEPENDENCY_UNAVAILABLE`, an `alert` error log and `delitools_usage_persist_failures_total` record the possible lost charge, and no result is returned.
- **Event identity.** The delcore event key is `<request_id>:<attempt>:in` or `<request_id>:<attempt>:out`, where `attempt` starts at 1. The event carries the service's ID, the mapped meter key as event type and a single flat attribute `quantity`, a canonical integer decimal. `occurred_at` is the time the request's execution started, taken at microsecond precision once after admission and before the first provider attempt, and is the same for every attempt and every delivery retry. Each row also keeps one delcore request key that every delivery reuses.
- **Delivery.** A worker in the same process runs a cycle every `DELITOOLS_OUTBOX_INTERVAL`: it deletes expired rows, selects up to `DELITOOLS_OUTBOX_BATCH` due pending rows, oldest first, drops the rows of tenants delcore reports `DELETED`, and calls delcore `IngestEvent` with the service token. The service-scoped event key makes retries idempotent: success and an identical replay both delete the row. Outcomes:

| Delcore result                                                                                                                                      | Action                                                                                                                             |
| --------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| Accepted or identical replay                                                                                                                        | Delete the row.                                                                                                                    |
| `ERROR_REASON_INVALID_INPUT`, `ERROR_REASON_INVALID_ATTRIBUTE`, `ERROR_REASON_EVENT_IDENTITY_CONFLICT`, `ERROR_REASON_REPORTING_WINDOW_CLOSED`      | Mark `poisoned`: never delivered again and kept until expiry, an `alert` error log, and the `delitools_outbox_poisoned` gauge.     |
| Anything else, including transport failures, timeouts, `NO_MATCHING_METER`, `CARDINALITY_EXCEEDED`, a rejected service token and a forbidden answer | Keep pending and retry with exponential backoff from `DELITOOLS_OUTBOX_BACKOFF_MIN` doubling up to `DELITOOLS_OUTBOX_BACKOFF_MAX`. |

- **Backoff bounds.** Startup rejects a `DELITOOLS_OUTBOX_BACKOFF_MIN` or `DELITOOLS_OUTBOX_BACKOFF_MAX` that cannot be converted to a calendar delta or added to the current time, with an error that names the setting and omits its value; there is no other maximum. If a retry deadline still cannot be formed, the cycle fails with an `alert` error log of category `usage_schedule` after the first failing row, the row stays pending with unchanged attempts, state and expiry, and the worker logs `usage_cycle` and runs the next cycle.
- **Acknowledgment.** An HTTP-success `IngestEvent` body deletes the row only when it decodes as the generated `delcore.v1.IngestEventResponse` ProtoJSON message, accepting its legal integer-string and snake_case field-name forms, and carries a canonical UUID v7 `event_id` and `revision` exactly 1, which delcore returns for original acceptance and identical replay. A non-object body, a wrong-type field, an absent, null, empty or default identity or revision, a non-UUID v7 identity or any other revision is the retryable category `malformed_response`: the row stays pending with unchanged identity, content and expiry, its attempt count increases and the normal backoff applies. The returned event ID is not compared with the caller's keys, and correlation metadata and aggregation progress are not delivery conditions. The worker never logs the response body.
- **Error details.** The worker reads a delcore reason only from an error detail whose Connect `type` is exactly `delcore.v1.ErrorDetail`. It skips details with any other, absent or non-string type, including an `Any` URL form such as `type.googleapis.com/delcore.v1.ErrorDetail`, and details whose value is not valid base64 or protobuf, then examines the next detail. Without a usable reason, the Connect code selects the retry category in the table above. It never logs a delcore response body.
- **Alerts.** The worker logs an `alert` error and sets `delitools_outbox_alert` when pending rows exceed `DELITOOLS_OUTBOX_ALERT_BACKLOG` or the oldest pending row is older than `DELITOOLS_OUTBOX_ALERT_AGE`, and warns while any row is poisoned. Alert infrastructure is not part of this contract.
- **Expiry.** A row expires 30 days after its first creation, fixed in `expires_at` at insertion; a database check constraint and an update trigger enforce it. Every cycle deletes every expired row regardless of state, including undelivered and poisoned rows. Retries, state changes and delivery never extend it. Delivery stops at expiry: immediately before each `IngestEvent` the worker reads a fresh clock and, when that reading is at or after the row's `expires_at`, deletes the row without sending, retrying or poisoning it and counts `delitools_usage_expired_total` and logs `usage_expired` once for that row; later eligible rows of the same cycle continue unchanged. The same check repeats after the service token is acquired and immediately before the HTTP request, so a wait on the token lock or Logto cannot start a request after expiry. A request that already started is never canceled and completes through the normal result handling. The charge for an undelivered row is permanently lost; the server does not estimate usage to compensate. A row's `occurred_at` precedes its creation by the execution time, so delcore's minimum `USAGE_LATENESS_SECONDS` of exactly 30 days is not enough. Operators must configure delcore with `USAGE_LATENESS_SECONDS` of at least 30 days plus `DELITOOLS_EXECUTION_TIMEOUT` plus one `DELITOOLS_OUTBOX_BACKOFF_MAX`, and keep `REPORTING_GRACE_SECONDS` above it, so a live row is never refused for lateness alone. This is the shared 30-day consumer policy and is not DeliTools-specific. Accepted delcore accounting retains its separate policy.
- **Tenant deletion.** Each cycle asks `GetTenantStates` about the tenants of the due rows and deletes all rows of a tenant that is explicitly `DELETED`. `UNKNOWN`, `DELETING`, `BLOCKED`, `ACTIVE` and any failed lookup delete nothing.
- **Querying usage.** Callers and operators read usage and charges from delcore. Responses carry no usage.

### Prerequisites in delcore

Operators register the DeliTools service in delcore with a Logto machine-to-machine application, the scopes `authorize` and `usage:write`, and the capabilities `translate` and `extract`, and configure its UUID v7 as `DELITOOLS_SERVICE_ID`. They declare the meters of the [model catalog](#model-catalog-and-prices), set operator usage prices for them in the plans or add-ons they sell, and grant the capabilities to tenant groups for the user-token path. Registration stays outside Git.

## Storage

PostgreSQL is authoritative for service state and holds only the tables below. Entity IDs are UUID v7.

- `usage_outbox`: ID, tenant ID, request ID, attempt, direction (`in` or `out`), event key (unique), delivery request key, meter key, positive quantity, `occurred_at`, state (`pending` or `poisoned`), delivery attempts, `next_attempt_at`, last attempt time, last bounded failure category, `created_at` and `expires_at`. Delivered rows are deleted. A trigger keeps identity, content and expiry immutable. It holds no input text, schema, output, credential, user ID, key ID or model identifier.
- `delitools_schema`: the migration version, SHA-256 digest and application time of each applied file.

Migrations are explicit, ordered and immutable SQL files embedded in the binary. `delitools-api migrate` applies them in one transaction under an advisory lock and records the version and digest of each file; repeating it is a no-op. `serve` refuses to start, and `/readyz` reports unready, when the recorded schema differs from the binary's expected version or digests. `delitools-api check-schema` performs the same check without serving. An older binary refuses a newer schema. Normal startup applies no migration.

## Security

- Credentials: API key secrets and user tokens go only to delcore. The Gateway key, service client secret and database URL stay outside Git; configuration types print them redacted. Delcore, token and Gateway URLs must be `https` or loopback `http`; redirects are not followed.
- Privacy: never persist or log input text, schemas, outputs, API key secrets, tokens, the Gateway key or provider payloads. Persist only the outbox rows and the schema table. Logs contain request IDs, procedure names, bounded categories, attempt counts, token totals and outbox event keys; the admission log also names the tenant ID and credential kind (`api_key` or `user`), and the outbox holds tenant IDs for delivery. They never contain user IDs, key IDs or model-derived content. Error messages and diagnostics never echo submitted content. Metrics and spans carry no tenant, request, model or content labels.
- Provider disclosure: submitted text and schemas are sent to Vercel AI Gateway and the selected model provider. Local non-persistence is not a provider retention guarantee. Public documentation states this.
- Content is untrusted. The model receives instructions separately from content and has no tools. Results are checked structurally and returned only to the authenticated caller. Instructions embedded in submitted text may still affect the model output, which the caller must treat as untrusted.
- Browsers are not supported: there is no CORS and any request with an `Origin` header is rejected. Operational endpoints are unauthenticated and must be protected by network policy.
- Enforce tenant boundaries from the verified credential, never from an unverified request field. Use parameterized SQL only.
- Fail closed for paid execution when delcore authorization or catalog checks fail or are unavailable.

## Logging

Use `tracing` structured logs. Operator logs are colored key/value text by default; `NO_COLOR` disables color and `LOG_FORMAT=json` emits one JSON object per record. Request spans carry `request_id` and procedure; log events carry bounded `category` values such as `request`, `admitted`, `gateway`, `usage`, `unknown_usage`, `usage_persist`, `usage_delivery_retry`, `usage_poison`, `usage_backlog`, `usage_schedule`, `usage_cycle` and `usage_expired`, plus counts, durations and token totals.

Prometheus metrics are served at `/metrics`; labels are bounded:

| Metric                                        | Labels                 | Meaning                                                                                                   |
| --------------------------------------------- | ---------------------- | --------------------------------------------------------------------------------------------------------- |
| `delitools_requests_total`                    | `procedure`, `outcome` | Completed requests with the stable reason label or `ok`.                                                  |
| `delitools_request_duration_seconds`          | `procedure`            | End-to-end latency.                                                                                       |
| `delitools_denied_total`                      | `reason`               | Denied calls: invalid input, authentication, permission, billing, limit, rate, model, unsupported schema. |
| `delitools_execution_failures_total`          | `procedure`, `reason`  | Failures after admission: provider, output, extraction, timeout, cancellation, dependency.                |
| `delitools_gateway_attempts_total`            | `outcome`              | Gateway attempts: `ok`, `transient`, `permanent`, `invalid_output`, `canceled`, `timeout`.                |
| `delitools_unknown_usage_total`               | `reason`               | Attempts without confirmed usage (`missing`, `invalid`).                                                  |
| `delitools_usage_persisted_total`             | `direction`            | Outbox events stored, `in` or `out`.                                                                      |
| `delitools_usage_persist_failures_total`      | none                   | Outbox insert failures after retries or budget exhaustion (once per attempt).                             |
| `delitools_usage_deliveries_total`            | `outcome`              | `delivered`, `retry`, `poison`, `tenant_deleted`.                                                         |
| `delitools_usage_expired_total`               | none                   | Rows deleted at the 30-day expiry.                                                                        |
| `delitools_outbox_pending`                    | none                   | Pending rows (gauge).                                                                                     |
| `delitools_outbox_poisoned`                   | none                   | Poisoned rows (gauge).                                                                                    |
| `delitools_outbox_oldest_pending_age_seconds` | none                   | Age of the oldest pending row (gauge).                                                                    |
| `delitools_outbox_alert`                      | none                   | 1 while a backlog threshold is exceeded.                                                                  |

OpenTelemetry spans `delitools.request` and `delitools.execute` export through OTLP when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. `/healthz` reports process liveness; `/readyz` checks schema compatibility. Operational inspection starts with the request ID from a failing response: search structured logs for it, check the metrics above, run `check-schema` for schema questions, and use `GetMeteringStatus` in delcore for downstream processing lag. Numeric SLOs and dashboards are not part of this contract.

## Build and Test

- Rust unit tests for credential classification, request validation, language tags, the schema subset, retry and backoff decisions, usage parsing (cache and reasoning breakdown fields excluded), event keys, delivery state transitions, rate limiting, model catalog parsing and configuration validation.
- Mocked Gateway, delcore and Logto HTTP servers on loopback (`tests/service.rs`) call the real HTTP surface and cover: ordered translation arrays, Unicode and BCP 47 tags, empty inputs, disallowed and unpriced models, malformed and truncated output; extraction with supported and unsupported schemas, missing evidence, invalid output and the fixed model; user-token and API-key paths, tenant mismatch, delcore failure and malformed answers; each configured limit; transient retry exhaustion; no output-repair retry and no model substitution; cancellation and timeout; the instance fence (held call, retry backoff, admission racing the fence); revocation or permission loss between attempts; repeated requests executing anew.
- PostgreSQL integration tests (`tests/postgres.rs`; `#[ignore]`, disposable `TEST_DATABASE_URL`, one schema per test): migrations, digests and `check-schema`, outbox atomicity before the response, restart recovery, claim and backoff, expiry, tenant cleanup, and the absence of content and secrets from rows. Binary tests start the real `serve` process against disposable PostgreSQL and loopback mocks (one at a time, since the lock is database-wide) and cover instance lock loss by terminating only the lock backend: a held provider call dropped within one second, no attempt or authorization during retry backoff, delivery stopped while the transport drain is held open and then completed by a replacement, bounded confirmed-usage finalization, and the SIGTERM counterfactual that keeps the bounded drain and the lock until exit.
- Protocol lint, generation, freshness and FILE breaking checks from the repository root, plus the Go consumer example build and tests (`examples/delitools-client`).
- Server Cargo formatting, Clippy and tests under the server workspace contract; OCI build, `migrate`, `check-schema` and startup checks against disposable PostgreSQL.
- Root `cargo test` remains required whenever client Rust crates change, and frontend tests run when frontend code changes.

Fixtures and builds do not establish live Vercel AI Gateway, Logto, delcore or production acceptance. Record the source revision, commands, outcomes and unperformed checks in the pull request or CI record.

## Dependencies and Integrations

Connect Rust, SQLx, Prometheus, OpenTelemetry, the `jsonschema` validation crate, the shared `server-common` authority, limit, telemetry and transport modules, delcore through Connect RPC, Logto token issuance and Vercel AI Gateway. Rust bindings are generated by `servers/crates/delino-protos` from the canonical schemas. Calls to delcore use `GetCurrentUser`, `Authorize`, `AuthorizeApiKey`, `GetCatalog`, `GetTenantStates` and `IngestEvent`.

## Change Triggers

Update this contract, the [protocol contract](protos-delitools-v1-contract.md) and the [project index](project-delitools.md) together when behavior, limits, paths or deployment boundaries change. Update the [delcore protocol](protos-delcore-v1-contract.md) and [delcore server](servers-delcore-api-contract.md) contracts when the delcore calls used here change. Update the public pages in `apps/delino-docs` for user-visible changes. Record validation and unresolved limits in pull requests and CI records.

## Environment configuration

Shared settings have no service-name prefix; DeliTools-specific settings use `DELITOOLS_`. Required: `DATABASE_URL`, `DELCORE_URL`, `DELCORE_RESOURCE`, `LOGTO_TOKEN_URL`, `CLIENT_ID`, `CLIENT_SECRET` (this service's own delcore identity), `DELITOOLS_SERVICE_ID` (the delcore service UUID v7), `AI_GATEWAY_API_KEY` and `DELITOOLS_MODELS`. Secrets are `DATABASE_URL`, `CLIENT_SECRET` and `AI_GATEWAY_API_KEY`.

Optional with defaults:

| Setting                                                                                           | Default                           |
| ------------------------------------------------------------------------------------------------- | --------------------------------- |
| `LISTEN`                                                                                          | `127.0.0.1:46322`                 |
| `AI_GATEWAY_BASE_URL`                                                                             | `https://ai-gateway.vercel.sh/v1` |
| `DELITOOLS_MAX_INPUT_BYTES`, `_MAX_TRANSLATION_ITEMS`, `_MAX_SCHEMA_BYTES`, `_MAX_OUTPUT_TOKENS`  | See the limits table              |
| `DELITOOLS_EXECUTION_TIMEOUT`                                                                     | `120s`                            |
| `DELITOOLS_TENANT_RATE_PER_SECOND`, `DELITOOLS_TENANT_RATE_BURST`, `DELITOOLS_TENANT_CONCURRENCY` | 5, 10, 4                          |
| `DELITOOLS_MAX_RETRIES`                                                                           | 2                                 |
| `DELITOOLS_RETRY_BASE_DELAY`, `DELITOOLS_RETRY_MAX_DELAY`                                         | `250ms`, `5s`                     |
| `DEPENDENCY_TIMEOUT`                                                                              | `3s`                              |
| `DELITOOLS_PRICE_CACHE_TTL`                                                                       | `30s`                             |
| `DELITOOLS_OUTBOX_INTERVAL`                                                                       | `5s`                              |
| `DELITOOLS_OUTBOX_BATCH`                                                                          | 50 (at most 500)                  |
| `DELITOOLS_OUTBOX_BACKOFF_MIN`, `DELITOOLS_OUTBOX_BACKOFF_MAX`                                    | `5s`, `15m`                       |
| `DELITOOLS_OUTBOX_ALERT_BACKLOG`, `DELITOOLS_OUTBOX_ALERT_AGE`                                    | 1000, `15m`                       |
| `LOG_FORMAT`                                                                                      | `text`                            |

Durations use forms such as `250ms`, `3s`, `5m` and `1h30m`. Standard `OTEL_*` and `NO_COLOR` keep their names; PostgreSQL integration tests read `TEST_DATABASE_URL`. The Go example reads `DELITOOLS_URL` and `DELITOOLS_TOKEN`. Reject missing, contradictory or out-of-range values at startup without printing them; only `DELITOOLS_MAX_RETRIES` accepts 0. Commit only safe placeholders in `servers/delitools-api/.env.example` and `servers/delitools-api/models.example.yaml`; real configuration stays outside Git.

## Local development

`servers/scripts/dev-api.mjs` owns the development launcher shared by the APIs. Its environment handoff follows the [repository workflow](repository-workflow-contract.md#local-development-databases). The development command injects the Infisical `dev` variables of the `/delitools-api` folder using the local root project configuration; follow the [environment contract](repository-environment-contract.md#local-development-injection).

Run `pnpm dev` at the repository root for all development servers, or from `servers/delitools-api` for this API only. The script executes `servers/target/debug/delitools-api serve --dev-env` after the shared builder completes; `--dev-env` loads the optional ignored service-local `.env` without overriding injected variables, and production commands never read it. Root `pnpm dev` starts the local `delitools-postgres` Compose database and applies its URL as `DATABASE_URL` after Infisical injection; direct service-local development sets it itself. Run `delitools-api migrate` before starting; the launcher does not provision dependencies, apply migrations or bypass configuration and schema checks. Keep the local listener at `127.0.0.1:46322` (Portless name `delitools-api`). A port conflict fails startup.

## Operator procedures

- **Migrate:** run `delitools-api migrate` against the target database before starting a new version; it is safe to repeat. `check-schema` exits non-zero when the schema is missing, newer or modified.
- **Backup and update:** back up and verify the database before migrating. Run `migrate` and `check-schema` with the new image, then replace the running instance; rolling or multi-instance updates are not supported. Pending usage rows resume delivery after a restart. On shutdown (SIGTERM or interrupt) with the instance lock intact, the server stops accepting connections and drains in-flight requests within the execution timeout plus the persistence budget (`DEPENDENCY_TIMEOUT`), then closes the database pool within one more `DEPENDENCY_TIMEOUT`; requests that remain are canceled, and usage already confirmed is persisted. A lost lock session instead fences execution and delivery immediately, as described under instance lock loss, and the process exits with a non-zero status; start the replacement only after the failed instance is gone or has lost the lock.
- **Fix-forward recovery:** correct a defect with a new image and, if state needs correction, a new forward migration. Do not roll back to a previous image and do not write reverse migrations. A restored backup can redeliver rows that delcore already accepted (an identical replay succeeds) and lacks rows created after the backup, whose charges are lost.
- **Delivery backlog:** a growing backlog or oldest age usually means delcore, the service credential, the scopes or the catalog is unavailable or misconfigured. Check delcore health and the service registration first. Poisoned rows need a catalog or accounting fix by hand and are not retried; they expire after 30 days.
- **Every key looks invalid or every call is `DEPENDENCY_UNAVAILABLE`:** check the service credential, `LOGTO_TOKEN_URL` and the `authorize` and `usage:write` scopes. Delcore answers `unauthenticated` both for a bad key and for a rejected service token on the key path.
- **Model or price changes:** edit the model file and restart; register prices in delcore first. Removing a model from the file blocks new requests for it only.
- **Gateway failures:** rising `PROVIDER_FAILED` and transient attempt metrics point to Gateway or provider incidents; do not change code paths to substitute models.

## Known limits

No caller idempotency or result replay exists. Unknown usage and expired undelivered usage are never charged, and an aborted provider call is not charged although the provider may bill the Gateway account for it. Concurrent requests can overspend because authorization reserves nothing. One instance serves all traffic. Rate and concurrency limits are process-local. Request limits do not certify throughput. Previous-image and reverse-migration recovery are not provided for this server.

Out of scope: Gateway catalog-price synchronization, cost-based markups, automatic selling-price changes, AI-specific delcore billing logic; application or management UI, a separate SDK, asynchronous job or result APIs, result history, stored source or results, caller idempotency and result replay; editing or renewing API keys, automatic model fallback, invalid-output repair retries, estimated billing and provider usage lookup or reconciliation; feature flags, production deployment, live-account acceptance as a completion gate, performance guarantees, separate support tools and training.

## References

- [Repository defaults](repository-defaults.md)
- [Project index](project-delitools.md)
- [Protocol contract](protos-delitools-v1-contract.md)
- [Delcore server](servers-delcore-api-contract.md)
- [Delcore protocol](protos-delcore-v1-contract.md)
- [Environment contract](repository-environment-contract.md)
- [Vercel AI Gateway OpenAI-compatible API](https://vercel.com/docs/ai-gateway/openai-compat)
- [Vercel AI Gateway model catalog](https://vercel.com/ai-gateway/models/gpt-6-luna)
