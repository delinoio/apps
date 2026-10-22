# DeliTools

DeliTools provides AI tools for developers through a Connect API. Version 1 has two synchronous methods: **translation** of a text array and **data extraction** from text into JSON that follows your schema. Each call returns the finished result or an error in the same response. There are no jobs to poll and no result to fetch later. All methods are on `delitools.v1.DeliToolsService`.

Call DeliTools from server-side code or the command line, with any Connect client or plain JSON over HTTP. Do not put an API key in frontend code. Set `DELITOOLS_URL` to the base URL of your DeliTools API server.

## Quick start

1. **Choose a credential.** Use your Delino login token, or create an API key with `CreateKey`. See the section Authentication and API keys.
2. **Find your tenant ID.** Usage is charged to the Delino tenant you name in `tenantId`.
3. **Call a method** with `Authorization: Bearer <credential>`.

```sh
# Translate two texts into Korean. Detect the input language.
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/TranslateTextArray" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","outputLanguage":"ko","model":"<allowed-model>","texts":["Good morning","Where is the station?"]}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000001",
  "texts": ["좋은 아침이에요", "역이 어디예요?"]
}
```

In JSON, field names use lowerCamelCase and enum values use their full names.

## Translate text

`TranslateTextArray` translates every string in an array.

| Field            | Required | Meaning                                                                                               |
| ---------------- | -------- | ----------------------------------------------------------------------------------------------------- |
| `tenantId`       | Yes      | The tenant that receives the usage charge. Use its canonical UUID version 7: lowercase, with hyphens. |
| `outputLanguage` | Yes      | BCP 47 language tag of the translation, for example `ko`, `pt-BR` or `zh-Hans`.                       |
| `inputLanguage`  | No       | BCP 47 language tag of the source. Omit it to detect one language for the array.                      |
| `model`          | Yes      | A model from the operator's allowlist.                                                                |
| `texts`          | Yes      | The strings to translate.                                                                             |
| `idempotencyKey` | No       | See Idempotency.                                                                                      |

The response contains `requestId` and `texts`, the translations in the original order and with the original length.

Rules:

- Language values are BCP 47 tags. `outputLanguage` is required. A tag that is not valid BCP 47, is not ASCII or is longer than 64 bytes returns an invalid-input error.
- `texts` must not be empty, and no string may be empty or contain only whitespace.
- If you set `inputLanguage` and it is the same as `outputLanguage`, the request is invalid.
- If you omit `inputLanguage`, DeliTools detects one input language for the whole array. A mixed-language array is still translated. If the detected language equals `outputLanguage`, the model still processes each item. DeliTools does not return the original array unprocessed.
- Only models on the operator allowlist are accepted. Other models return an invalid-model error. A model that exists in the Vercel AI Gateway catalog is not allowed automatically.
- The call succeeds only if every item succeeds. One failed item fails the whole request. DeliTools never returns partial translations and never substitutes the source text for a failed item.

## Extract data

`ExtractDataFromText` reads text and returns JSON that follows your JSON schema. The service uses a fixed model, `gpt-6-luna`. You do not choose the model.

| Field            | Required | Meaning                                                                                               |
| ---------------- | -------- | ----------------------------------------------------------------------------------------------------- |
| `tenantId`       | Yes      | The tenant that receives the usage charge. Use its canonical UUID version 7: lowercase, with hyphens. |
| `text`           | Yes      | The source text. It must not be empty or contain only whitespace.                                     |
| `jsonSchema`     | Yes      | A JSON schema, as a string, that uses the supported subset.                                           |
| `idempotencyKey` | No       | See Idempotency.                                                                                      |

