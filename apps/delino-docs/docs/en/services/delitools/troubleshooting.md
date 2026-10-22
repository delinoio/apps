# DeliTools configuration and troubleshooting

This page helps you set up a client for DeliTools and fix errors. For field rules see the [API reference](/services/delitools/api-reference).

## Configure your client

- **Base URL.** Use the DeliTools API server URL that your service operator gives you. Procedures live at `/delitools.v1.DelitoolsService/Translate` and `/delitools.v1.DelitoolsService/Extract`.
- **Protocol.** Use a Connect client, or send JSON with `POST` and `Content-Type: application/json`. JSON field names use lowerCamelCase.
- **Credential.** Send `Authorization: Bearer <credential>`. Use a Delino login token, or an API key that starts with `dlk_`. Keep keys in server-side code. DeliTools does not support calls from browsers.
- **Tenant.** A login token needs `tenantId`. An API key already identifies its tenant; if you also send `tenantId`, it must match.
- **Model.** Use a model identifier that the operator allows. Extraction uses a fixed model and has no `model` field.
- **Timeouts.** Set a client timeout that suits your largest request. A provider call can take several seconds, and DeliTools can retry transient failures, so the full call can take longer. A request that exceeds the operator's execution timeout returns `ERROR_REASON_TIMEOUT`; if your own timeout closes the call first, DeliTools treats it as a cancellation.
- **Retries.** Retry on `ERROR_REASON_RATE_LIMITED`, `ERROR_REASON_DEPENDENCY_UNAVAILABLE` and `ERROR_REASON_TIMEOUT`, and on `ERROR_REASON_PROVIDER_FAILED` only when the Connect code is `unavailable`, with exponential backoff and a small random delay. Every retry is a new execution that can be charged. See [Billing](/services/delitools/billing). Do not retry unchanged requests that fail validation.

## Fix an error

| Reason                                | What to do                                                                                                                                                                                                                                                   |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `ERROR_REASON_INVALID_INPUT`          | Fix the request. Check for empty strings, language tags, the tenant ID, the model and whether the schema is valid JSON.                                                                                                                                      |
| `ERROR_REASON_UNAUTHENTICATED`        | Check the token or key. It may be mistyped, expired, revoked or belong to a deleted tenant. Create a new key if you lost the secret. If every valid key suddenly fails, tell the service operator: a misconfigured service identity makes keys look invalid. |
| `ERROR_REASON_PERMISSION_DENIED`      | Check that the key grants `translate` or `extract`, that the user still has permission in the tenant, and that any `tenantId` you send is the tenant of the key.                                                                                             |
| `ERROR_REASON_BILLING_BLOCKED`        | Resolve the billing problem in Delcore, such as the balance or renewal, then retry.                                                                                                                                                                          |
| `ERROR_REASON_LIMIT_EXCEEDED`         | Send less data: fewer items, shorter text, or a smaller schema. Split large jobs into several requests.                                                                                                                                                      |
| `ERROR_REASON_RATE_LIMITED`           | Slow down, reduce concurrent requests and retry after a short delay.                                                                                                                                                                                         |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | If the error is `invalid_argument`, the model is not allowed: choose another one. If it is `failed_precondition`, ask the operator to register the model's prices.                                                                                           |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | Remove keywords outside the [supported subset](/services/delitools/api-reference). The root must be an object schema.                                                                                                                                        |
| `ERROR_REASON_PROVIDER_FAILED`        | With Connect code `unavailable`, wait and retry. With `internal`, the provider rejected the request: do not retry it unchanged, check the request and contact the operator with the request ID.                                                              |
| `ERROR_REASON_INVALID_OUTPUT`         | Retry once. Simplify the schema or the input if it repeats. The attempt is charged.                                                                                                                                                                          |
| `ERROR_REASON_EXTRACTION_FAILED`      | The text lacks required information. Provide the missing details, or make the property optional in your schema. The attempt is charged.                                                                                                                      |
| `ERROR_REASON_TIMEOUT`                | Send smaller requests. Retry later.                                                                                                                                                                                                                          |
| `ERROR_REASON_CANCELED`               | Your client closed the connection. Check for client timeouts or shutdowns.                                                                                                                                                                                   |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | Retry later. DeliTools refuses paid work when it cannot check authorization.                                                                                                                                                                                 |

## Common problems

- **A key works in one request and fails in the next.** A key that expired, was revoked, or belongs to a user who lost permission stops working immediately. Check it with `ListApiKeys`.
- **A plain `resource_exhausted` error without a reason.** Your request body is larger than the operator's overall size limit. Send less data.
- **A result looks wrong.** Models can make mistakes, and a result that follows your schema is not necessarily correct. Make the instructions in your schema's `description` fields clearer and check important values.
- **Unexpected charges.** Failed attempts with known usage and repeated requests are charged. See [Billing](/services/delitools/billing).
- **No usage visible yet.** Usage reaches Delcore shortly after the call. Check again later.

## Ask for support

Include the request ID from the response body or the `X-Delitools-Request-Id` header, the procedure name, the time and the error reason. Do not send your API key, your login token, or the texts you submitted.
