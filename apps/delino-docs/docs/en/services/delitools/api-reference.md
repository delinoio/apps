# DeliTools API reference

This page describes the two DeliTools methods. Both are Connect procedures on `delitools.v1.DelitoolsService` and accept JSON over HTTP `POST`. Field names are lowerCamelCase in JSON. Send `Content-Type: application/json` and `Authorization: Bearer <credential>`. See [DeliTools](/services/delitools/) for credentials and tenants.

Every response includes a request ID, in the body and in the `X-Delitools-Request-Id` header. Responses do not include token usage or charges. Read those from Delcore; see [Billing](/services/delitools/billing).

Each call is a new execution. DeliTools has no idempotency key and no stored results. If you repeat a request, it runs again and can be charged again.

## Translate

`POST /delitools.v1.DelitoolsService/Translate`

| Field            | Type            | Rules                                                                                                |
| ---------------- | --------------- | ---------------------------------------------------------------------------------------------------- |
| `tenantId`       | string          | Required for a Delino login token. For an API key, empty or the tenant of the key.                   |
| `texts`          | array of string | Required. At least one item. No empty string. Item count and total size are limited by the operator. |
| `targetLanguage` | string          | Required. A BCP 47 language tag, such as `fr` or `pt-BR`.                                            |
| `sourceLanguage` | string          | Optional. A BCP 47 language tag. If omitted, the model detects the source language.                  |
| `model`          | string          | Required. A model identifier that the operator allows.                                               |

The response contains `requestId` and `translations`. `translations` has the same number of items in the same order as `texts`.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"sourceLanguage":"en","targetLanguage":"fr","model":"<model-id>"}'
```

```json
{
  "requestId": "<request-id>",
  "translations": ["Bonjour", "Bonjour, bonne journée"]
}
```

- **Language tags.** DeliTools checks that a tag is well formed under BCP 47, ignoring case, with at most 64 characters and a two- or three-letter primary language. Tags made only of private-use subtags and grandfathered tags are rejected. DeliTools does not check that the model supports the language.
- **All or nothing.** The result is atomic. If any item fails, the whole request fails and you receive no partial translations.
- **No special cases.** An omitted source language, mixed-language input and text that is already in the target language all go to the model. DeliTools does not reject them and does not copy the source text for you.
- **Model.** Only models that the operator allows, and that have prices registered in Delcore, work. Another model is never substituted. Ask the service operator which model identifiers are available.

## Extract

`POST /delitools.v1.DelitoolsService/Extract`

| Field        | Type   | Rules                                                                                          |
| ------------ | ------ | ---------------------------------------------------------------------------------------------- |
| `tenantId`   | string | Required for a Delino login token. For an API key, empty or the tenant of the key.             |
| `text`       | string | Required and not empty. Size is limited by the operator.                                       |
| `jsonSchema` | string | Required. A JSON Schema document, as a JSON string, that uses only the supported subset below. |

The model is fixed to `openai/gpt-6-luna`. You cannot choose it. The response contains `requestId` and `json`, a JSON string that conforms to your schema.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Extract" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"text":"Please send three boxes of green tea.","jsonSchema":"{\"type\":\"object\",\"properties\":{\"item\":{\"type\":\"string\"},\"quantity\":{\"type\":\"integer\",\"minimum\":1}},\"required\":[\"item\",\"quantity\"],\"additionalProperties\":false}"}'
```

```json
{
  "requestId": "<request-id>",
  "json": "{\"item\":\"green tea\",\"quantity\":3}"
}
```

- **Grounded in your text.** The model must take values from the text. If the text lacks required information, the request fails with `ERROR_REASON_EXTRACTION_FAILED`. DeliTools does not invent values.
- **Checked output.** DeliTools validates the result against your schema. Malformed or nonconforming output fails with `ERROR_REASON_INVALID_OUTPUT`. DeliTools does not retry to repair invalid output.
- **Accuracy.** A result that follows the schema can still be wrong. Check results that matter.
- **Unsupported schemas fail first.** DeliTools rejects a schema outside the supported subset before it runs the model, so such a request is not charged.

### Supported JSON Schema subset

The schema is read as JSON Schema 2020-12. It must be a JSON object. `$schema` is optional and allowed only at the root, where DeliTools ignores its value. Every schema, including the root, each property schema and the `items` schema, needs `type`, `enum` or `const`. A schema can be at most 8 levels deep and have 256 schema nodes, and its size is limited by the operator.