The response contains `requestId` and `jsonData`, a JSON string that DeliTools validated against your schema.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/ExtractDataFromText" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","text":"Ada Lovelace lives in London.","jsonSchema":"{\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"},\"city\":{\"type\":\"string\"}},\"required\":[\"name\",\"city\"],\"additionalProperties\":false}"}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000002",
  "jsonData": "{\"name\":\"Ada Lovelace\",\"city\":\"London\"}"
}
```

How extraction works:

- The model is told to use only information that the text states. If the text lacks information that the schema requires, the call fails with an extraction-impossible error. The service does not invent a value to satisfy the schema.
- DeliTools checks the model output against your schema on the server. Malformed or nonconforming output is a failure with an invalid-output error. It is never returned as a result, and DeliTools does not retry or repair it.
- A schema check and a prompt do not guarantee that every value is semantically correct. Review extracted data before you rely on it. DeliTools does not return evidence for each value.
- DeliTools checks the schema before it calls any AI model. A schema outside the supported subset returns an invalid-schema error, with no AI call and no charge.

### Schema subset

DeliTools accepts only the part of JSON Schema that the structured output of the extraction model supports.

Supported:

- An object at the root.
- The types `object`, `array`, `string`, `number`, `integer`, `boolean` and `null`, nested to a depth of 8 levels. Each schema node has exactly one `type`, written as a string. A type array, such as `["string", "null"]`, is rejected.
- `properties`, with every property listed in `required`. A property name has 1 to 256 characters.
- `additionalProperties` set to `false` on every object.
- `items` for arrays.
- `description` on any schema.
- `enum` with scalar values. An enum must not be empty, its values must be unique and match the `type` of the node, and an `integer` enum takes whole numbers only.

Not supported, and rejected before execution:

- Any keyword outside the list above, for example `pattern`, `minimum`, `format` or `oneOf`.
- Optional properties. Every property of an object must be in `required`. `required` must list each property exactly once and must not name a property that `properties` does not define.
- `additionalProperties` set to `true`, or missing.
- References: `$ref` and recursive schemas.
- Schemas above the provider limits: 4,998 properties in total, 1,000 enum values, or 119,980 characters of property names and enum values together. The provider allows 5,000 properties and 120,000 characters in total, and DeliTools reserves the rest for its response envelope. An enum with more than 250 values may hold at most 15,000 characters of string values.

A supported schema:

```json
{
  "type": "object",
  "properties": {
    "invoiceNumber": { "type": "string", "description": "The invoice number" },
    "total": { "type": "number" },
    "status": { "type": "string", "enum": ["paid", "unpaid"] },
    "lines": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": { "item": { "type": "string" }, "quantity": { "type": "integer" } },
        "required": ["item", "quantity"],
        "additionalProperties": false
      }
    }
  },
  "required": ["invoiceNumber", "total", "status", "lines"],
  "additionalProperties": false
}
```

An unsupported schema, because `note` is optional and `pattern` is not supported:

```json
{
  "type": "object",
  "properties": {
    "code": { "type": "string", "pattern": "^[A-Z]{3}$" },
    "note": { "type": "string" }
  },
  "required": ["code"],
  "additionalProperties": false
}
```

## Request behavior

- Both methods are synchronous. Keep the connection open until you receive a result or an error.
- Every success and every error contains a request ID, a UUID version 7. Include it when you ask for support. Success responses contain only the result and the request ID. They do not contain the model, token counts or the charge. See Billing to inspect usage.
- DeliTools retries transient AI provider errors, up to a limit that the operator sets. It does not retry malformed output and never switches to another model on its own.
- If you cancel the call or disconnect, DeliTools tries to cancel the AI call and stops further retries. Usage that already happened is still measured and billed.
- The operator sets the limits for input size, translation item count, execution timeout, request rate per tenant and concurrent requests per tenant. These are deployment settings, not fixed product limits. A request over a limit returns a limit error.

## Authentication and API keys

Send `Authorization: Bearer <credential>` with each request. Two credential types work for both tools:

- **A Delino login token** (a Logto user access token) acts as the signed-in user.
- **A DeliTools API key** acts as its key kind describes.

Every request is also checked against your tenant in Delcore. The tenant must be active and allowed to run paid work, and billing must allow it. If DeliTools cannot complete these checks, it rejects new paid executions.

### Key kinds and capabilities

- A **personal key** belongs to one user. Its permissions are the overlap of its capabilities and that user's current permissions. When the user loses access to the tenant, the key stops working.
- A **service key** belongs to the tenant. It does not depend on the membership of the user who created it. Only a tenant Owner or Admin can create and manage service keys.
- Each key has one or both capabilities: `CAPABILITY_TRANSLATOR` and `CAPABILITY_EXTRACTOR`. A key can call only the tools for its capabilities.
- Users manage their own personal keys. Key management needs a Delino login token. An API key cannot create or revoke keys.

### Manage keys

- `CreateKey`: send `tenantId`, `kind` (`KEY_KIND_PERSONAL` or `KEY_KIND_SERVICE`), `capabilities` and an optional `expiresAt`.
- `ListKeys`: list the keys of a tenant that you may manage, filtered by `kind`, with `afterId` and `pageSize` for paging. `pageSize` defaults to 50 when you omit it or send 0. The maximum is 100, and a larger value returns an invalid-input error. You can never list another user's personal keys. With `KEY_KIND_PERSONAL` you get your own personal keys. With `KEY_KIND_SERVICE` you get the tenant's service keys, but only if you are an Owner or Admin. Without `kind`, you get your own personal keys, and also the tenant's service keys if you are an Owner or Admin.
- `RevokeKey`: send `tenantId` and `keyId`.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/CreateKey" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"tenantId":"<tenant-id>","kind":"KEY_KIND_SERVICE","capabilities":["CAPABILITY_TRANSLATOR"]}'
```

