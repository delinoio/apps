# DeliTools API

DeliTools provides synchronous text-array translation and schema-constrained extraction through Connect RPC. Each call returns the completed result and a request ID, or a typed error. There is no job queue or result-retrieval API. Ask your operator for the service base URL, tenant ID, eligible translation models and configured limits.

## Authentication and keys

Send `Authorization: Bearer <token>` with every request. Use a Logto user access token for the configured Delino resource audience, or a DeliTools API key. A valid identity must also have current tenant permissions and an unblocked billing account. New paid execution stops when these checks are unavailable.

Use `delitools.v1.DeliToolsService/CreateKey` with a user token. Set `tenantId`, `kind` (`KEY_KIND_PERSONAL` or `KEY_KIND_SERVICE`) and `capabilities` (`CAPABILITY_TRANSLATOR`, `CAPABILITY_EXTRACTOR`, or both). An optional `expiresAt` uses an RFC 3339 timestamp. The response returns the secret once. Store it securely; DeliTools cannot recover it. Create a replacement if you lose it.

Users manage their own personal keys. Personal keys remain limited by the user's current permissions. Tenant Owner/Admin manage service keys, which belong to the tenant and do not depend on the issuer's continued membership. `ListKeys` returns metadata without secrets; it accepts `tenantId`, `kind`, `afterId` and `pageSize` (1–100; default 50). Continue with `nextAfterId`. `RevokeKey` accepts `tenantId` and `keyId`.

Keys without an expiration remain valid until revoked, subject to current authorization. Revocation or removal of a user's permission rejects new executions and further AI retries. An already-running provider call may complete and remains billable. Tenant administrators do not gain platform-operator privileges.

## Translate an array

Call `delitools.v1.DeliToolsService/TranslateTextArray` with these JSON fields:

```json
{
  "tenantId": "YOUR_TENANT_ID",
  "outputLanguage": "ko",
  "model": "openai/gpt-6-luna",
  "texts": ["Hello", "Good morning"],
  "idempotencyKey": "translation-example-1"
}
```

Use BCP 47 language tags. `outputLanguage` is required; `inputLanguage` is optional. Without an input language, the model detects one language for the array as a whole and still attempts mixed-language input. Automatic detection of the output language does not skip model processing. Explicitly specifying identical input and output languages is invalid, as are an empty array and empty strings.

Only operator-allowlisted models are accepted. The response contains `requestId` and `texts`, preserving order and array length. Any failed item fails the whole request. No partial translations or source-text fallback are returned.

## Extract structured data

Call `delitools.v1.DeliToolsService/ExtractDataFromText`:

```json
{
  "tenantId": "YOUR_TENANT_ID",
  "text": "Ada is 37 years old.",
  "jsonSchema": "{\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"},\"age\":{\"type\":\"integer\"}},\"required\":[\"name\",\"age\"],\"additionalProperties\":false}",
  "idempotencyKey": "extraction-example-1"
}
```

Extraction always uses `openai/gpt-6-luna`; callers cannot select another model. The response contains `requestId` and `jsonData`, a JSON string that conforms to the accepted schema.

The supported schema subset has an object root and nested object, array, string, number, integer, boolean and null types. Use `properties`, `required`, `additionalProperties: false`, array `items`, optional descriptions and scalar `enum` values. Every object property must be required. References, arbitrary keywords and optional properties are rejected before AI execution. The caller schema supports eight nesting levels. The provider's combined schema limits are 5,000 properties, 1,000 enum values and 120,000 characters of property names and enum values; DeliTools reserves one property and five characters for its response envelope. Enums with more than 250 values may contain at most 15,000 characters of string values.

If required source information is missing, the model is instructed to return extraction-impossible instead of inventing values. The server validates the output schema and rejects malformed results. These checks and automated semantic fixtures do not guarantee perfect semantic accuracy. Review extracted facts before using them in consequential decisions.

## Idempotency and unknown outcomes

`idempotencyKey` is optional and can contain up to 200 bytes. Without it, every call is a fresh execution and can incur a charge. Deduplication uses tenant, method and key, including requests made with different credentials for the same tenant. Current authorization is checked before duplicate state is disclosed.

A matching in-progress call immediately returns processing with its existing request ID. A completed duplicate returns already-processed; it does not replay a result. Changed input under the same key returns input-conflict. Keep results on your side if you need them later.

