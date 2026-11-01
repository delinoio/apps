# Managed CPA v1 protocol contract

## Scope

`protos/mcpa/v1` owns the Managed CPA management schema `mcpa.proto`. Generated Go messages (`mcpa.pb.go`) and TypeScript messages (`mcpa_pb.ts`) are checked in beside it. Connect-Go bindings live in its `mcpav1connect` subdirectory. The files use Apache-2.0. The [server contract](servers-mcpa-api-contract.md) owns the behavior behind the messages; the [web console contract](apps-mcpa-web-contract.md) owns the console that consumes them.

The protocol covers management only. Model calls use the OpenAI-, Claude- and Gemini-compatible HTTP routes under `/r/{resource_id}/`, which the server contract defines; they are not part of this schema.

## Runtime and Language

The protobuf package is `mcpa.v1` and the service is `McpaService`. The Go package is `github.com/delinoio/apps/protos/mcpa/v1` (`mcpav1`), with Connect bindings in `mcpav1connect`, consumed by `servers/mcpa-api` from the root Go module. The private `@repo/protos` workspace exports the TypeScript bindings as `@repo/protos/mcpa/v1/mcpa_pb`. No Rust bindings are generated for this package. There is no REST surface, handwritten SDK or published client package.

## Users and Operators

The Managed CPA console, interactive MCP sessions and automation that uses Delcore personal service keys. All three call the same RPCs; MCP exposes each RPC as a tool. Operators correlate failures through trace IDs.

## Interfaces and Contracts

### Common messages

- `ErrorDetail` is attached to every Connect error. It carries `reason` (`ErrorReason`), a safe `trace_id`, and `reauthentication_url`, which is set for `ERROR_REASON_MFA_REQUIRED`.
- `Metadata` carries the `trace_id` and is the first field of every response.
- `Page` bounds list requests. `size` defaults to 50 and is at most 200. `after_id` is the opaque `next_after_id` of the previous page; an empty `next_after_id` ends the list.
- Timestamps use `google.protobuf.Timestamp`. Monetary and GiB values are canonical decimal strings, never floats. Byte counts, call counts and latency are `int64`.
- Identifiers are UUID v7 strings. Tenant, user and group IDs are Delcore IDs.

### Enums