- **One-time secret.** The response of `CreateKey` contains the `secret` one time. DeliTools stores only a verification hash and cannot show the secret again. If you lose a key, create a new one. Keep the secret in a secret manager.
- **Expiry.** `expiresAt` is optional. A key without an expiry stays valid until you revoke it. A personal key also needs its owner to keep access to the tenant. A service key does not depend on the user who created it, so removing that user does not stop it: revoke the service key or give it an expiry.
- **Revocation.** After you revoke a key, or after the user of a personal key or login token loses permission, new requests are rejected and a running call makes no more AI retry attempts. A call that is already running can finish. Its usage is billed.

## Billing

DeliTools reports usage to Delcore. Delcore rates and settles it in USD.

- **Basis.** The basis is the public token prices of the model in the Vercel AI Gateway model catalog, multiplied by the tokens that the call actually measured. Gateway discounts and promotional credits do not reduce the basis.
- **Markup.** Translation costs the basis times 1.15. Extraction costs the basis times 1.30. Amounts use exact decimals and Delcore's rounding rules.
- **Price changes.** DeliTools updates the catalog prices automatically. A new price applies to every customer for requests that start after it takes effect. A request keeps the price from its start for all of its retries. If DeliTools has no price for a model, it rejects new AI execution.
- **Failures are billable.** Tokens used by retries, failed calls, timeouts, cancellations and results that you could not receive still count, together with the markup.
- **Unknown usage.** If DeliTools cannot read the usage of a call at once, it defers the charge and reconciles with the Gateway later. It never estimates usage for a charge. If usage is still unknown when the record expires after 30 days, that charge is dropped.
- **Where to see usage.** Responses contain no usage. Use the Delino billing interface for your tenant to inspect usage and charges.
- **Blocked tenants.** If billing blocks your tenant, new executions fail with a billing-blocked error. A call that already runs is not interrupted. Because DeliTools measures usage after execution, concurrent calls can exceed a balance or limit before the block applies.

## Idempotency

`idempotencyKey` is optional. If you send one, it must have 1 to 256 visible ASCII characters (no spaces or control characters); any other key returns an invalid-input error. Without it, every call is a new execution and can create a new charge, including when you retry after a network failure.

With a key, DeliTools tracks the request by **tenant, method and key**. This applies across different credentials of the same tenant. DeliTools checks your current authorization before it tells you the state of a request.

| Situation                                    | Result                                                                      |
| -------------------------------------------- | --------------------------------------------------------------------------- |
| New key                                      | The call runs normally.                                                     |
| Same key and same input, still running       | A processing error with the request ID of the first call. It does not wait. |
| Same key and same input, finished            | An already-processed error. The result is not replayed.                     |
| Same key and different input                 | An input-conflict error.                                                    |
| The first call ended in an unknown state     | An unknown-outcome error. DeliTools does not repeat the AI call by itself.  |
| Same key after the 30-day record has expired | A new call, charged normally.                                               |

DeliTools keeps the key, a hash of the input and the state. It does not keep the input or the result. Deduplication holds only for 30 days after the first call. Save the result when you receive it: a repeated call with the same key cannot return it again.

## Errors

Every error has a Connect code, a stable reason, a request ID and an English message. The reason and the request ID are in the `delitools.v1.ErrorDetail` error detail. Use the reason, not the message, in your code.