An interruption can leave execution unknown. Do not retry it with a new key merely to recover a result: another execution can cause another charge. Give the request ID to your operator for state inspection and Gateway usage reconciliation. DeliTools does not replay the original request or accept manually entered fees.

Deduplication and pending-usage records expire 30 days after each record was first created, even if processing or reconciliation is unresolved. Retries do not extend this window. After deletion, the same key can start a new billable execution. Unresolved usage can be permanently lost at expiration.

## Billing and limits

Translation costs the applicable public Gateway token price multiplied by 1.15. Extraction uses 1.30. Actual measured token usage determines the cost; Gateway discounts and promotional credits do not reduce this basis. Delcore owns exact-decimal USD accounting and final rounding. Usage and charges appear through the separate billing interface, not in execution responses.

Prices are frozen when a request starts and remain fixed across its retries. Newly synchronized prices apply to later requests for every customer. Known prices remain usable during catalog outages without an age limit; a missing price blocks execution. Incurred usage remains billable after failure, timeout, cancellation or failed result delivery, including measured usage from retries. Unknown usage is reconciled rather than estimated. Post-execution billing cannot prevent all concurrent overspend.

Operators configure input bytes, translation item count, timeout, tenant request rate, concurrency and maximum transient retries. There are no universal product quotas documented here. The service does not retry malformed output or switch to another model. Client cancellation attempts to cancel provider execution and stops further retries; background usage accounting continues.

## Errors

Every application error contains an English message and `delitools.v1.ErrorDetail` with `reason` and `requestId`. Use the stable reason instead of parsing the message. Reason identifiers below omit the `ERROR_REASON_` prefix.

| Reason                                             | Connect code          | Action                                                           |
| -------------------------------------------------- | --------------------- | ---------------------------------------------------------------- |
| `INVALID_INPUT`, `INVALID_SCHEMA`, `INVALID_MODEL` | `invalid_argument`    | Correct the request.                                             |
| `UNAUTHENTICATED`                                  | `unauthenticated`     | Replace or renew credentials.                                    |
| `FORBIDDEN`                                        | `permission_denied`   | Check current tenant and key permissions.                        |
| `BILLING_BLOCKED`, `PRICE_UNAVAILABLE`             | `failed_precondition` | Restore billing access or contact the operator.                  |
| `LIMIT`                                            | `resource_exhausted`  | Reduce input or request pressure.                                |
| `PROCESSING`                                       | `aborted`             | The existing request is still running.                           |
| `ALREADY_PROCESSED`                                | `already_exists`      | Use the result you retained; there is no replay.                 |
| `INPUT_CONFLICT`                                   | `failed_precondition` | Do not reuse a key for changed input.                            |
| `UNKNOWN_OUTCOME`                                  | `unknown`             | Request operator inspection; do not automatically execute again. |
| `EXTRACTION_IMPOSSIBLE`                            | `failed_precondition` | Supply the missing source information.                           |
| `INVALID_OUTPUT`                                   | `data_loss`           | The model did not produce a complete valid result.               |
| `PROVIDER`                                         | `unavailable`         | A dependency failed; retain the request ID.                      |
| `TIMEOUT`                                          | `deadline_exceeded`   | Execution exceeded the configured deadline.                      |
| `CANCELED`                                         | `canceled`            | The caller canceled the execution.                               |
| `INTERNAL`                                         | `internal`            | Retain the request ID for diagnosis.                             |

## Data handling

DeliTools does not persist source text, schemas or results, and excludes them and credentials from logs, traces and audits. It retains input-comparison hashes, request states and usage attribution for the retention window. Platform audits and general operational logs also expire after 30 days. This retention policy does not expire API keys or delete accepted accounting records.

Text and schemas are sent to Vercel AI Gateway and the model provider. Gateway offers configurable retention and prompt-training controls; upstream handling depends on the provider and account configuration. Review [Vercel's data-handling explanation](https://vercel.com/i/what-is-an-ai-gateway) and [Gateway privacy controls](https://vercel.com/changelog/zero-data-retention-no-prompt-training-on-ai-gateway) before submitting data. OpenAI states that API data is not used for training by default, while abuse-monitoring retention and exceptions depend on its [data controls](https://developers.openai.com/api/docs/guides/your-data). DeliTools does not promise provider-wide zero retention or detect sensitive data for you.