| Keyword                  | Rule                                                                                                                                                            |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `type`                   | `object`, `array`, `string`, `number`, `integer`, `boolean`, `null`, or an array of these types, for example `["string","null"]` for a nullable value.          |
| `properties`, `required` | `properties` maps names to schemas. Every `required` name must appear in `properties`. At most 64 properties per object, and `required` names are not repeated. |
| `additionalProperties`   | `true` or `false` only (not a schema).                                                                                                                          |
| `items`                  | One schema for all items.                                                                                                                                       |
| `enum`, `const`          | `enum` has 1 to 64 distinct values. `const` is a single value.                                                                                                  |
| `minLength`, `maxLength` | Integers from 0 to 1,000,000, for strings. The minimum cannot exceed the maximum.                                                                               |
| `minItems`, `maxItems`   | Integers from 0 to 1,000,000, for arrays. The minimum cannot exceed the maximum.                                                                                |
| `minimum`, `maximum`     | Finite numbers, for `number` and `integer`. The minimum cannot exceed the maximum.                                                                              |
| `title`, `description`   | Notes for the model. They do not change validation.                                                                                                             |

These are not supported and return `ERROR_REASON_UNSUPPORTED_SCHEMA`: `$ref`, `$defs`, `definitions`, `$id`, `$comment`, `oneOf`, `anyOf`, `allOf`, `not`, `if`/`then`/`else`, `patternProperties`, `propertyNames`, `pattern`, `format`, `uniqueItems`, `multipleOf`, `exclusiveMinimum`, `exclusiveMaximum`, `contains`, `prefixItems`, `default`, `unevaluated` keywords, boolean schemas, unknown keywords, and schemas that are too large or too deeply nested. DeliTools never follows references, so a schema cannot make it fetch anything.

Set `"additionalProperties": false` on objects when you want only the properties you listed.

## Errors

Errors use a standard Connect code. The error details include a stable reason and the request ID. Messages are in English and never contain your content.

| Reason                                | Connect code                                | Meaning                                                                                                                                                                 |
| ------------------------------------- | ------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`          | `invalid_argument`                          | A field is invalid: empty array or string, bad language tag, missing or malformed tenant, a missing or disallowed model, or a schema that is not valid JSON.            |
| `ERROR_REASON_UNAUTHENTICATED`        | `unauthenticated`                           | The credential is missing, malformed, expired, revoked or unknown.                                                                                                      |
| `ERROR_REASON_PERMISSION_DENIED`      | `permission_denied`                         | The credential lacks the capability, the key grant or your tenant permission, or the `tenantId` you sent is not the tenant of the key.                                  |
| `ERROR_REASON_BILLING_BLOCKED`        | `failed_precondition`                       | Delcore blocked paid use, for example because of the balance, a renewal, a dispute, an operator block or a tenant being deleted.                                        |
| `ERROR_REASON_LIMIT_EXCEEDED`         | `resource_exhausted`                        | The input, item count or schema is larger than the operator's limits.                                                                                                   |
| `ERROR_REASON_RATE_LIMITED`           | `resource_exhausted`                        | Your tenant reached its request rate or concurrency limit. Retry later. A request-rate error includes a `Retry-After` header.                                           |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | `invalid_argument` or `failed_precondition` | The model is not allowed (`invalid_argument`), or it has no registered input and output prices (`failed_precondition`). Another model is not substituted.               |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | `invalid_argument`                          | The schema uses a keyword outside the supported subset.                                                                                                                 |
| `ERROR_REASON_PROVIDER_FAILED`        | `unavailable` or `internal`                 | The AI provider kept failing after the permitted retries (`unavailable`), or it rejected the request (`internal`).                                                      |
| `ERROR_REASON_INVALID_OUTPUT`         | `internal`                                  | The model output was malformed, cut short or did not match the contract.                                                                                                |
| `ERROR_REASON_EXTRACTION_FAILED`      | `failed_precondition`                       | The text does not contain the required information.                                                                                                                     |
| `ERROR_REASON_TIMEOUT`                | `deadline_exceeded`                         | The operator's execution timeout elapsed. It covers the checks and all attempts.                                                                                        |
| `ERROR_REASON_CANCELED`               | `canceled`                                  | The request was canceled or the connection closed.                                                                                                                      |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | `unavailable`                               | An authorization, price or billing check could not be completed, or DeliTools could not record the usage of the call. Paid use fails closed, and you receive no result. |

A request that is larger than the operator's overall request size limit can receive a plain `resource_exhausted` error without these details. See [Troubleshooting](/services/delitools/troubleshooting) for what to do.

## Limits and retries

The operator configures these limits, and they can change: input size, number of translation items, schema size and depth, execution timeout, request rate and burst per tenant, concurrent executions per tenant, and the maximum number of transient retries. Contact the service operator for the values that apply to you. A limit is a protection, not a throughput guarantee.

- DeliTools retries only transient AI provider failures, such as network errors, server errors and rate-limit responses, up to the configured count. It never retries to repair invalid output.
- Before each retry, DeliTools checks your authorization again. If your key was revoked or expired, you lost permission or billing is blocked, DeliTools stops and returns the matching error. A provider call that is already running can finish.
- If you cancel the request or close the connection, DeliTools cancels the provider call and stops retrying.
- If your client closes the call or its own timeout ends it, DeliTools treats this as a cancellation.
- The operator's execution timeout covers authorization, validation and every attempt. When it elapses, DeliTools aborts the provider call and returns `ERROR_REASON_TIMEOUT`.