| Reason                               | Connect code          | Meaning                                                                                         |
| ------------------------------------ | --------------------- | ----------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`         | `invalid_argument`    | The request is invalid, for example an empty array or identical languages.                      |
| `ERROR_REASON_INVALID_SCHEMA`        | `invalid_argument`    | The schema is outside the supported subset.                                                     |
| `ERROR_REASON_INVALID_MODEL`         | `invalid_argument`    | The model is not on the allowlist.                                                              |
| `ERROR_REASON_UNAUTHENTICATED`       | `unauthenticated`     | The credential is missing, invalid, expired or revoked.                                         |
| `ERROR_REASON_FORBIDDEN`             | `permission_denied`   | The credential or user may not use this tool in this tenant.                                    |
| `ERROR_REASON_BILLING_BLOCKED`       | `failed_precondition` | Billing does not allow new paid execution for the tenant.                                       |
| `ERROR_REASON_PRICE_UNAVAILABLE`     | `failed_precondition` | No public price is available for the model, so DeliTools does not run it.                       |
| `ERROR_REASON_LIMIT`                 | `resource_exhausted`  | A configured limit was exceeded, such as input size, items, rate or concurrency.                |
| `ERROR_REASON_PROCESSING`            | `aborted`             | A request with the same idempotency key is still running.                                       |
| `ERROR_REASON_ALREADY_PROCESSED`     | `already_exists`      | A request with the same idempotency key and input already finished.                             |
| `ERROR_REASON_INPUT_CONFLICT`        | `failed_precondition` | The idempotency key was used with a different input.                                            |
| `ERROR_REASON_UNKNOWN_OUTCOME`       | `unknown`             | The outcome of the first execution is unknown.                                                  |
| `ERROR_REASON_EXTRACTION_IMPOSSIBLE` | `failed_precondition` | The text does not contain information that the schema requires.                                 |
| `ERROR_REASON_INVALID_OUTPUT`        | `data_loss`           | The model returned malformed output or output that does not match the schema.                   |
| `ERROR_REASON_PROVIDER`              | `unavailable`         | The AI provider failed after the permitted retries.                                             |
| `ERROR_REASON_TIMEOUT`               | `deadline_exceeded`   | The execution timeout passed.                                                                   |
| `ERROR_REASON_CANCELED`              | `canceled`            | The call was canceled.                                                                          |
| `ERROR_REASON_INTERNAL`              | `internal`            | An internal failure, for example in storage.                                                    |
| `ERROR_REASON_INTERNAL`              | `unavailable`         | The tenant authorization or billing check could not be completed, so new paid work is rejected. |

Handling:

- Fix the request for `invalid_argument`, `permission_denied`, `unauthenticated` and `failed_precondition` errors before you retry. Resolve the billing block for a billing-blocked error.
- A provider, timeout or internal error can be retried later. Provider and timeout failures can still be billed. Use an idempotency key to avoid a second execution when the first one may have succeeded. An `unavailable` error with `ERROR_REASON_INTERNAL` means that DeliTools could not complete the authorization or billing check. Branch on both the code and the reason, because `ERROR_REASON_INTERNAL` also appears with `internal`.
- Unknown, processing and already-processed errors contain the request ID of the first call. Contact support with that ID if the outcome stays unknown.

## Data retention

DeliTools never stores your input text, your schema or the results. It also keeps them out of logs, traces and audit events. Credentials and secrets are excluded from telemetry.

DeliTools stores only the data it needs to run the service and to bill it: verification hashes of API keys, who and which tenant a request belongs to, input hashes for idempotency, request states, attempt identifiers, measured token usage, price snapshots, and safe audit and operational records.

| Data                                        | Kept for                            |
| ------------------------------------------- | ----------------------------------- |
| Idempotency records                         | 30 days from first creation         |
| Usage delivery and reconciliation records   | 30 days from first creation         |
| Platform audit records and operational logs | 30 days from creation               |
| API keys                                    | Not deleted by the 30-day retention |
| Accepted accounting entries in Delcore      | Under Delcore's own retention rules |

The 30-day retention does not apply to API keys. The retention sweep never deletes them. Expiry and revocation only stop a key from authenticating, so its verification hash and attribution stay stored.

The 30 days start when the record is first created. Retries and state changes do not extend them. DeliTools deletes a record when it expires in every state, including requests that did not finish and usage that is not yet delivered. You accept two risks:

- After an idempotency record expires, a repeated call with the same key runs and is charged again.
- Usage that is still pending when its record expires is lost, and DeliTools does not charge it.

## External provider data handling

To do its work, DeliTools sends your text, and for extraction your schema, to the Vercel AI Gateway and from there to the provider of the model that runs your request. These services handle data under their own policies. Read them before you send data to DeliTools:

- [Vercel AI Gateway zero data retention](https://vercel.com/docs/ai-gateway/security-and-compliance/zdr)
- [Vercel AI Gateway models](https://vercel.com/ai-gateway/models): each model page lists its providers.
- The data policy of the provider of the model you use.

DeliTools does not promise zero data retention at any provider. It does not detect sensitive data and does not remove it from your input. Do not submit data that you may not share with these providers.

## Limits

The operator of the DeliTools API server sets the limits for input size, the number of items in a translation, the execution timeout, requests per minute per tenant, concurrent requests per tenant, the number of AI retries and the translation model allowlist. These are deployment settings and can change. They are not fixed product limits, and DeliTools makes no throughput or availability guarantee. A request that exceeds a limit returns `ERROR_REASON_LIMIT` or, for an invalid model, `ERROR_REASON_INVALID_MODEL`. Ask the operator of your deployment for its current values.