| Enum             | Values                                                                                                                                                                                                                                                                                                                                                   |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ErrorReason`    | `INVALID_INPUT` (1), `UNAUTHENTICATED` (2), `PERMISSION_DENIED` (3), `NOT_FOUND` (4), `ALREADY_EXISTS` (5), `FAILED_PRECONDITION` (6), `MFA_REQUIRED` (7), `DEPENDENCY_UNAVAILABLE` (8), `TENANT_INACTIVE` (9), `BUDGET_EXHAUSTED` (10), `MODEL_NOT_ALLOWED` (11), `IDEMPOTENCY_CONFLICT` (12), `TENANT_SERVICE_KEY_REJECTED` (13)                       |
| `ResourceRole`   | `MANAGE` (1), `INVOKE` (2). `MANAGE` implies every `INVOKE` action.                                                                                                                                                                                                                                                                                      |
| `PrincipalKind`  | `USER` (1, a Delcore internal user ID), `GROUP` (2, a Delcore group ID in the resource's tenant)                                                                                                                                                                                                                                                         |
| `Provider`       | `GEMINI` (1, Gemini API keys), `GEMINI_CLI` (2, Gemini CLI OAuth), `VERTEX` (3, Vertex AI API key or service account), `CLAUDE` (4, Anthropic API key or Claude Code OAuth), `CODEX` (5, OpenAI API key or Codex OAuth), `ANTIGRAVITY` (6, OAuth), `KIMI` (7, OAuth or API key), `OPENAI_COMPATIBLE` (8, any OpenAI-compatible upstream with an API key) |
| `CredentialKind` | `OAUTH` (1), `API_KEY` (2)                                                                                                                                                                                                                                                                                                                               |
| `EndpointFormat` | `OPENAI` (1: Chat Completions, Completions, Responses and models), `CLAUDE` (2: Messages, token counting and models), `GEMINI` (3: generateContent, streamGenerateContent and models)                                                                                                                                                                    |
| `LoadBalancing`  | `ROUND_ROBIN` (1)                                                                                                                                                                                                                                                                                                                                        |
| `OauthFlowKind`  | `AUTHORIZATION_CODE` (1), `DEVICE_CODE` (2)                                                                                                                                                                                                                                                                                                              |
| `OauthFlowState` | `PENDING` (1), `COMPLETED` (2), `FAILED` (3), `EXPIRED` (4)                                                                                                                                                                                                                                                                                              |
| `CallStatus`     | `SUCCEEDED` (1), `PROVIDER_ERROR` (2), `PROVIDER_AUTH_ERROR` (3), `CLIENT_CANCELLED` (4), `STREAM_INTERRUPTED` (5), `REJECTED_MODEL_NOT_ALLOWED` (6), `REJECTED_BUDGET` (7), `REJECTED_TENANT` (8), `REJECTED_DEPENDENCY` (9), `NO_CREDENTIAL` (10), `INTERNAL_ERROR` (11)                                                                               |
| `StatusClass`    | `2XX` (1), `3XX` (2), `4XX` (3, excluding 429), `429` (4), `5XX` (5), `NONE` (6, no provider response)                                                                                                                                                                                                                                                   |
| `AuditAction`    | `RESOURCE_CREATED` (1), `RESOURCE_UPDATED` (2), `RESOURCE_DELETED` (3), `CREDENTIAL_REGISTERED` (4), `CREDENTIAL_REMOVED` (5), `CALL_KEY_ISSUED` (6), `CALL_KEY_REVOKED` (7), `ACL_CHANGED` (8), `ROUTING_CHANGED` (9), `RESOURCE_BUDGET_CHANGED` (10), `TENANT_BUDGET_CHANGED` (11)                                                                     |
| `ActorKind`      | `CONSOLE` (1), `MCP` (2), `PERSONAL_KEY` (3), `SYSTEM` (4)                                                                                                                                                                                                                                                                                               |

Each enum's full value names use its prefix, for example `ERROR_REASON_MFA_REQUIRED` and `PROVIDER_GEMINI_CLI`. The zero value `<PREFIX>_UNSPECIFIED` is invalid input wherever a request requires the field. The `Provider` enum tracks the providers that the pinned CLIProxyAPI SDK supports; a provider added by an SDK upgrade receives a new value.

### Entity messages

- `Resource`: `id`, `tenant_id`, `name`, `description`, `routing` (`RoutingSettings`), `endpoints` (one `Endpoint` per format), `caller_role` (the caller's effective role), `created_by_user_id`, `create_time` and `update_time`.
- `Endpoint`: `format` and the absolute `base_url`, for example `https://mcpa.api.delino.io/r/<resource_id>/v1` for OpenAI.
- `RoutingSettings`: `load_balancing` and `model_allowlist`. An empty allowlist allows every model the resource's credentials can serve; a non-empty list allows only the exact listed names.
- `AclEntry`: `resource_id`, `principal_kind`, `principal_id`, `role`, `created_by_user_id` and `create_time`.
- `Credential`: `id`, `resource_id`, `provider`, `kind`, `name` (customer label), `account_label` (a non-secret OAuth account identifier, empty for API keys), `secret_hint` (last four characters of an API key, empty for OAuth), `base_url` (for `OPENAI_COMPATIBLE` and optional Vertex endpoints), `models` (declared upstream models for `OPENAI_COMPATIBLE`), `created_by_user_id` and `create_time`. It never contains a secret.
- `ProviderOAuthFlow`: `id`, `resource_id`, `provider`, `kind`, `state`, `authorization_url`, `user_code`, `verification_uri`, `expire_time`, `credential_id` (set when `COMPLETED`) and `failure_message` (safe text when `FAILED`).
- `CallKey`: `id`, `resource_id`, `name`, `display_prefix` (never the secret part), `issuer_user_id`, `create_time`, optional `expire_time`, `last_used_time` and `revoke_time`.
- `Budget`: `limit_usd` (empty means no limit), `spent_usd` (estimated measured spend in the current period), `period_start`, `period_end` and `exhausted`.
- `CallRecord`: `id`, `resource_id`, `call_key_id`, `provider`, `model`, `status`, `provider_status_class`, `error_class` (bounded, secret-free classifier), `request_bytes`, `response_bytes`, `latency_ms`, `request_id` (the debug identifier returned to the caller), `start_time`, `format` and `streaming`. It never contains request or response bodies.
- `CallFilter`: `resource_ids`, `models`, `providers`, `call_key_ids`, `statuses`, `start_time` and `end_time`. Empty repeated fields do not filter.
- `UsageRow`: a daily aggregate of `day`, `resource_id`, `model`, `provider`, `call_key_id` and `status`, with `call_count`, `request_bytes` and `response_bytes`.
- `UsageSummary`: `call_count`, `request_bytes`, `response_bytes`, `total_bytes`, `total_gib` (`total_bytes / 2^30`) and `estimated_usd` (`total_gib × 0.07`). Delcore issues the final bill.
- `AuditEvent`: `id`, `tenant_id`, `resource_id`, `action`, `actor_kind`, `actor_user_id`, `target_id` (the changed entity), `details` (bounded, secret-free attributes) and `occur_time`.
- `Viewer`: `user_id`, `tenants` (`Tenant` with `id`, `name` and `personal`) and `actor_kind`. `Group` carries `id` and `name`.

### RPCs

All RPCs are unary.

| RPC                        | Request fields                                                            | Response fields                                                   |
| -------------------------- | ------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| `GetViewer`                | none                                                                      | `viewer`                                                          |
| `ListTenantGroups`         | `tenant_id`                                                               | `groups`                                                          |
| `CreateResource`           | `request_key` (UUID v7), `tenant_id`, `name`, `description`               | `resource`                                                        |
| `GetResource`              | `resource_id`                                                             | `resource`                                                        |
| `ListResources`            | `tenant_id`, `page`                                                       | `resources` (only those with a caller role), `next_after_id`      |
| `UpdateResource`           | `resource_id`, `name`, `description`                                      | `resource`                                                        |
| `DeleteResource`           | `resource_id`, `confirm_name` (must equal the name)                       | none                                                              |
| `UpdateRouting`            | `resource_id`, `routing`                                                  | `resource`                                                        |
| `ListAclEntries`           | `resource_id`                                                             | `entries`                                                         |
| `PutAclEntry`              | `resource_id`, `principal_kind`, `principal_id`, `role`                   | `entry`                                                           |
| `RemoveAclEntry`           | `resource_id`, `principal_kind`, `principal_id`                           | none                                                              |
| `ListCredentials`          | `resource_id`                                                             | `credentials`                                                     |
| `RegisterApiKeyCredential` | `resource_id`, `provider`, `name`, `api_key`, `base_url`, `models`        | `credential`                                                      |
| `RemoveCredential`         | `resource_id`, `credential_id`                                            | none                                                              |
| `StartProviderOAuth`       | `resource_id`, `provider`, `name`                                         | `flow`                                                            |
| `CompleteProviderOAuth`    | `resource_id`, `flow_id`, `callback_url` (the full provider redirect URL) | `flow`                                                            |
| `GetProviderOAuthFlow`     | `resource_id`, `flow_id`                                                  | `flow`                                                            |
| `IssueCallKey`             | `request_key` (UUID v7), `resource_id`, `name`, optional `expire_time`    | `call_key`, `secret` (only on the first execution)                |
| `ListCallKeys`             | `resource_id`, `page`                                                     | `call_keys` (all for managers, own for invokers), `next_after_id` |
| `RevokeCallKey`            | `resource_id`, `call_key_id`                                              | `call_key`                                                        |
| `GetResourceBudget`        | `resource_id`                                                             | `budget`                                                          |
| `SetResourceBudget`        | `resource_id`, `limit_usd` (empty removes the limit)                      | `budget`                                                          |
| `GetTenantBudget`          | `tenant_id`                                                               | `budget`                                                          |
| `SetTenantBudget`          | `tenant_id`, `limit_usd`                                                  | `budget`                                                          |
| `GetUsage`                 | `tenant_id`, `filter`, `page`                                             | `summary`, `rows`, `next_after_id`                                |
| `ListCallRecords`          | `tenant_id`, `filter`, `page`                                             | `records`, `next_after_id`                                        |
| `ListAuditEvents`          | `resource_id`, `page`                                                     | `events`, `next_after_id`                                         |
| `ListTenantAuditEvents`    | `tenant_id`, `page`                                                       | `events` (tenant-level events only), `next_after_id`              |

Every response also carries `metadata`. The server contract owns the authorization requirement, validation bounds and error reasons of each RPC.

### Compatibility rules

- Preserve field numbers, enum values and RPC names. New fields, enum values and RPCs are additive.
- Consumers treat an unrecognized `ErrorReason` as the failure of its Connect code and an unrecognized enum value as unknown, without failing the whole response.
- `CreateResource` and `IssueCallKey` are idempotent by `request_key` for 30 days. Same input replays the original result; `IssueCallKey` replays return an empty `secret`. Different input fails with `ERROR_REASON_IDEMPOTENCY_CONFLICT`. Other mutations are naturally idempotent or fail with a typed reason.

## Storage

Wire messages are not the database schema. The server contract owns persistence and retention.

## Security

Secrets cross the wire only in two directions: `RegisterApiKeyCredentialRequest.api_key` from the client, and `IssueCallKeyResponse.secret` once to the client. No other message carries provider tokens, API keys, call key secrets or Delcore keys. Request and response bodies of model calls never appear in any message. Management credentials travel in the cookie session, the `X-Mcpa-CSRF` header or the `Authorization` header, never in message fields. `ErrorDetail` and error messages never echo secrets or submitted content.

## Logging

Correlate failures through `Metadata.trace_id` and `ErrorDetail.trace_id`. Call records carry the `request_id` that the call surface returns in `X-Mcpa-Request-Id`. Logs never include secrets.

## Build and Test

Use the root Buf generation, lint, formatting, FILE breaking and freshness checks under the [repository workflow](repository-workflow-contract.md); `protos/scripts/` owns Buf entry resolution and breaking checks. Root `pnpm proto:generate` generates Go and TypeScript bindings together and must preserve schemas and instruction files. The root Buf input selection includes `protos/mcpa/v1`. Run `go build ./...` and `go vet ./...` for the generated Go package. Protocol changes select protocol validation, the `servers/mcpa-api` checks and the `apps/mcpa` checks.

## Dependencies and Integrations

Connect-Go, Protobuf and the TypeScript protobuf runtime use the root pinned versions. The server consumes the bindings from the root Go module; the console consumes them through `@repo/protos` and `@connectrpc/connect-query`. The server separately calls the [Delcore protocol](protos-delcore-v1-contract.md) as a registered consumer service; this schema adds nothing to Delcore.

## Change Triggers

Update this contract, the [server contract](servers-mcpa-api-contract.md), the [web console contract](apps-mcpa-web-contract.md) and the [project index](project-mcpa.md) when the public interface changes. Update server and console consumers in the same change. Describe interface changes and validation results in the pull request.

## References

- [Project index](project-mcpa.md)
- [Server contract](servers-mcpa-api-contract.md)
- [Web console contract](apps-mcpa-web-contract.md)
- [Repository defaults](repository-defaults.md)
- [Repository boundary](repository-boundary-contract.md)
